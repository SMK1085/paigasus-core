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

## 2. Facts this design depends on (checked 2026-09-19)

- F1. Moon 2.5.3's resolved task JSON shows no `mutex` option. This does not prove that the
  option is absent, because the JSON leaves out every option that is not set. The decision does
  not depend on F1 (see § 3).
- F2. Both `build` tasks declare `outputs: ['.next/**/*', '!.next/cache/**/*']`. That covers
  `.next/standalone/**`, so a cache hit restores the standalone tree. A cache-hit restore
  MERGES into the existing tree: it does not delete files that are not in the archive. The main
  checkout's `ts/apps/iam-console/.next/static/` holds two BUILD_ID directories, which neither
  `next build` nor `rm -rf .next/static` can leave.
- F3. `next build` deletes everything in `.next` except `cache`, `dev`, `lock` and `trace`
  (`cleanDistDir` defaults to `true`). So after an executed build, `.next/standalone` is fresh
  and holds no `.next/static`.
- F4. Neither `build` task copies `.next/static` into the standalone tree today. Only the two
  `global-setup.ts` files do that.
- F5. In the gateway-console e2e tree, `global-setup.ts` is the only file that writes into
  iam-console's tree. `support/two-zone-harness.ts` only reads it (`spawn` with `cwd`).
- F6. In both apps' `tests/e2e/**`, the two `global-setup.ts` files are the only files that call a
  filesystem write function.
- F7. `ci/tailwind-source/run.mjs` walks only `<app>/.next/static`, and its app walk skips `.next`.
  A copy under `.next/standalone` does not change the guard's result.
- F8. `.next/static` is under 1 MB per app (iam-console 904 KB, gateway-console 816 KB).
- F9. Both apps' `test` tasks depend on `~:build`, list `tests/**/*` as inputs, and use
  `merge: replace`. Neither lists `moon.yml`. So an edit to `moon.yml` alone selects neither
  `test` task. The `test` hash includes the build hash, and the build hash includes `script`.
- F10. `createNextConfig` sets no `generateBuildId`, so each build gets a random BUILD_ID.
  `static/<BUILD_ID>/` holds only the build and SSG manifests, not the chunks.
- F11. No gate knows about `global-setup.ts` or the `test-e2e` staging today.

## 3. Decision

Stage once, in the task that both tiers already depend on: each app's `build`. The e2e tiers
become read-only on every build tree.

Rejected:

- A private copy of iam-console's standalone tree for gateway-console. It copies 44 MB, needs a
  filter for the subtree that the other tier writes, and still reads a tree under modification.
- A `deps` edge from `gateway-console-ts:test-e2e` to `iam-console-ts:test-e2e`. It puts the two
  slowest tiers in series, it schedules the iam-console tier on every gateway-console run, and a
  red iam-console tier blocks the gateway-console tier. It also does not protect a manual
  `pnpm exec playwright test` that runs at the same time as another. Removing the writer is
  better than serializing it, whether or not Moon has a mutex option.

## 4. Design

### 4.1 `build` stages the standalone tree

In `ts/apps/iam-console/moon.yml` and `ts/apps/gateway-console/moon.yml`, the `build` script gets
these steps after the existing `server.js` check (`<app>` is the app directory name, and
`$DEST` is `.next/standalone/apps/<app>`):

1. Delete `$DEST/.next/static` and `$DEST/public`. After an executed build this deletes nothing
   (F3). The step protects against a future `cleanDistDir: false`, and it makes the copy in
   step 2 go to an absent destination.
2. `cp -R .next/static "$DEST/.next/static"`. The destination is absent, so BSD `cp` and GNU `cp`
   give the same result.
3. `if [ -d public ]; then cp -R public "$DEST/public"; fi`. This is an `if` block, not
   `[ -d public ] && …`, because Moon takes the block's status from its last command.
4. Read `.next/BUILD_ID` into a variable. Fail the build with a message on stderr if the value is
   empty, if `$DEST/.next/BUILD_ID` does not hold the same value, or if
   `$DEST/.next/static/<BUILD_ID>` is not a directory.

Shell rules for these lines: the script stays `set -euo pipefail`. No pipe into an early-exit
reader (`head`, `grep -q`, `grep -m`), because `ci/actionlint/run.sh` check 13 scans every
`moon.yml`. No here-string.

