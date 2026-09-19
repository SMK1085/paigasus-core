# SMA-655 — the console e2e tiers must not write into a build tree

Linear: SMA-655. Related: SMA-512 (pull request 4 added the two-zone tier), SMA-651 (the run that
found the race).

## 1. Problem

Two e2e tiers write to the same directory, and each one deletes it first:

- `ts/apps/iam-console/tests/e2e/global-setup.ts:17-18` runs `rmSync` and then `cpSync` on
  `ts/apps/iam-console/.next/standalone/apps/iam-console/.next/static`.
- `ts/apps/gateway-console/tests/e2e/global-setup.ts:39` calls `stage(...)` for iam-console too,
  because the two-zone tier starts the iam-console standalone server. `stage()` runs the same
  `rmSync` and `cpSync` (`:23-24`), and also on `public/` (`:33-34`).

When Moon runs `iam-console-ts:test-e2e` and `gateway-console-ts:test-e2e` at the same time, one
task deletes the tree while the other task writes it or serves from it. The SMA-651 local
`moon ci` run measured both symptoms: `ENOTEMPTY` in the gateway-console setup, and 16 of 17
iam-console specs with `React never hydrated`. Each task passed when it ran alone.

## 2. Facts this design depends on (measured 2026-09-19)

- F1. Moon 2.5.3 has no `mutex` task option. `moon task <id> --json | .options` lists no such key.
  Only a `deps` edge can put two tasks in series.
- F2. Both `build` tasks declare `outputs: ['.next/**/*', '!.next/cache/**/*']`. That covers
  `.next/standalone/**`, so a cache hit restores the standalone tree.
- F3. Neither `build` task copies `.next/static` into the standalone tree today. Only the two
  `global-setup.ts` files do that.
- F4. In the gateway-console e2e tree, `global-setup.ts` is the only file that writes into
  iam-console's tree. `support/two-zone-harness.ts` only reads it (`spawn` with `cwd`).
- F5. In both apps' `tests/e2e/**`, the two `global-setup.ts` files are the only files that call a
  filesystem write function.
- F6. `ci/tailwind-source/run.mjs` walks only `<app>/.next/static`, and its app walk skips `.next`.
  A copy under `.next/standalone` does not change the guard's result.
- F7. `.next/static` is under 1 MB per app (iam-console 904 KB, gateway-console 816 KB).
- F8. Both apps' `test` tasks depend on `~:build` and list `tests/**/*` as inputs.
- F9. No gate knows about `global-setup.ts` or the `test-e2e` staging today.

## 3. Decision

Stage once, in the task that both tiers already depend on: each app's `build`. The e2e tiers
become read-only on every build tree.

Rejected:

- A private copy of iam-console's standalone tree for gateway-console. It copies 44 MB, needs a
  filter for the subtree that the other tier writes, and still reads a tree under modification.
- A `deps` edge from `gateway-console-ts:test-e2e` to `iam-console-ts:test-e2e`. It puts the two
  slowest tiers in series, it schedules the iam-console tier on every gateway-console run, and a
  red iam-console tier blocks the gateway-console tier. It also does not protect a manual
  `pnpm exec playwright test` that runs at the same time as another.

## 4. Design

### 4.1 `build` stages the standalone tree

In `ts/apps/iam-console/moon.yml` and `ts/apps/gateway-console/moon.yml`, the `build` script gets
these steps after the existing `server.js` check (`<app>` is the app directory name):

1. Delete `.next/standalone/apps/<app>/.next/static` and `.next/standalone/apps/<app>/public`.
2. Copy `.next/static` to `.next/standalone/apps/<app>/.next/static`.
3. If `public/` exists, copy it to `.next/standalone/apps/<app>/public`.
4. Read `.next/BUILD_ID`. Fail the build with a message on stderr if
   `.next/standalone/apps/<app>/.next/static/<BUILD_ID>` is not a directory.

Step 1 is necessary because a copy merges into an existing tree. Without it, a file removed from
`public/` stays in the standalone copy.

