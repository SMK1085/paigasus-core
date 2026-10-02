# SMA-667: one writer and a drift gate for the committed napi glue

- **Issue:** [SMA-667](https://linear.app/smaschek/issue/SMA-667)
- **Date:** 2026-10-02
- **Status:** challenged once (verdict APPROVE WITH CHANGES), changes folded in; waits for approval
- **Related:** SMA-663 (found the drift), SMA-634 (the wasm drift gate this design copies),
  SMA-693 (in flight, can touch the same `moon.yml`)

An earlier session drafted and challenged a spec for this issue on 2026-09-27. That file is lost:
it was in a session scratchpad and never committed. Its summary survives in a Linear comment on the
issue. This spec starts again from that summary and from new measurements.

## 1. Problem

Two glue files in the napi binding crate are committed:

- `rs/crates/bindings/paigasus-node-bindings/index.js`
- `rs/crates/bindings/paigasus-node-bindings/index.d.ts`

`napi build --platform` writes both files into the crate folder. Three callers run that command in
place: `paigasus-kernel-ts:build`, `paigasus-kernel-ts:test`, and the `--write` mode of
`ci/version-lockstep/run.sh`. The `build` and `build:release` scripts in the crate's
`package.json:34-37` do the same. So the committed copy has no single owner, and every build
overwrites it.

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
drift.

### 2.1 Who reads the committed bytes

The committed glue is a shipped artifact. Three readers use the committed bytes directly, with no
`napi build` before them:

- **The npm package.** `release.yml:1129-1136` runs `npm publish` for `@paigasus/node-bindings`
  from the crate folder of a fresh checkout. The package `files` list is
  `["index.js","index.d.ts"]` (`rs/crates/bindings/paigasus-node-bindings/package.json:30`).
- **The prebuild assembly.** `prebuild.yml:393-421` packs the committed loader and loads it with
  `require`.
- **The container image.** `ts/Dockerfile:37-39` COPYs both files by name.

### 2.2 Gaps

- **G1. No gate.** No check compares the committed glue with the generator output. In CI, `build`
  and `test` overwrite the committed files before any CI reader sees them, so a stale committed copy
  is invisible to CI. It reaches the three readers in 2.1 unchanged.
- **G2. Builds write tracked files.** When the generator moves (a lockfile refresh inside `^3.7.2`)
  or a Rust export changes, every local build dirties the tree. A `git add -A` then commits an
  unrelated glue diff. Or the developer forgets to commit it, and the npm package and the image ship
  stale glue.
- **G3. The wasm glue already has a gate.** `tests/committed-wasm.test.ts` (SMA-634) compares four
  wasm glue files with a fresh build. The `paigasus_wasm_bg.js` change that SMA-663 saw is in scope
  of that gate. It is out of scope here.

## 3. Goals and non-goals

**Goals**

1. Two sanctioned writers of the committed napi glue: a new `paigasus-kernel-ts:generate-napi-glue`
   task, and `version-lockstep --write` (release stamping).
2. `paigasus-kernel-ts:build` and `paigasus-kernel-ts:test` do not write a tracked file.
3. `paigasus-kernel-ts:test` fails when the committed glue is not the output of the locked generator
   for the current Rust source.
4. The failure message names the remedy.
5. A `@napi-rs/cli` bump arrives in its own Dependabot PR, so a glue change never blocks the other
   JS updates.

**Non-goals**

- Pin `@napi-rs/cli` to an exact version. The catalog range `^3.7.2` stays. Goal 5 handles bumps.
- Change the CI-only napi builds in `.github/workflows/prebuild.yml` and `release.yml`. Their output
  is never committed.
- Delete or change the `build` and `build:release` scripts in the crate's `package.json`. They are
  manual tools, not sanctioned writers. They write the same bytes as `generate-napi-glue`. If they
  ever do not, the gate catches it.
- A `repo:*` gate. See section 4.
- A repo-wide "the tree is clean after `moon ci`" check. It would pin goal 2 for every task, not
  only this one. That is a separate issue.
- The wasm glue (G3).
- A "check 4" for the pnpm-installed copy, as the wasm gate has. No runtime consumer loads the
  installed napi copy: vitest aliases `@paigasus/node-bindings` to the crate folder, and the napi
  entry `./napi` is used only by the kernel's own tests.

## 4. Approaches

| | Approach | Cost | Verdict |
|---|---|---|---|
| A | A vitest file in `paigasus-kernel-ts:test`. `test` builds into sibling scratch files, and the vitest file compares them with the committed files. | No second build. No registry work. Same shape as `committed-wasm.test.ts`. | **Chosen.** |
| B | A new `repo:napi-glue-drift` gate with its own napi build in a temporary folder. | A second Rust link per CI run. The full registry list in `ci/CLAUDE.md` (`T=(…)`, the `ci-targets` block, `SELF_SCHEDULED_GATES`, `REQUIRED_REPO_TASKS`, …). | Rejected: more cost, no more coverage. |
| C | A `git diff --exit-code` step in `ci.yml` after the build. | Cheapest. | Rejected: in today's in-place form it compares the generator with itself after the overwrite, so it only detects a stale commit by accident of order. As a general "tree is clean" check it is out of scope (section 3). A local run never fails. |

## 5. Design

### 5.1 The writers

**`paigasus-kernel-ts:generate-napi-glue` (new).** It is the napi analog of `generate-wasm`. It
builds into the scratch files, like `build` and `test`, and then copies them over the committed
files:

```yaml
generate-napi-glue:
  script: 'touch ../../../rs/crates/libs/paigasus-kernel/src/lib.rs ../../../rs/crates/bindings/paigasus-node-bindings/src/lib.rs ../../../rs/crates/bindings/paigasus-wasm/src/lib.rs && pnpm exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings --js index.fresh.js --dts index.fresh.d.ts && cp ../../../rs/crates/bindings/paigasus-node-bindings/index.fresh.js ../../../rs/crates/bindings/paigasus-node-bindings/index.js && cp ../../../rs/crates/bindings/paigasus-node-bindings/index.fresh.d.ts ../../../rs/crates/bindings/paigasus-node-bindings/index.d.ts'
  deps: ['^:build']
  inputs: <see below>
  options:
    runInCI: false
    cache: false
```

- **Why `cp`, not an in-place `napi build`.** napi writes each output to a temporary file and then
  renames it (`@napi-rs/cli` 3.10.4 `dist/cli.js:1054-1081`). A rename replaces the inode. pnpm
  installs the `file:` dependency `@paigasus/node-bindings` as a hard link, and `tsc` resolves it
  through `ts/node_modules` (`tsconfig.base.json` has no `paths`). So after an in-place build,
  `src/binding-parity.types.ts` type-checks against a stale `index.d.ts`. `cp` writes into the
  existing inode, so the hard link stays. The same problem is recorded for the wasm glue in
  `ts/packages/paigasus-console-core/testing/installed-wasm.ts:3-7`. All three tasks also use one
  invocation shape.
- `runInCI: false`: CI never writes the glue, it only compares. In CI, `moon run` of this task does
  nothing (`ci/affected-graph/ci_targets.py:161-162`).
- `cache: false`: the task writes tracked files, and a cached replay writes nothing.
- The literal `napi build` stays in the `script`, not behind a wrapper.
  `ci/affected-graph/cargo_moon_parity.py:130` (`FFI_MARKERS`) finds FFI tasks by the resolved
  command. The task must satisfy A5, A7 and A8 like the other FFI tasks.
- **A8:** the task needs an entry `ALLOW_UNLOCKED_CARGO["paigasus-kernel-ts:generate-napi-glue"]` in
  `cargo_moon_parity.py` (about lines 408-439), with the same reason as the `generate-wasm` entry
  (`:429-434`): `napi build` has no `--locked`.
- **Inputs (A5 + A7).** They are declared although the task is `cache: false`, because the
  assertions read the declaration. The list is the `build` task's Rust and workspace inputs, without
  the wasm-only entries of `generate-wasm` (`scripts/**/*`):
  - `package.json` (the kernel's `@napi-rs/cli` devDependency)
  - `/ts/pnpm-lock.yaml`
  - `/rs/crates/libs/paigasus-kernel/src/**/*`, `/rs/crates/libs/paigasus-kernel/Cargo.toml`
  - `/rs/crates/bindings/paigasus-node-bindings/build.rs`, `…/src/**/*`, `…/Cargo.toml`,
    `…/package.json`
  - `/rs/crates/bindings/paigasus-wasm/src/**/*`, `…/Cargo.toml`, `…/package.json` (the A7
    `dependsOn` closure)
  - `/rs/Cargo.lock`, `/rs/Cargo.toml`, `/rs/rust-toolchain.toml`, `/.prototools`,
    `/rs/.cargo/config.toml`

  The plan confirms this list against A5 and A7 with `ci/affected-graph/run.sh`.
- A comment above the task says: run it after a Rust change to the binding or the kernel, or after a
  `@napi-rs/cli` lockfile change, then commit both files. Run it as its own command, not in the same
  `moon run` as `test` (see 5.4).

**`version-lockstep --write` (unchanged).** `run_write` already runs an in-place `napi build`
(`ci/version-lockstep/run.sh`, about line 1000) to stamp the version guards in `index.js`. It runs in
the release flow (`release.yml:379`, on `ubuntu-latest`), where no `ts/node_modules` hard link
matters. This design does not change it. The gate holds its output to the same standard, because a
release PR runs `paigasus-kernel-ts:test` too.

### 5.2 `build` and `test` stop writing the committed files

Both scripts change one argument list:

```
pnpm exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings \
  --js index.fresh.js --dts index.fresh.d.ts
```

- `--js` and `--dts` are relative to the output folder, which is the crate folder (napi 3.10.4
  `--help`). The fresh files are siblings of the committed files, at the same depth. M5 shows that
  this gives the same bytes. M4 shows that a subfolder does not.
- `--js` and `--dts` are not part of napi's type-def cache fingerprint (`cli.js:10412-10433`), so
  all callers share one type-def folder in `rs/target`. The flags change nothing else that napi
  writes.
- The `.node` file keeps its place. `vitest.config.ts` aliases `@paigasus/node-bindings` to the
  committed `index.js`, which loads that `.node`. So the tests run the committed loader against the
  fresh addon. The committed loader is what the three readers in 2.1 ship.
- `build` does not need the fresh files, but it must not write the committed ones. It gets the same
  two flags. Its declared `outputs` do not change.
- These comments in `moon.yml` become false and change: line 25 ("the .node + glue land in the crate
  dir"), and the `build` input comment that says the glue is "this task's OUTPUT, not an input". The
  new text says that the glue is a committed file with two sanctioned writers (5.1), and that
  `build` and `test` write only the scratch copies.

The crate's own `.gitignore` (`rs/crates/bindings/paigasus-node-bindings/.gitignore`, beside
`*.node`) gets:

```
# SMA-667: the napi build's scratch glue. paigasus-kernel-ts:build, :test and :generate-napi-glue
# write these. tests/committed-napi-glue.test.ts compares them with the committed index.js and
# index.d.ts.
index.fresh.js
index.fresh.d.ts
```

`.gitattributes` gets two lines next to the wasm glue, for the same reason (the gate compares
working-tree bytes, and a CRLF checkout must not red it):

```
rs/crates/bindings/paigasus-node-bindings/index.js text eol=lf linguist-generated=true
rs/crates/bindings/paigasus-node-bindings/index.d.ts text eol=lf linguist-generated=true
```

The comment above them gets the conflict rule: on a merge conflict in either file, take either
side, then run `generate-napi-glue`.

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
- **Check 2 (non-vacuous).** napi writes a `.d.ts` with only `__napiBindingTarget` and no function
  when its type-def folder is missing or empty (`cli.js:10894-10899`). Two such files are equal, so
  check 1 alone could pass on a broken build. Check 2 runs on both the committed and the fresh pair:
  - the set of names in `module.exports.<name> =` lines of `index.js` equals the set of names in
    `export declare (function|const|class) <name>` lines of `index.d.ts`;
  - that set holds `sum` and has more than one member.
- **Check 3 (the wiring pin).** The test reads `ts/packages/paigasus-kernel/moon.yml` as text. It
  asserts that the `build` and `test` scripts each contain
  `--js index.fresh.js --dts index.fresh.d.ts`. This pins goal 2: if a later edit removes the flags
  from `build`, `build` writes the committed files again with no failure anywhere else.

The checks read files only. They do not load `index.fresh.js`.

`test` task inputs get two new entries, so a change to them re-keys the task:

- `/rs/crates/bindings/paigasus-node-bindings/index.js`
- `/rs/crates/bindings/paigasus-node-bindings/index.d.ts`

They are named one by one, not as an `index.*` glob, so the scratch files cannot re-key the task.
This matches the wasm input list. `/ts/pnpm-lock.yaml` is already an input, and it moves with every
`@napi-rs/cli` change under `--frozen-lockfile`. `/ts/pnpm-workspace.yaml` is not added: it adds no
coverage, and it would schedule a full napi and wasm build for every unrelated edit there.

**The affected-graph pin.** A PR that changes only the glue reaches the gate through the two new
input lines alone. `ci/affected-graph/run.sh` gets two `run_task_case_ci` cases, one per file, like
the `wasm-artifact->console` case (`run.sh:729-745`). The plan measures each expected task set; it
holds at least `paigasus-kernel-ts:test`.

### 5.4 Concurrency

Moon runs `build` and `test` at the same time: neither has an edge to the other (`moon.yml:42`,
`:134`). Both write `index.fresh.*`. This is safe, from a read of napi 3.10.4:

- each output is written to a temporary file and renamed (`cli.js:1054-1081`), so no reader sees a
  partial file;
- `postBuild` holds a cross-process lock on the crate folder (`cli.js:10470-10471`, `:1106-1158`);
- both runs write the same bytes.

Two residual risks stay:

- One napi can read a partial type-def file while another `cargo` recompiles. Check 2 detects the
  empty case. The probability of a partial, non-empty read is low.
- `generate-napi-glue` and `test` in one `moon run` have no order. `test` can hash its inputs before
  `generate-napi-glue` writes them, and cache a result under the old hash. So the documentation says:
  run `generate-napi-glue` as its own command, then run `test`.

### 5.5 Dependabot (goal 5)

`.github/dependabot.yml` gets a separate group for `@napi-rs/cli` in the npm ecosystem, and the
`npm-minor-patch` group excludes it. Then a CLI bump arrives in its own PR. If the bump changes the
glue, check 1 fails on that PR only. A person runs `generate-napi-glue` and pushes the result to the
PR branch. The other JS updates are not blocked.

The plan confirms the exact Dependabot syntax (`patterns` and `exclude-patterns` on the group) and
that `repo:actionlint` or another gate does not reject the change. The `ts/CLAUDE.md` rule (5.7)
says that a Dependabot rebase or recreate deletes a pushed glue commit, as in the wasm-bindgen
runbook in `rs/CLAUDE.md`.

### 5.6 Proof that the gate can fail

A green run proves nothing about the gate. The implementation must show each of these, and the PR
body records the results:

- **P1.** With an inert edit in the committed `index.d.ts` (one comment character),
  `moon run paigasus-kernel-ts:test` fails at check 1 and prints the remedy. Restore the file with an
  edit, not with `git checkout --`.
- **P2.** With an inert edit in `index.js` (one comment character), the test fails at check 1, and
  the other node tests still pass.
- **P3.** With the fresh files deleted and vitest run directly (no rebuild), check 0 fails.
- **P4.** After `moon run paigasus-kernel-ts:build` and `moon run paigasus-kernel-ts:test`,
  `git status --short` is empty.
- **P5.** After a deliberate Rust export change (for example a renamed `sum` parameter in the
  binding), `test` fails, then `generate-napi-glue` writes new glue, then `test` passes. Revert the
  Rust change and run `generate-napi-glue` again, and the tree is clean.
- **P6 (deletion proofs).** Each of these makes a check or a case fail:
  - remove `--js index.fresh.js --dts index.fresh.d.ts` from `build`: check 3 fails;
  - remove one of the two new `test` input lines: the matching affected-graph case fails;
  - replace the fresh `index.d.ts` with a file that holds only `__napiBindingTarget`: check 2 fails.
- **P7.** After `generate-napi-glue`, the `ts/node_modules` copy of `index.d.ts` has the new content
  (the hard link survived the `cp`).

### 5.7 Documentation and the images.yml filter

**Decision D2 (revised): the two glue files leave the `images.yml` filter.** The written rule in
`docs/ops/RUNBOOK-containers.md` section 1 (lines 57-66) and in `rs/CLAUDE.md` (about lines
244-252) says: a file that is a Moon task `input` stays off the filter, because a bad edit there
already reds the ordinary `moon ci`. The two files were on the filter only because they were "NOT
inputs anywhere". After 5.3 they are `test` inputs, and the gate reds the ordinary `moon ci` on a
stale copy. So the rule removes them.

- `.github/workflows/images.yml:53-60`: remove the comment and both paths.
- `docs/ops/RUNBOOK-containers.md` section 1: the napi glue moves to the list of files that are
  inputs and stay off the filter, with the reason (`paigasus-kernel-ts:test` input, held by
  `tests/committed-napi-glue.test.ts`).
- `rs/CLAUDE.md` (about lines 244-252): remove the two files from the filter list, and change the
  sentence that says they are "not Moon `inputs` anywhere".
- `ts/CLAUDE.md`: a short rule next to the wasm artifact rule. Run `generate-napi-glue` after a
  binding or kernel export change, or after a `@napi-rs/cli` lockfile change, and commit both files.
  Run it as its own command (5.4). On the Dependabot PR, push the regenerated glue to the PR branch,
  and know that a rebase deletes it (5.5).
- `ci/version-lockstep/README.md:26`: the line says that the napi glue "drifts silently today".
  Change it to name the new gate. Lines 105 and 145 (L1/L2) stay: they say that the `napi-glue`
  reader in `run.sh:222-237` has no fixture of its own, and the new gate does not test that reader.

`ci/CLAUDE.md:248` says that `repo:actionlint` gates workflow `paths:` filters. The plan runs that
gate after the `images.yml` edit.

## 6. Decisions

| ID | Decision | Source |
|---|---|---|
| D1 | Scope: one writer task plus a drift gate. Not "close with no code", not "gate only". | Sven, 2026-10-02 |
| D2 | The two glue paths leave the `images.yml` filter, as the written rule requires (5.7). This replaces the first answer (keep them), which Sven gave before the rule was known. | Sven, 2026-10-02, after the challenge |
| D3 | The gate lives in `paigasus-kernel-ts:test` (approach A). | Sven approved the design, 2026-10-02 |
| D4 | Sibling scratch files `index.fresh.js` and `index.fresh.d.ts`, not a subfolder. | M4, M5 |
| D5 | `version-lockstep --write` keeps its in-place napi build. | 5.1 |
| D6 | `@napi-rs/cli` gets its own Dependabot group (5.5). | Sven, 2026-10-02, after the challenge |
| D7 | `generate-napi-glue` builds to the scratch files and `cp`s them, to keep the pnpm hard link. | Challenge finding, 5.1 |

## 7. Risks

- **R1. Linux output may differ from macOS.** M5 was measured on macOS arm64 only. Evidence for
  identical bytes: the release-pr job runs on `ubuntu-latest` (`release.yml:200`, `:379`), and its
  `#306` and `#334` stamps changed only the version guards relative to the PR 268 glue. If the CLI
  version was the same for those runs, that is a Linux-versus-macOS measurement. The template lists
  every platform, and there is one type-def file, so the sort order cannot differ. The plan checks
  the CLI version of those runs. The first CI run of the new test is the final measurement. If Linux
  differs, the gate is wrong, not the glue: stop and re-design. Do not commit Linux bytes.
- **R2. The release PR.** The release-plz commit alone is self-consistent (old `package.json`, old
  guards). The stamp step (`release.yml:379`) writes `package.json` first and then regenerates.
  `repo:version-lockstep` already fails if the guards are not stamped (`run.sh:36`, `:222-237`). No
  new way is known for a release PR to fail check 1 while R1 holds.
- **R3. A concurrent branch.** The SMA-693 worktree can touch `ts/packages/paigasus-kernel/moon.yml`.
  A merge conflict is likely but mechanical.
- **R4. Affected-graph assertions.** The new FFI task, the A8 entry, the new inputs and the two new
  cases can red `repo:affected-smoke`. The plan runs `ci/affected-graph/run.sh` under system bash
  3.2.
- **R5. Future host dependence.** Two later changes would make the glue depend on the host: a WASI
  target in `napi.targets` (the output then reads untracked `.wasi.cjs` files, `cli.js:10206-10222`),
  and a `#[cfg]`-gated `#[napi]` item. Either one makes check 1 fail in CI. The `ts/CLAUDE.md` rule
  names both.

## 8. Testing

- The new vitest file (5.3) is the gate and its own test.
- P1 to P7 (5.6) prove that it fails for the right reasons.
- The two affected-graph cases (5.3) pin the input lines.
- The full gate graph from the root `CLAUDE.md` runs before the push, with the bash rules in that
  file.

## 9. Challenge log (2026-10-02)

Verdict: APPROVE WITH CHANGES, no BLOCKER.

| Finding | Severity | Action |
|---|---|---|
| G1 misses the npm publish and prebuild readers | MAJOR | Folded in: 2.1, G1, G2. |
| `@napi-rs/cli` bumps block the grouped Dependabot PR | MAJOR | Folded in: goal 5, 5.5, D6 (Sven chose a separate group). |
| Nothing pins the new `test` inputs | MAJOR | Folded in: two affected-graph cases (5.3), P6. |
| D2 contradicts the written filter rule; RUNBOOK not listed | MAJOR | Folded in: D2 reversed by Sven, 5.7 lists all three documents. |
| A8 `ALLOW_UNLOCKED_CARGO` entry not named; input list vague | MINOR | Folded in: 5.1. |
| No concurrency analysis | MINOR | Folded in: 5.4. |
| In-place `generate` breaks the pnpm hard link | MINOR | Folded in: `cp` form, D7, P7. |
| "Exactly two writers" conflicts with the `package.json` scripts | MINOR | Folded in: "two sanctioned writers", non-goal text. |
| Goal 2 and the gate wiring are not pinned | MINOR | Partly folded in: check 3 pins the `build`/`test` flags, P6. Not folded in: a pin on the vitest `include` list. Reason: no cheap host for it; the `include` list is next to the other kernel tests, and a removal shows as a missing test file in the diff. Not folded in: approach C as a repo-wide check. Reason: separate scope (section 3). |
| No `.gitattributes` entry | MINOR | Folded in: 5.2. |
| version-lockstep README edit claims too much | MINOR | Folded in: only line 26 changes. |
| `ts/pnpm-workspace.yaml` input adds cost, no coverage | MINOR | Folded in: input removed. |
| Crate `.gitignore`; stronger check 2; inert P2 edit; `moon.yml:25`; no check 4 | MINOR | All folded in (5.2, 5.3, 5.6, section 3). |
| R1 and R2 are better answered than stated; WASI and `#[cfg]` risks | MINOR | Folded in: R1, R2, R5. |
