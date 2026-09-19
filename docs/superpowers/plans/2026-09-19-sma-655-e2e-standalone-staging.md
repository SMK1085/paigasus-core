# SMA-655 — stage the console standalone tree in `build` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The iam-console and gateway-console e2e tiers stop writing into a build tree, so Moon can run them at the same time without one tier wiping the other's static files.

**Architecture:** Each app's Moon `build` script copies `.next/static` into its own standalone tree after `next build`. Both e2e `global-setup.ts` files only check the staged tree through one function, `assertStagedBuild`. Two unit-test files pin the result: an allowlist scan that reds any `fs` write in `tests/e2e/**`, and a build-side case that reds if `build` stops staging.

**Tech Stack:** Moon 2.5.3 `script:` blocks (bash, `set -euo pipefail`), TypeScript, Node 24, vitest 5, Playwright, Next 16.3.4 standalone output.

**Spec:** `docs/superpowers/specs/2026-09-19-sma-655-e2e-standalone-staging-design.md`

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Commits: conventional, scope `ts` (or `docs` for plan/spec files), each message ends with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`. **Add commits, never `--amend`.** Never `--no-verify`.
- Bash tool PATH: prefix commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` and `export PROTO_REPORTER=text`.
- Work ONLY in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-655-e2e-standalone-race`, branch `feature/sma-655-e2e-standalone-race`. Check `git branch --show-current` before the first commit of each task.
- Scratch output goes to `SP=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/f3b305a0-b377-43c2-a34e-0e8a797ccd30/scratchpad`. Shell state does not persist between tool calls: set `SP` again in every call that uses it.
- Run every command in the foreground. Do not start a background job and end your turn.
- `moon.yml` script lines: no pipe into `head`, `grep -q`, `grep -m` or `awk … exit` (`ci/actionlint/run.sh` check 13); no here-string; lowercase shell variable names; `if … then … fi`, never `[ … ] && …` as a last command.
- The two apps' copies of `tests/e2e/support/staged-build.ts`, `tests/unit/staged-build.test.ts` and `tests/unit/e2e-read-only.test.ts` are BYTE-IDENTICAL (the precedent is `tests/unit/hydration.test.ts`). Check with `diff`.
- The e2e setups import only these from `node:fs`: `existsSync`, `readFileSync`, `readdirSync`, `statSync`, `lstatSync`.
- The error hint names `moon run <app>-ts:build --force`.
- `ts:fmt` (Prettier) is a separate whole-tree gate: run it after any `.ts` edit.

## File map

| File | Change | Responsibility |
|---|---|---|
| `ts/apps/{iam,gateway}-console/tests/e2e/support/staged-build.ts` | Create (identical) | `assertStagedBuild(appDir, standaloneDir, buildTask)`: the one read-only check of a staged tree |
| `ts/apps/{iam,gateway}-console/tests/unit/staged-build.test.ts` | Create (identical) | Unit cases for `assertStagedBuild` on temporary trees |
| `ts/apps/{iam,gateway}-console/moon.yml` | Modify `build` script + comment; add `'moon.yml'` to `test` inputs | Stage the tree; make a `moon.yml`-only edit select `test` |
| `ts/apps/{iam,gateway}-console/tests/standalone-staging.test.ts` | Create (differ only in app name) | Build-side pin on the real build tree |
| `ts/apps/{iam,gateway}-console/tests/unit/e2e-read-only.test.ts` | Create (identical) | Allowlist scan of `tests/e2e/**` + `playwright.config.ts`, with fixtures |
| `ts/apps/{iam,gateway}-console/tests/e2e/global-setup.ts` | Rewrite | Read-only checks |
| `CLAUDE.md` | Modify | Correct the stale sentence; add the gotcha |
| `docs/superpowers/specs/2026-09-19-sma-655-e2e-standalone-staging-design.md` | Append § 10 | Measurements (baseline and after) |

---

### Task 1: Baseline measurement on the unchanged code (spec V2, first half)

No code changes. This records how often the race fails BEFORE the fix, so the after-run has something to compare with.

**Files:**
- Modify: `docs/superpowers/specs/2026-09-19-sma-655-e2e-standalone-staging-design.md` (append § 10)

- [ ] **Step 1: Check the preconditions**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-655-e2e-standalone-race
git branch --show-current            # feature/sma-655-e2e-standalone-race
git diff --stat e1413535 -- ts        # empty: no ts change yet
docker info >/dev/null && echo docker-ok
```

Expected: the branch name, no output from the diff, `docker-ok`. If Docker is not reachable, STOP and report: the gateway-console tier needs it.

- [ ] **Step 2: Run both tiers at the same time, three times**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
SP=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/f3b305a0-b377-43c2-a34e-0e8a797ccd30/scratchpad
for n in 1 2 3; do
  moon run iam-console-ts:test-e2e gateway-console-ts:test-e2e --force > "$SP/baseline-$n.log" 2>&1
  echo "run $n rc=$?"
done
```

Use a 600000 ms tool timeout per call; if one call cannot hold three runs, run them one per call. Expected: at least one non-zero `rc` is likely, but any result is valid data.

- [ ] **Step 3: Classify each run**

For each log, record: the rc; whether it contains `ENOTEMPTY`; whether it contains `React never hydrated`; which task failed. Use `grep -c` on the file (NOT `grep -q` in a pipe):

```bash
for n in 1 2 3; do f="$SP/baseline-$n.log"; echo "run $n: ENOTEMPTY=$(grep -c ENOTEMPTY "$f") hydrated=$(grep -c 'React never hydrated' "$f") failed=$(grep -cE 'test-e2e.*(failed|FAIL)' "$f")"; done
```

- [ ] **Step 4: Append § 10 to the spec**

Append this, filled with the measured values:

```markdown
## 10. Measurements

### 10.1 Baseline (base commit e1413535, 2026-09-19)

Command: `moon run iam-console-ts:test-e2e gateway-console-ts:test-e2e --force`, three runs.

| Run | rc | ENOTEMPTY | `React never hydrated` | Failed task |
|---|---|---|---|---|
| 1 | … | … | … | … |
| 2 | … | … | … | … |
| 3 | … | … | … | … |
```

If all three runs pass, write that plainly: the race did not reproduce in three runs, so the after-run cannot show an improvement by itself, only no regression.

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/specs/2026-09-19-sma-655-e2e-standalone-staging-design.md
git commit -m "docs(ts): record the SMA-655 race baseline" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: `assertStagedBuild`, the read-only check (both apps)

**Files:**
- Create: `ts/apps/iam-console/tests/e2e/support/staged-build.ts`
- Create: `ts/apps/iam-console/tests/unit/staged-build.test.ts`
- Create: identical copies of both under `ts/apps/gateway-console/`

**Interfaces:**
- Produces: `export function assertStagedBuild(appDir: string, standaloneDir: string, buildTask: string): void` and `export class StagedBuildError extends Error`. Tasks 3 and 4 call `assertStagedBuild`.

- [ ] **Step 1: Write the failing test** — `ts/apps/iam-console/tests/unit/staged-build.test.ts`

```ts
// SPDX-License-Identifier: Apache-2.0
//
// assertStagedBuild (SMA-655). Each case builds a temporary app tree and a temporary standalone
// tree, then damages exactly one thing, so a failure can only come from what the case changes.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { assertStagedBuild, StagedBuildError } from '../e2e/support/staged-build';

const BUILD_ID = 'abc123';
const TASK = 'demo-ts:build';
const roots: string[] = [];

function write(file: string, content: string): void {
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, content);
}

/** A complete, correctly staged pair of trees. */
function stagedPair(): { appDir: string; standaloneDir: string } {
  const root = mkdtempSync(path.join(tmpdir(), 'staged-build-'));
  roots.push(root);
  const appDir = path.join(root, 'app');
  const standaloneDir = path.join(appDir, '.next', 'standalone', 'apps', 'demo');
  write(path.join(appDir, '.next', 'BUILD_ID'), BUILD_ID);
  write(path.join(standaloneDir, '.next', 'BUILD_ID'), BUILD_ID);
  for (const tree of [path.join(appDir, '.next', 'static'), path.join(standaloneDir, '.next', 'static')]) {
    write(path.join(tree, BUILD_ID, '_buildManifest.js'), 'manifest');
    write(path.join(tree, 'chunks', 'main.js'), 'main-chunk');
  }
  return { appDir, standaloneDir };
}