The outputs do not change (F2). The script change changes the build hash, so Moon cannot restore
an old archive that has no staged tree.

### 4.2 The e2e setups only check

Each `tests/e2e/global-setup.ts` keeps its `server.js` check and then calls one check function
from a new file, `tests/e2e/support/staged-build.ts`. The function takes the app directory and
the standalone directory, and it throws when:

- `<app>/.next/BUILD_ID`, trimmed, is empty;
- `<standalone>/.next/BUILD_ID`, trimmed, is different from it;
- `<standalone>/.next/static/<BUILD_ID>` is not a directory;
- a file under `<app>/.next/static` is absent from `<standalone>/.next/static`, or has a
  different size there. Extra files in the staged copy are accepted, because a cache-hit restore
  merges (F2).

The error message names `moon run <app>-ts:build --force`. The `--force` is necessary: after a
manual `pnpm exec next build`, the staged tree is gone (F3), but Moon sees the same hash and does
not run `build` again. The gateway-console setup calls the check for both apps. The setups import
only read functions from `node:fs`.

A bare `next build` no longer gives a tree that the e2e tier can run. The setup error states this.

### 4.3 The pin

**Scan: `tests/unit/e2e-read-only.test.ts`, in each app.** It runs in the app's `test` task. It
reads every file with a script extension (`.ts`, `.tsx`, `.mts`, `.cts`, `.js`, `.mjs`, `.cjs`)
under `tests/e2e/**`, and also `playwright.config.ts`. It is an ALLOWLIST, not a denylist:

- The modules in scope are `fs`, `node:fs`, `fs/promises` and `node:fs/promises`.
- From these modules, only a named import from a fixed read set passes: `existsSync`,
  `readFileSync`, `readdirSync`, `statSync` and `lstatSync`. A type-only import passes.
- Every other form of these modules is a red: another named import, a default import, a namespace
  import, `require(...)`, `createRequire`, and a dynamic `import(...)`.
- An `ALLOWED_EXCEPTIONS` table, keyed by file path with a reason, ships EMPTY.

The scan matches on source text with comments removed. String contents are NOT masked, because
the module specifier is a string. It reports each file and each offending form.

Fixture cases prove that the scan reds on a named write import, an aliased import
(`rmSync as remove`), a default import, a namespace import, `require('node:fs')`, a dynamic
`import('node:fs')` and a `fs/promises` import. They also prove that it accepts a read-only named
import and a type-only import, and that a banned form inside a comment does not red. The scan
asserts that it read at least one file from each of `tests/e2e/` and `tests/e2e/support/`, so a
wrong glob cannot pass with nothing scanned.

**Build side: `tests/standalone-staging.test.ts`, in each app.** It sits next to
`tests/standalone-runtime.test.ts`, because both depend on the build output. It calls the same
check function as the setup (§ 4.2) on the real build tree. It reds `test` if a later edit
removes the staging from `build`.

**Selection.** Both apps' `test` tasks add `'moon.yml'` to their `inputs`. Without it, a pull
request that edits only `build`'s script in `moon.yml` selects no task that runs the pin (F9).
The plan verifies this with `moon query tasks --affected` on a `moon.yml`-only change.

Each app has its own copy of these files. This follows the repository pattern: each app owns its
e2e support files.

### 4.4 Documentation

- Update the header comments of both `global-setup.ts` files: the setups check, `build` stages.
- Update the `build` comments in both `moon.yml` files.
- Correct `CLAUDE.md`'s sentence that the iam-console tier "checks the build and copies
  `.next/static` in `tests/e2e/global-setup.ts`".
- Add a CLAUDE.md gotcha: an e2e tier must not write into a build tree, because two tiers can
  share a tree and Moon runs them at the same time; staging belongs to `build`; the
  `e2e-read-only` and `standalone-staging` tests pin it; a manual `next build` needs
  `moon run <app>-ts:build --force` after it.

## 5. Assumption: the standalone servers do not write into their tree

Next's file-system cache can write into `.next/server/app` and `.next/cache/fetch-cache` of the
tree a server runs from. Three servers can run from iam-console's tree at the same time: its own
tier, the two-zone tier, and `standalone-runtime.test.ts`. The app code has no `revalidate =`,
`'use cache'`, `unstable_cache` or `next/image`, but its Server Actions call `revalidatePath`.
The spec assumes that these servers write nothing into their tree. Verification V4 checks it.
If V4 finds a write, the spec returns to design before implementation continues.