The script is `set -euo pipefail`, as today. The outputs do not change (F2). The script change
changes the build hash, so Moon cannot restore an old archive that has no staged tree.

### 4.2 The e2e setups only check

Each `tests/e2e/global-setup.ts` keeps its `server.js` check. It also reads `<app>/.next/BUILD_ID`
and throws if `<standalone>/.next/static/<BUILD_ID>` is not a directory. The error message names
`moon run <app>-ts:build`. The gateway-console setup does both checks for both apps. The setups
import no filesystem write function.

### 4.3 The pin

A new unit test in each app, `tests/unit/e2e-read-only.test.ts`, runs in that app's `test` task
(F8). It has two parts.

- **Scan.** It reads every `.ts` file under `tests/e2e/**`. It fails, and names the file, if the
  file imports a write function from `node:fs`, `node:fs/promises`, `fs` or `fs/promises`, or
  calls one through a namespace import (`fs.rmSync(`). The banned names are `rm`, `rmSync`,
  `rmdir`, `rmdirSync`, `cp`, `cpSync`, `copyFile`, `copyFileSync`, `writeFile`,
  `writeFileSync`, `appendFile`, `appendFileSync`, `mkdir`, `mkdirSync`, `mkdtemp`,
  `mkdtempSync`, `rename`, `renameSync`, `unlink`, `unlinkSync`, `symlink`, `symlinkSync`,
  `truncate`, `truncateSync`. Fixture cases prove that the scan detects a named import, an
  aliased import (`rmSync as remove`), a namespace call and the `fs/promises` form, and that it
  accepts a read-only import (`existsSync`, `readFileSync`). The scan also asserts that it read at
  least one file, so a wrong glob cannot pass with nothing scanned.
- **Build side.** It reads the app's `.next/BUILD_ID` and asserts that
  `.next/standalone/apps/<app>/.next/static/<BUILD_ID>` is a directory. This case reds `test` if
  a later edit removes the staging from `build`.

Each app has its own copy. This follows the repository pattern: each app owns its e2e support
files, and a third app repeats the same shape.

### 4.4 Documentation

- Update the header comments of both `global-setup.ts` files: the setups check, `build` stages.
- Update the `build` comments in both `moon.yml` files.
- Add a CLAUDE.md gotcha: an e2e tier must not write into a build tree, because two tiers can
  share a tree and Moon runs them at the same time; staging belongs to `build`; the
  `e2e-read-only` test pins it.

## 5. Error handling

- A missing standalone `server.js`: the build fails (unchanged), and the setup throws (unchanged).
- A missing staged `static/<BUILD_ID>`: the build fails, the `test` case fails, and the setup
  throws with the `moon run <app>-ts:build` hint.
- A missing `public/`: normal today (neither app has one). The build copies nothing.

## 6. Verification

- V1. Both apps' `test` tasks pass with the new unit test.
- V2. A local `moon ci` that selects both `test-e2e` tiers passes both, two times in a row
  (the SMA-655 acceptance). The gateway-console tier needs Docker.
- V3. Mutation checks, each one restored after it runs:
  - M1: add the old `rmSync(...)` line back into one `global-setup.ts`. The scan reds.
  - M2: remove the staging steps from one `build` script and rebuild. The build-side case reds.
  - M3: replace a banned name in a fixture with a read-only name. The fixture case reds, which
    proves the fixture is not vacuous.

## 7. Out of scope

- `ts/packages/paigasus-app-shell/tests/e2e/global-setup.ts` stages its own fixture in a private
  directory. It has no shared tree and no race.
- A general Moon serialization mechanism. Moon 2.5.3 has none (F1).
- The scan is a text scan, not a TypeScript parse. A write through `child_process` (for example
  `spawn('rm', ...)`) or through a helper module outside `tests/e2e/**` is not detected. Section
  8 records this residual.

## 8. Residuals

- R1. The scan sees only direct `fs` writes in `tests/e2e/**`. A shell command or an imported
  helper outside that tree can still write into a build tree without a red.
- R2. Nothing asserts that the two apps' copies of the test stay identical.