function failure(appDir: string, standaloneDir: string): StagedBuildError {
  try {
    assertStagedBuild(appDir, standaloneDir, TASK);
  } catch (error) {
    if (error instanceof StagedBuildError) return error;
    throw error;
  }
  throw new Error('expected assertStagedBuild to throw, and it did not');
}

afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe('assertStagedBuild', () => {
  it('accepts a complete staged tree', () => {
    const { appDir, standaloneDir } = stagedPair();
    expect(() => assertStagedBuild(appDir, standaloneDir, TASK)).not.toThrow();
  });

  it('accepts extra files in the staged tree, because a cache-hit restore merges', () => {
    const { appDir, standaloneDir } = stagedPair();
    write(path.join(standaloneDir, '.next', 'static', 'old-build', '_buildManifest.js'), 'stale');
    expect(() => assertStagedBuild(appDir, standaloneDir, TASK)).not.toThrow();
  });

  it('refuses an empty BUILD_ID, which would make every path check name static/ itself', () => {
    const { appDir, standaloneDir } = stagedPair();
    write(path.join(appDir, '.next', 'BUILD_ID'), '  \n');
    expect(failure(appDir, standaloneDir).message).toMatch(/BUILD_ID is missing or empty/);
  });

  it('trims the BUILD_ID, so a trailing newline is not a mismatch', () => {
    const { appDir, standaloneDir } = stagedPair();
    write(path.join(appDir, '.next', 'BUILD_ID'), `${BUILD_ID}\n`);
    expect(() => assertStagedBuild(appDir, standaloneDir, TASK)).not.toThrow();
  });

  it('refuses a standalone tree from a different build', () => {
    const { appDir, standaloneDir } = stagedPair();
    write(path.join(standaloneDir, '.next', 'BUILD_ID'), 'other-build');
    expect(failure(appDir, standaloneDir).message).toMatch(/BUILD_ID is 'other-build', not 'abc123'/);
  });

  it('refuses a staged tree with no directory for this build', () => {
    const { appDir, standaloneDir } = stagedPair();
    rmSync(path.join(standaloneDir, '.next', 'static', BUILD_ID), { recursive: true });
    expect(failure(appDir, standaloneDir).message).toMatch(/static\/abc123 is not a directory/);
  });

  it('refuses a staged tree with a missing file, and names it', () => {
    const { appDir, standaloneDir } = stagedPair();
    rmSync(path.join(standaloneDir, '.next', 'static', 'chunks', 'main.js'));
    expect(failure(appDir, standaloneDir).message).toMatch(/missing: chunks\/main\.js/);
  });

  it('refuses a staged file whose size differs, and names it', () => {
    const { appDir, standaloneDir } = stagedPair();
    write(path.join(standaloneDir, '.next', 'static', 'chunks', 'main.js'), 'short');
    expect(failure(appDir, standaloneDir).message).toMatch(/size differs: chunks\/main\.js/);
  });

  it('refuses an app tree with no static files at all', () => {
    const { appDir, standaloneDir } = stagedPair();
    rmSync(path.join(appDir, '.next', 'static'), { recursive: true });
    expect(failure(appDir, standaloneDir).message).toMatch(/no files under/);
  });

  it('names the forced build in every failure', () => {
    const { appDir, standaloneDir } = stagedPair();
    write(path.join(standaloneDir, '.next', 'BUILD_ID'), 'other-build');
    expect(failure(appDir, standaloneDir).message).toContain('moon run demo-ts:build --force');
  });
});
```

- [ ] **Step 2: Run it to see it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/iam-console && pnpm exec vitest run tests/unit/staged-build.test.ts; cd -
```