## 6. Error handling

- A missing standalone `server.js`: the build fails (unchanged), and the setup throws (unchanged).
- A missing or incomplete staged tree: the build fails, the `standalone-staging` case fails, and
  the setup throws with the `--force` hint.
- A missing `public/`: normal today (neither app has one). The build copies nothing.

## 7. Verification

- V1. Both apps' `test` tasks pass with the new tests.
- V2. Baseline, on the base commit: run
  `moon run iam-console-ts:test-e2e gateway-console-ts:test-e2e --force` three times and record
  how many runs fail. Then run the same command three times on the branch. All three must pass.
  Also run one local `moon ci` that selects both tiers, if the host can run it (CLAUDE.md records
  bash and pipe limits on this host); record the result either way. The gateway-console tier
  needs Docker.
- V3. Mutation checks, each one restored after it runs, and all of them re-run after any fix:
  - M1: add the old `rmSync(...)` line back into one `global-setup.ts`. The scan reds.
  - M2a: remove steps 1–3 from one `build` script and keep step 4. The build fails.
  - M2b: remove steps 1–4 from one `build` script and rebuild. The `standalone-staging` case reds.
  - M3: replace a banned form in one fixture with an allowed form. That fixture case reds, which
    proves the fixture is not vacuous.
  - M4: remove `'moon.yml'` from one `test` task's inputs. `moon query tasks --affected` for a
    `moon.yml`-only change no longer selects that `test` task.
- V4. `.next/BUILD_ID` is written early in `next build`, before the rest of the server tree, so
  "newer than BUILD_ID" cannot distinguish a build's own output from a later write — it catches
  ordinary build output too. Instead: touch a marker file, start both apps' Playwright suites
  directly (no Moon, so no cache restore) at the same time, and wait for both to finish. Then run
  two checks against the marker: a narrow one, every FILE under each app's `.next` (excluding
  `.next/cache`) newer than the marker; and a broad one, every file AND DIRECTORY under each app's
  `.next/standalone` newer than the marker, with no exclusion, so it covers
  `.next/standalone/.../.next/cache/fetch-cache`. Nothing may appear in either. Also check whether
  a `fetch-cache` path exists under either app's `.next` at all, since a 0 count from the broad
  check means different things depending on the answer.

## 8. Out of scope

- `ts/packages/paigasus-app-shell/tests/e2e/global-setup.ts` stages its own fixture in a private
  directory. It has no shared tree and no race.
- A Moon serialization mechanism.
- A shared scan function for both apps. Each app owns its e2e support files; R2 records the cost.

## 9. Residuals

- R1. The scan sees only `fs` module use in `tests/e2e/**` and `playwright.config.ts`. A shell
  command (`child_process`) or a helper module outside that tree can still write into a build
  tree without a red.
- R2. Nothing asserts that the two apps' copies of the tests stay identical.
- R3. Nothing asserts that a third `ts/apps/*` app with an e2e tier has these tests.
- R4. A cache-hit restore merges (F2), so a stable-named `public/` file that a later build deleted
  can survive in the staged tree and be served. Neither app has `public/` today.
- R5. A `build` that runs while a tier serves from the same tree still races, because Next's
  clean deletes `.next/standalone`. Inside one `moon ci` this cannot happen, because both tiers
  depend on the build. It can happen with two concurrent local sessions in one checkout.
- R6. The BUILD_ID and size checks prove which build was staged and that its files are present.
  They do not prove that the content of each file is identical.

## 10. Measurements

### 10.1 Baseline (base commit 8a185402, 2026-09-19)

Command: `moon run iam-console-ts:test-e2e gateway-console-ts:test-e2e --force`, three runs.

| Run | rc | ENOTEMPTY | `React never hydrated` | Failed task |
|---|---|---|---|---|
| 1 | 0 | 0 | 0 | none |
| 2 | 1 | 1 | 0 | gateway-console-ts:test-e2e |
| 3 | 0 | 0 | 0 | none |

