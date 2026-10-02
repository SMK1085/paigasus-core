# SMA-667: one writer and a drift gate for the committed napi glue

- **Issue:** [SMA-667](https://linear.app/smaschek/issue/SMA-667)
- **Date:** 2026-10-02
- **Status:** draft, for the spec challenge
- **Related:** SMA-663 (found the drift), SMA-634 (the wasm drift gate this design copies),
  SMA-693 (in flight, can touch the same `moon.yml`)

An earlier session drafted and challenged a spec for this issue on 2026-09-27. That file is lost:
it was in a session scratchpad and never committed. Its summary survives in a Linear comment on the
issue. This spec starts again from that summary and from new measurements.

## 1. Problem

Two glue files in the napi binding crate are committed:

- `rs/crates/bindings/paigasus-node-bindings/index.js`
- `rs/crates/bindings/paigasus-node-bindings/index.d.ts`

`napi build --platform` writes both files into the crate folder. Three moon tasks run that command
in place: `paigasus-kernel-ts:build`, `paigasus-kernel-ts:test`, and the `--write` mode of
`ci/version-lockstep/run.sh`. So the committed copy has no single owner, and every build overwrites
it.

SMA-663 saw this as a dirty tree: on 2026-09-21, each napi build rewrote both files.

## 2. Measurements (2026-10-02, this branch, HEAD `27df30c6`)

| ID | What | Result |
|---|---|---|
| M1 | `pnpm exec napi --version` in `ts/packages/paigasus-kernel` | `3.10.4`. The issue says 3.10.3. The catalog range is `^3.7.2` (`ts/pnpm-workspace.yaml:169`). |
| M2 | `napi build --platform --cwd …/paigasus-node-bindings`, then `git status --short` | rc 0, **no change**. The committed glue matches the locked generator now. |
| M3 | `git log` on the two files | `bd4e90e6` (PR 268) regenerated them: `index.js` 301 lines, `index.d.ts` +9 lines (the `__napiBindingTarget` export). This is the diff SMA-663 saw. Releases `#306` and `#334` changed only the version guards in `index.js`. |
| M4 | The same build with `--js .napi-fresh/index.js --dts .napi-fresh/index.d.ts` | `index.d.ts` identical. `index.js` **differs** in two relative paths (`./paigasus-node-bindings.wasi.cjs` becomes `../…`). A scratch subfolder does not work. |
| M5 | The same build with `--js index.fresh.js --dts index.fresh.d.ts` (sibling files) | Both files **byte-identical** to the committed glue. The committed `index.js` mtime did not change, so the build did not write it. The `.node` file stays in the crate folder. |

M2 means that there is no drift to repair today. The defect is that nothing prevents the next
drift. These are the gaps:

- **G1. No gate.** No check compares the committed glue with the generator output. In CI, `build`
  and `test` overwrite the committed files before any reader sees them. Only the `ts/Dockerfile`
  COPY reads the committed bytes directly (`images.yml:53-60`).
- **G2. Builds write tracked files.** When the generator moves (a lockfile refresh inside `^3.7.2`)
  or a Rust export changes, every local build dirties the tree. A `git add -A` then commits an
  unrelated glue diff, or the developer forgets to commit it and the Docker image ships stale glue.
- **G3. The wasm glue already has a gate.** `tests/committed-wasm.test.ts` (SMA-634) compares four
  wasm glue files with a fresh build. The `paigasus_wasm_bg.js` change that SMA-663 saw is in scope
  of that gate. It is out of scope here.

## 3. Goals and non-goals

**Goals**

1. Exactly two writers of the committed napi glue: a new `paigasus-kernel-ts:generate-napi-glue`
   task, and `version-lockstep --write` (release stamping).
2. `paigasus-kernel-ts:build` and `paigasus-kernel-ts:test` do not write a tracked file.
3. `paigasus-kernel-ts:test` fails when the committed glue is not the output of the locked generator
   for the current Rust source.
4. The failure message names the remedy.

**Non-goals**

- Pin `@napi-rs/cli` to an exact version. The gate catches a generator move, and the remedy is one
  command. The catalog range `^3.7.2` stays.
- Change the CI-only napi builds in `.github/workflows/prebuild.yml` and `release.yml`. Their output
  is never committed.
- Change the `build` and `build:release` scripts in
  `rs/crates/bindings/paigasus-node-bindings/package.json`. They write the same bytes as
  `generate-napi-glue`. If they ever do not, the gate catches it.
- A `repo:*` gate. See section 4.
- The wasm glue (G3).

## 4. Approaches

| | Approach | Cost | Verdict |
|---|---|---|---|
| A | A vitest file in `paigasus-kernel-ts:test`. `test` builds into sibling scratch files, and the vitest file compares them with the committed files. | No second build. No registry work. Same shape as `committed-wasm.test.ts`. | **Chosen.** |
| B | A new `repo:napi-glue-drift` gate with its own napi build in a temporary folder. | A second Rust link per CI run. The full registry list in `ci/CLAUDE.md` (`T=(…)`, the `ci-targets` block, `SELF_SCHEDULED_GATES`, `REQUIRED_REPO_TASKS`, …). | Rejected: more cost, no more coverage. |
| C | A `git diff --exit-code` step in `ci.yml` after the build. | Cheapest. | Rejected: it needs the in-place write that goal 2 removes, and a local run never fails. |

## 5. Design

### 5.1 The writers

**`paigasus-kernel-ts:generate-napi-glue` (new).** It is the napi analog of `generate-wasm`:

```yaml
generate-napi-glue:
  script: 'touch <the three lib.rs files, as in build> && pnpm exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings'
  deps: ['^:build']
  inputs: <the A5 and A7 set — the same list generate-wasm declares>
  options:
    runInCI: false
    cache: false
```

- `runInCI: false`: CI never writes the glue, it only compares.
- `cache: false`: the task writes tracked files, and a cached replay writes nothing.
- The literal `napi build` stays in the `script`, not behind a wrapper.
  `ci/affected-graph/cargo_moon_parity.py` finds FFI tasks by the resolved command
  (`FFI_MARKERS`), and the task must satisfy A5, A7 and A8 like the other FFI tasks.
- A comment above the task says: run it after a Rust change to the binding or the kernel, or after a
  `@napi-rs/cli` lockfile change, then commit both files.

**`version-lockstep --write` (unchanged).** `run_write` already runs an in-place `napi build`
(`ci/version-lockstep/run.sh`, about line 1000) to stamp the version guards in `index.js`. That is a
legitimate writer: it runs in the release flow, where the package version changes. This design does
not change it. The gate holds its output to the same standard, because a release PR runs
`paigasus-kernel-ts:test` too.

### 5.2 `build` and `test` stop writing the committed files

Both scripts change one argument list:

```
pnpm exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings \
  --js index.fresh.js --dts index.fresh.d.ts
```

- `--js` and `--dts` are relative to the output folder, which is the crate folder (napi 3.10.4
  `--help`). The fresh files are siblings of the committed files, at the same depth. M5 shows that
  this gives the same bytes. M4 shows that a subfolder does not.
- The `.node` file keeps its place. `vitest.config.ts` aliases `@paigasus/node-bindings` to the
  committed `index.js`, which loads that `.node`. So the tests run the committed loader against the
  fresh addon, which is what ships.
- `build` does not need the fresh files, but it must not write the committed ones. It gets the same
  two flags. Its declared `outputs` do not change.
- The comments on `build` and `test` in `moon.yml` that say the glue is "this task's OUTPUT" become
  false. They change to say that the glue is a committed file with two writers (5.1) and that the
  task writes only the scratch copies.

The root `.gitignore` gets:

```
# SMA-667: the napi build's scratch glue. paigasus-kernel-ts:build and :test write these, and
# tests/committed-napi-glue.test.ts compares them with the committed index.js and index.d.ts.
rs/crates/bindings/paigasus-node-bindings/index.fresh.js
rs/crates/bindings/paigasus-node-bindings/index.fresh.d.ts
```

### 5.3 The gate

A new file: `ts/packages/paigasus-kernel/tests/committed-napi-glue.test.ts`. It goes into the
`node` project's `include` list in `vitest.config.ts`.

- **Check 0.** Both fresh files exist. If not, the test fails with "run
  `moon run paigasus-kernel-ts:test`, which builds them". A missing file is a failure, never a skip.
- **Check 1.** For each pair (`index.js`/`index.fresh.js`, `index.d.ts`/`index.fresh.d.ts`), the
  committed bytes equal the fresh bytes. The test compares UTF-8 text, so vitest shows a readable
  diff. The message names the remedy:
  `Run \`moon run paigasus-kernel-ts:generate-napi-glue\` and commit index.js and index.d.ts under
  rs/crates/bindings/paigasus-node-bindings/.`
- **Check 2 (non-vacuous).** Each compared file is not empty and holds a known marker:
  `index.d.ts` holds `export declare function sum`, and `index.js` holds `module.exports.sum`. Two
  empty files are equal, so check 1 alone could pass on a broken build.

The checks read files only. They do not load the fresh `index.fresh.js`.

`test` task inputs get three new entries, so a change to them re-keys the task:

- `/rs/crates/bindings/paigasus-node-bindings/index.js`
- `/rs/crates/bindings/paigasus-node-bindings/index.d.ts`
- `/ts/pnpm-workspace.yaml` (the `@napi-rs/cli` catalog range)

They are named one by one, not as an `index.*` glob, so the scratch files cannot re-key the task.
This matches the wasm input list. `/ts/pnpm-lock.yaml` is already an input.

### 5.4 Proof that the gate can fail

A green run proves nothing about the gate. The implementation must show each of these, and the PR
body records the results:

- **P1.** With a one-character edit in the committed `index.d.ts`, `moon run paigasus-kernel-ts:test`
  fails at check 1 and prints the remedy. Restore the file with an edit, not with `git checkout --`.
- **P2.** With the same kind of edit in `index.js`, the test fails at check 1.
- **P3.** With the fresh files deleted and vitest run directly (no rebuild), check 0 fails.
- **P4.** After `moon run paigasus-kernel-ts:build` and `moon run paigasus-kernel-ts:test`,
  `git status --short` is empty.
- **P5.** After a deliberate Rust export change (for example a renamed `sum` parameter in the
  binding), `test` fails, then `generate-napi-glue` writes new glue, then `test` passes. Revert the
  Rust change and run `generate-napi-glue` again, and the tree is clean.

### 5.5 Documentation

- `.github/workflows/images.yml:53-58`: the comment says that the build "regenerates them fresh
  every run". That becomes false. The new comment says: `ts/Dockerfile` COPYs the committed files by
  name, `generate-napi-glue` and `version-lockstep --write` are their writers, and
  `tests/committed-napi-glue.test.ts` holds them to the generator. Both paths stay in the filter
  (decision Q1, 2026-10-02).
- `ts/CLAUDE.md`: a short rule next to the wasm artifact rule. Run `generate-napi-glue` after a
  binding or kernel export change, or after a `@napi-rs/cli` lockfile change, and commit both files.
- `ci/version-lockstep/README.md`: lines 26, 105 and 145 say that the napi glue "drifts silently
  today". Change them to name the new gate.
- `rs/CLAUDE.md:248` and `:258` mention the `images.yml` filter for the two files. Check the text
  and correct it if it repeats the "regenerates fresh" claim.

## 6. Decisions

| ID | Decision | Source |
|---|---|---|
| D1 | Scope: one writer task plus a drift gate. Not "close with no code", not "gate only". | Sven, 2026-10-02 |
| D2 | Q1: the two glue paths stay in the `images.yml` filter; the comment is rewritten. | Sven, 2026-10-02 |
| D3 | The gate lives in `paigasus-kernel-ts:test` (approach A). | Sven approved the design, 2026-10-02 |
| D4 | Sibling scratch files `index.fresh.js` and `index.fresh.d.ts`, not a subfolder. | M4, M5 |
| D5 | `version-lockstep --write` keeps its in-place napi build. | 5.1 |

## 7. Risks

- **R1. Linux output may differ from macOS.** M5 was measured on macOS arm64 only. The generated
  `index.js` lists every platform, so I expect identical bytes on Linux CI. This is not measured.
  The first CI run of the new test is the measurement. If Linux differs, the gate is wrong, not the
  glue: stop and re-design. Do not commit Linux bytes.
- **R2. The release PR.** A release-plz PR changes the package version. If the release flow does not
  run `version-lockstep --write`, the committed version guards stay old, and check 1 fails on the
  release PR. `#334` shows that the release flow does stamp `index.js` today: `release.yml:379` runs
  `bash ci/version-lockstep/run.sh --write`. The plan must confirm that this step commits its output
  to the release PR branch before that PR's CI runs.
- **R3. A concurrent branch.** The SMA-693 worktree can touch `ts/packages/paigasus-kernel/moon.yml`.
  A merge conflict is likely but mechanical.
- **R4. Affected-graph assertions.** New inputs and a new FFI task can red `repo:affected-smoke`
  (A5, A7, A8, `inputFiles`). The plan must run `ci/affected-graph/run.sh` under system bash 3.2.

## 8. Testing

- The new vitest file (5.3) is the gate and its own test.
- P1 to P5 (5.4) prove that it fails for the right reasons.
- The full gate graph from the root `CLAUDE.md` runs before the push, with the bash rules in that file.