Expected: FAIL, the import `../e2e/support/staged-build` cannot be resolved.

- [ ] **Step 3: Write the implementation** — `ts/apps/iam-console/tests/e2e/support/staged-build.ts`

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The ONE check that a standalone tree is staged for an e2e tier (SMA-655). The standalone output
// has no .next/static of its own (measured, SMA-510), so the Moon `build` task copies it in
// (moon.yml). This module only READS. No e2e tier may write into a build tree: iam-console's tree
// serves both this app's tier and gateway-console's two-zone tier, Moon runs the two at the same
// time, and when each tier staged the tree for itself, one tier's delete wiped it under the other.
// tests/unit/e2e-read-only.test.ts reds any fs write under tests/e2e/.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';

export class StagedBuildError extends Error {
  override readonly name = 'StagedBuildError';
}

function readBuildId(file: string): string {
  return existsSync(file) ? readFileSync(file, 'utf8').trim() : '';
}

/** Every regular file under `root`, as paths relative to it, with `/` separators. */
function listFiles(root: string, dir: string = root, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) listFiles(root, full, out);
    else if (entry.isFile()) out.push(path.relative(root, full).split(path.sep).join('/'));
  }
  return out;
}

/**
 * Throws a StagedBuildError unless `standaloneDir` holds this build's static tree: the same
 * non-empty BUILD_ID as `appDir`, a `static/<BUILD_ID>` directory, and every file under
 * `appDir/.next/static` with the same size. Extra staged files pass, because a Moon cache-hit
 * restore merges into the existing tree instead of replacing it.
 */
export function assertStagedBuild(appDir: string, standaloneDir: string, buildTask: string): void {
  const fail = (reason: string): never => {
    throw new StagedBuildError(
      `${reason}. The \`build\` task stages the standalone tree: run \`moon run ${buildTask} --force\`. ` +
        '`--force` is needed because a bare `next build` deletes the staged tree without staging it ' +
        'again, and Moon then sees an unchanged hash and skips the build.',
    );
  };
  const buildId = readBuildId(path.join(appDir, '.next', 'BUILD_ID'));
  if (buildId === '') fail(`${path.join(appDir, '.next', 'BUILD_ID')}: BUILD_ID is missing or empty`);
  const stagedId = readBuildId(path.join(standaloneDir, '.next', 'BUILD_ID'));
  if (stagedId !== buildId) fail(`the standalone tree's BUILD_ID is '${stagedId}', not '${buildId}'`);
  const sourceStatic = path.join(appDir, '.next', 'static');
  const stagedStatic = path.join(standaloneDir, '.next', 'static');
  const buildDir = path.join(stagedStatic, buildId);
  if (!existsSync(buildDir) || !statSync(buildDir).isDirectory()) fail(`${buildDir.split(path.sep).join('/')} is not a directory`);
  const sources = existsSync(sourceStatic) ? listFiles(sourceStatic) : [];
  if (sources.length === 0) fail(`no files under ${sourceStatic}`);
  const problems: string[] = [];
  for (const rel of sources) {
    const staged = path.join(stagedStatic, rel);
    if (!existsSync(staged)) problems.push(`missing: ${rel}`);
    else if (statSync(staged).size !== statSync(path.join(sourceStatic, rel)).size) problems.push(`size differs: ${rel}`);
  }
  if (problems.length > 0) fail(`the staged static tree does not match ${sourceStatic} (${problems.length}): ${problems.slice(0, 5).join(', ')}`);
}
```

Note: the "not a directory" message uses `/` separators so the test regex `static\/abc123` matches on every platform.

- [ ] **Step 4: Run it to see it pass**

```bash
cd ts/apps/iam-console && pnpm exec vitest run tests/unit/staged-build.test.ts; cd -
```

Expected: PASS, 10 tests.

- [ ] **Step 5: Copy both files to gateway-console and run there**

```bash
cp ts/apps/iam-console/tests/e2e/support/staged-build.ts ts/apps/gateway-console/tests/e2e/support/staged-build.ts
cp ts/apps/iam-console/tests/unit/staged-build.test.ts ts/apps/gateway-console/tests/unit/staged-build.test.ts
cd ts/apps/gateway-console && pnpm exec vitest run tests/unit/staged-build.test.ts; cd -
```

Expected: PASS, 10 tests.

- [ ] **Step 6: Lint, format, type-check**

```bash
moon run ts:lint ts:fmt iam-console-ts:typecheck gateway-console-ts:typecheck
```

Expected: all pass. If `ts:fmt` fails, run `pnpm -C ts exec prettier --write <the four files>` and re-run.

- [ ] **Step 7: Commit**

```bash
git add ts/apps/iam-console/tests/e2e/support/staged-build.ts ts/apps/iam-console/tests/unit/staged-build.test.ts ts/apps/gateway-console/tests/e2e/support/staged-build.ts ts/apps/gateway-console/tests/unit/staged-build.test.ts
git commit -m "test(ts): add a read-only check of the staged standalone tree (SMA-655)" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: `build` stages the tree; the build-side pin (both apps)

**Files:**
- Modify: `ts/apps/iam-console/moon.yml` (`build` comment + `script`, around `:51-77`; `test` inputs, after the `'playwright.config.ts'` entry)
- Modify: `ts/apps/gateway-console/moon.yml` (`build` comment + `script`, around `:42-63`; `test` inputs, after the `'playwright.config.ts'` entry)
- Create: `ts/apps/iam-console/tests/standalone-staging.test.ts`
- Create: `ts/apps/gateway-console/tests/standalone-staging.test.ts`