The race reproduced once in three runs. In run 2, `gateway-console-ts:test-e2e` failed at
`global-setup.ts:23` with `Error: ENOTEMPTY, Directory not empty` on
`ts/apps/iam-console/.next/standalone/apps/iam-console/.next/static`, while
`iam-console-ts:test-e2e` was running its own setup at the same time. This matches the race this
spec describes: both tiers stage into the shared standalone tree without serialization.

The `failed` column in the raw `grep -cE 'test-e2e.*(failed|FAIL)'` count was 1 in every run,
including the two passing ones. In runs 1 and 3, the match was a passing test's own title
(`R14: a failed grant leaves the account unable to call models, …`), not a failure. Only run 2
carried a true failure, reported by Moon as `Task gateway-console-ts:test-e2e failed to run.`

### 10.2 After the change (branch feature/sma-655-e2e-standalone-race)

Same command, three runs: `moon run iam-console-ts:test-e2e gateway-console-ts:test-e2e --force`.
All three used the real `moon run` invocation end to end; no fallback to a direct Playwright
invocation was needed. `contracts:generate` (an upstream dependency of both apps' `build` tasks)
hit the documented BSR `resource_exhausted` rate limit on runs 2 and 3, each time deleting
`ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts`; each time the file was
restored with the one allowed `git checkout --`, then the run was retried after a wait (5, then a
further 8, then a further 10 minutes for run 3's second retry). The row below is the result of the
retry that reached the real Playwright suites, not of the earlier `contracts:generate`-only
failures, which produced no e2e result at all and are not counted as a run.

| Run | rc | ENOTEMPTY | `React never hydrated` |
|---|---|---|---|
| 1 | 0 | 0 | 0 |
| 2 | 0 | 0 | 0 |
| 3 | 0 | 0 | 0 |

All three runs passed: 17/17 iam-console tests and 22/22 gateway-console tests (single-zone and
two-zone) each time, with no ENOTEMPTY and no hydration failure in any log.

**V4, first attempt (wrong reference file).** After the final run, `find <standalone-app-dir>
-type f -newer .../.next/BUILD_ID -not -path '.../static/*' -not -path '.../public/*'` listed
files: 104 for iam-console, 86 for gateway-console (190 total), none under `static/` or
`public/`. Every one of the 190 sits under `.next/required-server-files.json` or
`.next/server/**` — compiled route bundles (`.js`), route manifests (`.json`), prerendered
`_not-found`/`_global-error`/middleware artifacts (`.html`, `.rsc`, `.meta`), and source maps
(`.map`). Zero of the 190 are under `.next/cache/fetch-cache`.

**Why that reference was wrong.** `.next/BUILD_ID` is written early in `next build`, before the
rest of the server tree is finalized, so "newer than BUILD_ID" cannot tell a build's own output
from a later write — it also catches ordinary build output from the same `next build` invocation.
All 190 files carry the same wall-clock second as BUILD_ID itself, consistent with this: they are
part of the same build pass, not a later write. The `build` task's own script only ever touches
`$dest/.next/static` and `$dest/public`, and the now-read-only `global-setup.ts` files only read
(pinned by `tests/unit/e2e-read-only.test.ts`, which passed in every run above) — neither one
wrote any of the 190 paths. `assertStagedBuild` (`tests/e2e/support/staged-build.ts`), the
invariant that actually gates a release, checks BUILD_ID equality and `static/` file
existence/size, not `.next/server/**` mtimes, and it passed in all three runs. This first attempt
is kept here as a recorded false start, not as the V4 verdict.

**V4, corrected re-check (marker-based, 2026-09-19 19:36–19:37 CEST).** Script: `$SP/v4-check.sh`
(`$SP` = the session scratchpad). Method: touch a marker file (`$SP/v4-marker`, `ls -l` shows
mtime 19:36), wait one second, then run `pnpm exec playwright test` in `ts/apps/iam-console` and
`ts/apps/gateway-console` at the same time (no Moon, so no cache restore can mask a write), and
wait for both — `$SP/v4-iam.log` (mtime 19:36) and `$SP/v4-gw.log` (mtime 19:37) confirm both
tiers finished, `iam rc=0` / `gw rc=0` (`$SP/v4-rc-iam.txt`, `$SP/v4-rc-gw.txt`).

The script itself only ran one check — `find <app>/.next -type f -newer marker -not -path
'*/.next/cache/*'` per app — and its output was not redirected to a file at the time. A second,
broader check (`.next/standalone`, files and directories, no exclusion, so it covers
`.next/standalone/.../.next/cache/fetch-cache`, the specific path spec § 5 names as the runtime
hazard) was also run at the time but its output was likewise not saved. Neither result was
verifiable from disk, so both were re-run on 2026-09-19 19:44 CEST against the same on-disk state
— the trees were not touched between the original run and the re-measurement (only read-only
review checks ran in between), and the marker file still exists — and saved this time:

- Narrow check (the one `v4-check.sh` itself runs): `find ts/apps/iam-console/.next
  ts/apps/gateway-console/.next -type f -newer "$SP/v4-marker" -not -path '*/.next/cache/*'`,
  saved to `$SP/v4-narrow.txt`: **0 lines**.
- Broad check: `find ts/apps/iam-console/.next/standalone ts/apps/gateway-console/.next/standalone
  -newer "$SP/v4-marker"` (files and directories, no exclusion), saved to `$SP/v4-broad.txt`:
  **0 lines**.
- Named-hazard existence check: `find ts/apps/iam-console/.next ts/apps/gateway-console/.next
  -path '*fetch-cache*'`, saved to `$SP/v4-fetch-cache.txt`: **0 lines** — no `fetch-cache` path
  exists anywhere under either app's `.next` tree at all, in the staged tree or the source tree.
  The broad check's 0 is therefore not "an existing fetch-cache directory was untouched"; it is
  "no fetch-cache directory exists to be touched" — a stronger result for the same conclusion.

**V4 verdict: clean.** All three counts are 0, confirmed by the saved artifacts above, and the
finds ran (2026-09-19 19:44 CEST) well after both tiers finished (19:36–19:37 CEST). The servers
wrote nothing into their trees while the tiers ran, and no fetch-cache mechanism exists in either
tree to write into. Spec § 5's assumption holds. The first attempt's 190-file list is superseded,
not a finding to act on.

**Mutations (§ 7 V3).** Reused from task-3-report.md and task-4-report.md (SMA-655
`.superpowers/sdd/2026-09-19-sma-655-e2e-standalone-staging/`), not re-run:

- M1 (task-4-report.md): reds as expected — the read-only scan failed with
  `tests/e2e/global-setup.ts: import { rmSync } from 'node:fs'` after the old write was added back.
- M1b (task-4-report.md): reds as expected — a new file under `tests/e2e/support/` importing
  `node:fs` failed the scan with the matching violation line.
- M2a (task-3-report.md): reds as expected — the `build` script failed with
  `iam-console: staging the standalone tree failed (BUILD_ID '...', staged '...')` when steps 1–3
  were removed and step 4 (the check) was kept.
- M2b (task-3-report.md): reds as expected — `standalone-staging.test.ts` failed with
  `StagedBuildError: .../static/<id> is not a directory` when steps 1–4 were all removed.
- M3 (task-4-report.md): reds as expected — the fixture case `reds on a default import` failed
  when the banned form was replaced with an allowed one, proving the fixture is not vacuous.
- M4 (task-3-report.md): as expected in both directions — with `'moon.yml'` present in `test`'s
  inputs, `moon query tasks --affected` for a `moon.yml`-only edit selected
  `iam-console-ts:test`; with the line removed, the same query did not select it.

`ci/affected-graph/run.sh` under `/bin/bash` 3.2 (via the bash-only shim directory, per the
gotcha in CLAUDE.md): rc=0. Every case, including `ui->console`, reported `PASS`; no red
`ui->console` case and no selection-set case picking up an extra `*-console-ts:test` from the new
`moon.yml` input.

Local `moon ci :build :test :test-e2e :lint :fmt :typecheck --base origin/main`: rc=0. All 12
selected `RunTask` actions passed (`contracts:generate`, both apps' `build`/`typecheck`/`test`/
`test-e2e`, `ts:fmt`, `ts:lint`); `contracts:generate` was a cache hit this time, so this run did
not touch the BSR rate limit. Both apps' `build` tasks were also cache hits, reusing the
standalone trees staged by run 3 above, which is why the V4 first-attempt file list above and this
`moon ci` run describe the same on-disk trees.