**Interfaces:**
- Consumes: `assertStagedBuild` from Task 2; `APP_DIR`, `STANDALONE_APP_DIR` from each app's existing `tests/e2e/support/paths.ts`.

- [ ] **Step 1: Write the failing build-side test** — `ts/apps/iam-console/tests/standalone-staging.test.ts`

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The build-side pin of SMA-655. `iam-console-ts:build` stages .next/static into the standalone
// tree (moon.yml), and this reds if a later edit removes that staging. It sits next to
// standalone-runtime.test.ts because both read the real build output; `test` depends on
// `~:build`, and lists moon.yml as an input so a moon.yml-only edit selects it.
import { describe, it } from 'vitest';
import { APP_DIR, STANDALONE_APP_DIR } from './e2e/support/paths';
import { assertStagedBuild } from './e2e/support/staged-build';

describe('the build stages the standalone tree (SMA-655)', () => {
  it("holds this build's static tree", () => {
    assertStagedBuild(APP_DIR, STANDALONE_APP_DIR, 'iam-console-ts:build');
  });
});
```

Create `ts/apps/gateway-console/tests/standalone-staging.test.ts` with the same content, except: `gateway-console-ts:build` in the call, and `\`gateway-console-ts:build\`` in the header comment.

- [ ] **Step 2: Run it to see it fail**

```bash
moon run iam-console-ts:build gateway-console-ts:build
cd ts/apps/iam-console && pnpm exec vitest run tests/standalone-staging.test.ts; cd -
cd ts/apps/gateway-console && pnpm exec vitest run tests/standalone-staging.test.ts; cd -
```

Expected: both FAIL with a `StagedBuildError`. A fresh build has no staged `.next/static` (Next's `cleanDistDir`), so the message is `…/static/<id> is not a directory`. If one PASSES, a previous e2e run staged the tree and Moon served a cache hit: run `moon run <app>-ts:build --force` and repeat. Record which message you saw.

- [ ] **Step 3: Add the staging to iam-console's `build`**

In `ts/apps/iam-console/moon.yml`, append this paragraph to the END of the comment block above `build`'s `script:` (after the line ending `ci/tailwind-source/README.md.`):

```yaml
    #
    # SMA-655: the standalone tree is staged HERE, never in an e2e setup. The standalone output has
    # no .next/static of its own (measured, SMA-510), and without it every client chunk is a 404.
    # Two e2e tiers serve from this tree — this app's own and gateway-console's two-zone tier — and
    # Moon runs them at the same time; when each tier staged the tree for itself, one tier's
    # delete wiped it under the other. `outputs` covers the staged files, so a cache hit restores
    # them. `next build` already empties .next/standalone (cleanDistDir), so the `rm -rf` deletes
    # nothing today: it keeps `cp -R` copying to an ABSENT destination (BSD and GNU cp differ when
    # it exists), and it survives a future `cleanDistDir: false`. The last check fails the build
    # when the staged tree is not this build's. tests/standalone-staging.test.ts pins the result.
```

Then replace the `script:` block with:

```yaml
    script: |
      set -euo pipefail
      rm -rf .next/static
      pnpm exec next build
      if [ ! -f .next/standalone/apps/iam-console/server.js ]; then
        echo "iam-console: standalone entry point missing — output: 'standalone' did not take effect" >&2
        exit 1
      fi
      dest=.next/standalone/apps/iam-console
      rm -rf "$dest/.next/static" "$dest/public"
      cp -R .next/static "$dest/.next/static"
      if [ -d public ]; then
        cp -R public "$dest/public"
      fi
      build_id="$(cat .next/BUILD_ID)"
      staged_id="$(cat "$dest/.next/BUILD_ID" 2>/dev/null || true)"
      if [ -z "$build_id" ] || [ "$staged_id" != "$build_id" ] || [ ! -d "$dest/.next/static/$build_id" ]; then
        echo "iam-console: staging the standalone tree failed (BUILD_ID '$build_id', staged '$staged_id')" >&2
        exit 1
      fi
```

- [ ] **Step 4: Add the same staging to gateway-console's `build`**

In `ts/apps/gateway-console/moon.yml`, append the SAME comment paragraph to the end of the comment above `build`'s `script:`, with one change: replace "this app's own and gateway-console's two-zone tier" with "for iam-console, that app's own tier and this app's two-zone tier; for this app, its own tier". Then replace the `script:` block with the iam-console block above, with every `iam-console` replaced by `gateway-console` (the `server.js` path, `dest`, and both echo messages).

- [ ] **Step 5: Add `'moon.yml'` to both `test` tasks' inputs**

In BOTH `ts/apps/iam-console/moon.yml` and `ts/apps/gateway-console/moon.yml`, in the `test:` task's `inputs:` (NOT `test-e2e`), directly after the `- 'playwright.config.ts'` entry, add:

```yaml
      # SMA-655: `build`'s script stages the standalone tree and tests/standalone-staging.test.ts
      # checks it. This task carries `merge: replace`, and nothing else makes a moon.yml-only edit
      # select it, so without this entry a PR that removes the staging runs no check at all.
      - 'moon.yml'
```

- [ ] **Step 6: Rebuild and see the test pass**

```bash
moon run iam-console-ts:build gateway-console-ts:build
cd ts/apps/iam-console && pnpm exec vitest run tests/standalone-staging.test.ts; cd -
cd ts/apps/gateway-console && pnpm exec vitest run tests/standalone-staging.test.ts; cd -
```

Expected: both builds run (the script change is a new hash) and pass; both tests PASS.

- [ ] **Step 7: Prove selection (mutation M4 and its control)**

```bash
cd ts/apps/iam-console && echo '# sma-655 probe' >> moon.yml && cd -
moon query tasks --affected > "$SP/affected-moonyml.json" 2>&1
python3 -c "import json;d=json.load(open('$SP/affected-moonyml.json'));print(sorted(f'{p}:{t}' for p,ts in d['tasks'].items() for t in ts))"
```

(`SP` is the scratchpad path from Task 1.) Expected: the list contains `iam-console-ts:test`. Then remove the probe line with the Edit tool (do NOT use `git checkout --`, which also reverts uncommitted work). Then temporarily delete the `- 'moon.yml'` input line from iam-console's `test`, add the probe line again, re-run the query, and confirm `iam-console-ts:test` is ABSENT. Restore the input line and remove the probe line with the Edit tool. Confirm `git diff ts/apps/iam-console/moon.yml` shows only the intended changes. Parse the JSON as shown: one target per `tasks[project][task]`, never a grep on `"target"` (CLAUDE.md).

- [ ] **Step 8: Mutations M2a and M2b**

M2a: in iam-console's `script`, delete the `rm -rf "$dest/…"` line, the `cp -R .next/static …` line and the `if [ -d public ]` block (keep the last check). Run `moon run iam-console-ts:build --force`. Expected: the build FAILS with `iam-console: staging the standalone tree failed`.

M2b: also delete the last check (the `build_id`, `staged_id` lines and the final `if` block). Run `moon run iam-console-ts:build --force`, then `cd ts/apps/iam-console && pnpm exec vitest run tests/standalone-staging.test.ts`. Expected: the build passes and the test FAILS with `StagedBuildError`.

Restore the script with the Edit tool, run `moon run iam-console-ts:build --force`, and confirm the test passes again. Record both results for the PR description.

- [ ] **Step 9: Run both apps' `test` tasks**

```bash
moon run iam-console-ts:test gateway-console-ts:test
```

Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add ts/apps/iam-console/moon.yml ts/apps/gateway-console/moon.yml ts/apps/iam-console/tests/standalone-staging.test.ts ts/apps/gateway-console/tests/standalone-staging.test.ts
git commit -m "fix(ts): stage the console standalone tree in build, not in the e2e setups (SMA-655)" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: The allowlist scan, then read-only e2e setups (both apps)

The scan comes first and must red on the CURRENT setups: that is the proof that it detects the real regression.

**Files:**
- Create: `ts/apps/iam-console/tests/unit/e2e-read-only.test.ts` and an identical copy under `ts/apps/gateway-console/tests/unit/`
- Rewrite: `ts/apps/iam-console/tests/e2e/global-setup.ts`
- Rewrite: `ts/apps/gateway-console/tests/e2e/global-setup.ts`

**Interfaces:**
- Consumes: `assertStagedBuild` (Task 2); the paths constants in each app's `tests/e2e/support/paths.ts`.

- [ ] **Step 1: Write the scan** — `ts/apps/iam-console/tests/unit/e2e-read-only.test.ts`

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The e2e tier must not write into a build tree (SMA-655). iam-console's standalone tree serves
// both this app's tier and gateway-console's two-zone tier, Moon runs the two at the same time,
// and when each tier staged the tree for itself, one tier's delete wiped it under the other.
// Staging belongs to the `build` task (moon.yml); the setups only check it.
//
// This is an ALLOWLIST. From `fs`, `node:fs`, `fs/promises` and `node:fs/promises`, only a named
// import from READ_SET, or a type-only import, passes. Every other form is a red: another named
// import, a default or namespace import, a side-effect import, a re-export, `require(...)`,
// `createRequire` and a dynamic `import(...)`.
//
// Limits (spec § 9, R1): this is a text scan, not a parse. It does not see a write through
// `child_process`, or through a helper module outside tests/e2e/. The comment stripper does not
// know regex literals or a template literal's `${…}`: a regex literal holding a quote, `//` or
// `/*`, or a comment inside `${…}`, can shift the scanner's string state, so it can hide real code
// or expose string text as code. No scanned file has such a shape before its imports today.
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const APP_DIR = fileURLToPath(new URL('../..', import.meta.url));
const E2E_DIR = path.join(APP_DIR, 'tests', 'e2e');
const FS_MODULE = /^(?:node:)?fs(?:\/promises)?$/;
const READ_SET: ReadonlySet<string> = new Set(['existsSync', 'readFileSync', 'readdirSync', 'statSync', 'lstatSync']);
const SCRIPT_FILE = /\.(?:[cm]?[jt]s|tsx)$/;
/** File path (relative to the app, `/` separators) → reason. Ships EMPTY; an entry needs a reason. */
const ALLOWED_EXCEPTIONS: ReadonlyMap<string, string> = new Map();

/** Removes `//` and block comments. String and template contents stay, because a module specifier is a string. */
function stripComments(source: string): string {
  let out = '';
  let quote: string | null = null;
  let i = 0;
  while (i < source.length) {
    const c = source[i];
    const next = source[i + 1];
    if (quote !== null) {
      out += c;
      if (c === '\\' && next !== undefined) {
        out += next;
        i += 2;
        continue;
      }
      if (c === quote) quote = null;
      i += 1;
      continue;
    }
    if (c === '"' || c === "'" || c === '`') {
      quote = c;
      out += c;
      i += 1;
      continue;
    }
    if (c === '/' && next === '/') {
      while (i < source.length && source[i] !== '\n') i += 1;
      continue;
    }
    if (c === '/' && next === '*') {
      const end = source.indexOf('*/', i + 2);
      i = end === -1 ? source.length : end + 2;
      out += ' ';
      continue;
    }
    out += c;
    i += 1;
  }
  return out;
}

/** Every use of an fs module in `source` that is not a named or type-only import from READ_SET. */
function findFsViolations(source: string): string[] {
  const code = stripComments(source);
  const found: string[] = [];
  for (const m of code.matchAll(/\bimport\s+(type\s+)?([^'";]*?)\s*\bfrom\s*(['"])([^'"]+)\3/g)) {
    const [, typeOnly, clause = '', , specifier = ''] = m;
    if (!FS_MODULE.test(specifier) || typeOnly !== undefined) continue;
    const named = /^\{([^}]*)\}$/.exec(clause.trim());
    if (named === null) {
      found.push(`import ${clause.trim()} from '${specifier}'`);
      continue;
    }
    for (const raw of (named[1] ?? '').split(',')) {
      const item = raw.trim();
      if (item === '' || /^type\s/.test(item)) continue;
      const name = (item.split(/\s+as\s+/)[0] ?? '').trim();
      if (!READ_SET.has(name)) found.push(`import { ${name} } from '${specifier}'`);
    }
  }
  const forms: ReadonlyArray<readonly [RegExp, (specifier: string) => string]> = [
    [/\bimport\s*(['"])([^'"]+)\1/g, (s) => `import '${s}'`],
    [/\bexport\s+[^;]*?\bfrom\s*(['"])([^'"]+)\1/g, (s) => `export … from '${s}'`],
    [/\brequire\s*\(\s*(['"])([^'"]+)\1/g, (s) => `require('${s}')`],
    [/\bimport\s*\(\s*(['"])([^'"]+)\1/g, (s) => `import('${s}')`],
  ];
  for (const [pattern, describeForm] of forms) {
    for (const m of code.matchAll(pattern)) {
      const specifier = m[2] ?? '';
      if (FS_MODULE.test(specifier)) found.push(describeForm(specifier));
    }
  }
  if (/\bcreateRequire\b/.test(code)) found.push('createRequire');
  return found;
}

function scannedFiles(): string[] {
  const e2e = readdirSync(E2E_DIR, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile() && SCRIPT_FILE.test(entry.name))
    .map((entry) => path.join(entry.parentPath, entry.name));
  return [...e2e, path.join(APP_DIR, 'playwright.config.ts')];
}

function relative(file: string): string {
  return path.relative(APP_DIR, file).split(path.sep).join('/');
}

describe('findFsViolations', () => {
  const red: ReadonlyArray<readonly [string, string]> = [
    ['a named write import', "import { rmSync } from 'node:fs';"],
    ['an aliased write import', "import { rmSync as remove } from 'fs';"],
    ['a default import', "import fs from 'node:fs';"],
    ['a namespace import', "import * as fs from 'node:fs';"],
    ['a default import beside named reads', "import fs, { existsSync } from 'node:fs';"],
    ['require', "const fs = require('node:fs');"],
    ['a dynamic import', "const fs = await import('node:fs');"],
    ['an fs/promises write import', "import { writeFile } from 'node:fs/promises';"],
    ['a bare fs/promises write import', "import { mkdir } from 'fs/promises';"],
    ['createRequire', "import { createRequire } from 'node:module';\nconst load = createRequire(import.meta.url);"],
    ['a side-effect import', "import 'node:fs';"],
    ['a re-export', "export { rmSync } from 'node:fs';"],
    ['a write import after a string holding //', "const url = 'http://x';\nimport { cpSync } from 'node:fs';"],
  ];
  it.each(red)('reds on %s', (_label, source) => {
    expect(findFsViolations(source)).not.toEqual([]);
  });

  const green: ReadonlyArray<readonly [string, string]> = [
    ['named reads', "import { existsSync, readFileSync } from 'node:fs';"],
    ['an aliased read', "import { statSync as stat } from 'fs';"],
    ['a type-only import', "import type { Stats } from 'node:fs';"],
    ['an inline type beside a read', "import { type Stats, lstatSync } from 'node:fs';"],
    ['commented-out writes', "// import { rmSync } from 'node:fs';\n/* import fs from 'fs'; */"],
    ['a write-named import from another module', "import { rmSync } from './helpers';"],
    ['node:path', "import path from 'node:path';"],
  ];
  it.each(green)('accepts %s', (_label, source) => {
    expect(findFsViolations(source)).toEqual([]);
  });
});

describe('the e2e tree is read-only on every build tree (SMA-655)', () => {
  const files = scannedFiles();

  it('scans the e2e directory, its support directory and playwright.config.ts', () => {
    const scanned = files.map(relative);
    expect(scanned.filter((f) => /^tests\/e2e\/[^/]+$/.test(f)).length).toBeGreaterThan(0);
    expect(scanned.filter((f) => f.startsWith('tests/e2e/support/')).length).toBeGreaterThan(0);
    expect(scanned).toContain('playwright.config.ts');
  });

  it('names only scanned files in ALLOWED_EXCEPTIONS', () => {
    const scanned = new Set(files.map(relative));
    expect([...ALLOWED_EXCEPTIONS.keys()].filter((f) => !scanned.has(f))).toEqual([]);
  });

  it('holds no fs write in any scanned file', () => {
    const violations = files
      .filter((file) => !ALLOWED_EXCEPTIONS.has(relative(file)))
      .flatMap((file) => findFsViolations(readFileSync(file, 'utf8')).map((form) => `${relative(file)}: ${form}`));
    expect(violations).toEqual([]);
  });
});
```

- [ ] **Step 2: Run it and see the real scan red on the current setups**

```bash
cp ts/apps/iam-console/tests/unit/e2e-read-only.test.ts ts/apps/gateway-console/tests/unit/e2e-read-only.test.ts
cd ts/apps/iam-console && pnpm exec vitest run tests/unit/e2e-read-only.test.ts; cd -
cd ts/apps/gateway-console && pnpm exec vitest run tests/unit/e2e-read-only.test.ts; cd -
```

Expected: every `findFsViolations` case passes. The case `holds no fs write in any scanned file` FAILS in both apps and lists `tests/e2e/global-setup.ts: import { rmSync } from 'node:fs'` and `… { cpSync } …`. If it passes, the scan is broken: stop and fix the scan before you continue.

- [ ] **Step 3: Rewrite iam-console's setup** — `ts/apps/iam-console/tests/e2e/global-setup.ts`

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Runs ONCE, in the Playwright runner process, before any worker starts. It does not start servers:
// see playwright.config.ts. The build comes from `iam-console-ts:build` (the Moon task's deps).
//
// This setup only CHECKS the build tree; it never writes into it (SMA-655). The standalone output
// has no .next/static of its own (measured, SMA-510), so `build` stages it (moon.yml). An e2e
// setup must not do that copy: gateway-console's two-zone tier serves from this same tree, Moon
// runs the two tiers at the same time, and a delete-then-copy here wiped the tree under the other
// tier. tests/unit/e2e-read-only.test.ts reds any fs write under tests/e2e/.
import { existsSync } from 'node:fs';
import path from 'node:path';
import { APP_DIR, STANDALONE_APP_DIR } from './support/paths';
import { assertStagedBuild } from './support/staged-build';

export default function globalSetup(): void {
  const serverJs = path.join(STANDALONE_APP_DIR, 'server.js');
  if (!existsSync(serverJs)) {
    throw new Error(`the standalone server is missing at ${serverJs}. Run \`moon run iam-console-ts:build\` first (iam-console-ts:test-e2e depends on it).`);
  }
  assertStagedBuild(APP_DIR, STANDALONE_APP_DIR, 'iam-console-ts:build');
}
```

- [ ] **Step 4: Rewrite gateway-console's setup** — `ts/apps/gateway-console/tests/e2e/global-setup.ts`

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Runs ONCE, in the Playwright runner process, before any worker starts. It does not start servers:
// see playwright.config.ts. The single-zone build comes from `gateway-console-ts:build` (the Moon
// task's deps). The two-zone tier (SMA-512 PR4 task 3) also needs `iam-console`'s own standalone
// build — `gateway-console-ts:test-e2e`'s `deps` names `iam-console-ts:build` directly, in
// `ts/apps/gateway-console/moon.yml`. If that edge is ever missing, run both builds by hand:
//   moon run iam-console-ts:build gateway-console-ts:build
//
// This setup only CHECKS both build trees; it never writes into either (SMA-655). Each `build`
// stages its own standalone tree (moon.yml). iam-console's tree also serves iam-console's own
// tier, Moon runs the two tiers at the same time, and the delete-then-copy this setup used to do
// there wiped the tree under that tier. tests/unit/e2e-read-only.test.ts reds any fs write under
// tests/e2e/.
import { existsSync } from 'node:fs';
import path from 'node:path';
import { APP_DIR, IAM_CONSOLE_APP_DIR, IAM_CONSOLE_STANDALONE_DIR, STANDALONE_APP_DIR } from './support/paths';
import { assertStagedBuild } from './support/staged-build';

function check(moonTask: string, appDir: string, standaloneDir: string): void {
  const serverJs = path.join(standaloneDir, 'server.js');
  if (!existsSync(serverJs)) {
    throw new Error(`the standalone server is missing at ${serverJs}. Run \`moon run ${moonTask}\` first.`);
  }
  assertStagedBuild(appDir, standaloneDir, moonTask);
}

export default function globalSetup(): void {
  check('gateway-console-ts:build', APP_DIR, STANDALONE_APP_DIR);
  check('iam-console-ts:build', IAM_CONSOLE_APP_DIR, IAM_CONSOLE_STANDALONE_DIR);
}
```

- [ ] **Step 5: Run the scan again and see it pass**

```bash
cd ts/apps/iam-console && pnpm exec vitest run tests/unit/e2e-read-only.test.ts; cd -
cd ts/apps/gateway-console && pnpm exec vitest run tests/unit/e2e-read-only.test.ts; cd -
diff ts/apps/iam-console/tests/unit/e2e-read-only.test.ts ts/apps/gateway-console/tests/unit/e2e-read-only.test.ts && echo IDENTICAL
```

Expected: PASS in both apps; `IDENTICAL`.

- [ ] **Step 6: Mutations M1 and M3**

M1: in `ts/apps/gateway-console/tests/e2e/global-setup.ts`, change the import to `import { existsSync, rmSync } from 'node:fs';` and add `rmSync(path.join(IAM_CONSOLE_STANDALONE_DIR, '.next', 'static'), { recursive: true, force: true });` as the first line of `globalSetup`. Run the gateway-console scan. Expected: FAIL naming `tests/e2e/global-setup.ts: import { rmSync } from 'node:fs'`. Restore with the Edit tool.

M1b: add a new file `ts/apps/iam-console/tests/e2e/support/probe.ts` holding `import fs from 'node:fs';` and run the iam-console scan. Expected: FAIL naming `tests/e2e/support/probe.ts`. Delete the file.

M3: in the iam-console scan's `red` table, change the `'a default import'` source to `"import { existsSync } from 'node:fs';"`. Run it. Expected: that case FAILS (proves the fixture is not vacuous). Restore with the Edit tool. Run both scans again: PASS.

- [ ] **Step 7: Run both e2e tiers once, and all checks**

```bash
moon run iam-console-ts:test gateway-console-ts:test iam-console-ts:test-e2e gateway-console-ts:test-e2e
moon run ts:lint ts:fmt iam-console-ts:typecheck gateway-console-ts:typecheck
```

Expected: all PASS. The gateway-console tier needs Docker.

- [ ] **Step 8: Commit**

```bash
git add ts/apps/iam-console/tests/unit/e2e-read-only.test.ts ts/apps/gateway-console/tests/unit/e2e-read-only.test.ts ts/apps/iam-console/tests/e2e/global-setup.ts ts/apps/gateway-console/tests/e2e/global-setup.ts
git commit -m "fix(ts): make the console e2e setups read-only and pin it (SMA-655)" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: CLAUDE.md

**Files:**
- Modify: `CLAUDE.md` (the "A Playwright `globalSetup` runs in another process" entry; a new entry after it)

- [ ] **Step 1: Correct the stale sentence**

In the entry that starts `- **A Playwright \`globalSetup\` runs in another process than the tests.**`, replace:

```
The iam-console e2e tier only
  checks the build and copies `.next/static` in `tests/e2e/global-setup.ts`, and starts the fake IAM,
```

with:

```
The iam-console e2e tier only
  checks the staged build in `tests/e2e/global-setup.ts` (SMA-655: `build` stages `.next/static`), and starts the fake IAM,
```

Keep the rest of the entry unchanged.

- [ ] **Step 2: Add the gotcha directly after that entry**

```markdown
- **An e2e tier must never write into a build tree** (MEASURED, SMA-655). iam-console's standalone
  tree serves two tiers — `iam-console-ts:test-e2e` and gateway-console's two-zone tier — and Moon
  runs them at the same time. When each tier's `global-setup.ts` deleted and re-copied
  `.next/static` there, one tier's delete wiped the tree under the other: `ENOTEMPTY` in one
  setup, and `React never hydrated` in 16 of 17 specs of the other. Each app's `build` script now
  stages the standalone tree (`.next/static`, and `public/` if it exists), and the setups only
  check it through `tests/e2e/support/staged-build.ts`. Two tests pin this in each app's `test`
  task: `tests/unit/e2e-read-only.test.ts` is an allowlist scan that reds any `fs` write form in
  `tests/e2e/**` or `playwright.config.ts` (its `ALLOWED_EXCEPTIONS` ships empty), and
  `tests/standalone-staging.test.ts` reds if `build` stops staging. Both `test` tasks list
  `moon.yml` as an input, because without it a `moon.yml`-only edit selects neither. After a bare
  `pnpm exec next build` the staged tree is gone (Next's `cleanDistDir`), and `moon run
  <app>-ts:build` without `--force` sees an unchanged hash and skips; use `--force`. Residuals:
  the scan does not see a write through `child_process` or through a helper outside
  `tests/e2e/`; a Moon cache-hit restore MERGES into `.next`, so a deleted stable-named `public/`
  file can survive; nothing asserts that a third console app has these tests.
```

- [ ] **Step 3: Check that the ci-targets markers still count 1 each**

```bash
grep -c 'ci-targets:begin' CLAUDE.md; grep -c 'ci-targets:end' CLAUDE.md; grep -c 'ciReport.json' CLAUDE.md
```

Expected: `1`, `1`, and the same `ciReport.json` count as `git show HEAD:CLAUDE.md | grep -c ciReport.json`. The new text must not name `ciReport.json` (check 12 of `repo:actionlint`).

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record that e2e tiers must not write into a build tree (SMA-655)" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: Verification (spec V2 second half, V4, repo gates)

**Files:**
- Modify: `docs/superpowers/specs/2026-09-19-sma-655-e2e-standalone-staging-design.md` (§ 10.2)

- [ ] **Step 1: Run both tiers at the same time, three times**

```bash
for n in 1 2 3; do
  moon run iam-console-ts:test-e2e gateway-console-ts:test-e2e --force > "$SP/after-$n.log" 2>&1
  echo "run $n rc=$?"
done
```

Expected: `rc=0` three times. If a run fails, keep its log, do NOT re-run, and report the log (CLAUDE.md step 0: a re-run destroys the evidence).

- [ ] **Step 2: V4 — no server wrote into its tree**

After the last run:

```bash
for app in iam-console gateway-console; do
  s=ts/apps/$app/.next/standalone/apps/$app
  echo "== $app"; find "$s" -type f -newer "$s/.next/BUILD_ID" -not -path "$s/.next/static/*" -not -path "$s/public/*"
done
```

Expected: no files listed (the staged `static/` and `public/` are the only files newer than `BUILD_ID`). If a file is listed, STOP and report it: spec § 5 says the work returns to design.

- [ ] **Step 3: Run the affected-graph gate with system bash**

This gate needs `/bin/bash` 3.2 on this host, but a `/bin`-first PATH downgrades `python3`. Use a bash-only shim directory:

```bash
mkdir -p "$SP/bash32" && ln -sf /bin/bash "$SP/bash32/bash"
PATH="$SP/bash32:$PATH" /bin/bash ci/affected-graph/run.sh > "$SP/affected-smoke.log" 2>&1; echo rc=$?
```

Expected: `rc=0`. If it fails, read the log. A red `ui->console` case or a selection-set case that now includes an extra `*-console-ts:test` because of the new `moon.yml` input is a real finding: report the case and its expected-vs-actual rows before you change anything.

- [ ] **Step 4: Try one local `moon ci` that selects both tiers**

```bash
moon ci :build :test :test-e2e :lint :fmt :typecheck --base origin/main > "$SP/moon-ci.log" 2>&1; echo rc=$?
```

Record the rc. On failure, follow CLAUDE.md's diagnosis procedure (copy `.moon/cache/ciReport.json` and the failed task's state directory to `$SP` BEFORE anything else re-runs). A failure in a gate that CLAUDE.md records as bash-version or pipe-capacity limited on this host is not a finding: record it as such.

- [ ] **Step 5: Append § 10.2 to the spec**

```markdown
### 10.2 After the change (branch feature/sma-655-e2e-standalone-race)

Same command, three runs.

| Run | rc | ENOTEMPTY | `React never hydrated` |
|---|---|---|---|
| 1 | … | … | … |
| 2 | … | … | … |
| 3 | … | … | … |

V4: files newer than `BUILD_ID` outside the staged paths: … (expected: none).
Mutations: M1 …, M1b …, M2a …, M2b …, M3 …, M4 … (each: red as expected / not).
`ci/affected-graph/run.sh` under /bin/bash 3.2: rc … .
Local `moon ci :build :test :test-e2e :lint :fmt :typecheck`: rc … (and why, if not 0).
```

- [ ] **Step 6: Commit**

```bash
git add docs/superpowers/specs/2026-09-19-sma-655-e2e-standalone-staging-design.md
git commit -m "docs(ts): record the SMA-655 verification" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```
