# SMA-536: ts `tsc` tasks prove their binding typings are current, and a gate holds their upstream inputs

- **Linear:** [SMA-536](https://linear.app/smaschek/issue/SMA-536)
- **Date:** 2026-10-04
- **Status:** Revision 2, after one spec-challenger pass. Waiting for approval.
- **Related:** SMA-526 (Rust `lint` propagation), SMA-528 (only `inputs` confer affectedness),
  SMA-560 (cross-stack wrapper inputs, A7), SMA-634 and SMA-667 (committed glue drift gates)

## 1. Problem

The inherited `typecheck` task in `.moon/tasks/typescript-project.yml:38-45` has no inputs
outside its own project. So a change in an upstream project cannot make a ts `typecheck`
affected.

The issue text suggests `deps: ['^:build']`, the fix that SMA-526 used for Rust `lint`. That
fix alone does not work. SMA-528 measured that Moon marks a task affected only when one of its
own `inputs` matches a changed file. `deps` and `--include-relations` only schedule upstream
tasks. They never select a downstream task.

The state of the tree on 2026-10-04 (read from the files, unless marked MEASURED):

1. Most ts projects already declare upstream inputs by hand. `paigasus-console-core-ts`,
   `paigasus-sdk-ts`, `paigasus-discovery-ts`, `paigasus-proto-ts`, `paigasus-app-shell-ts`,
   `iam-console-ts` and `gateway-console-ts` override `typecheck` with `inputs` or
   `deps: ['contracts:generate']`. **No gate checks these lists.** A removed or forgotten
   entry serves a cached PASS.
2. `ci/affected-graph/run.sh:117` filters task names to `build`, `test`, `lint` and
   `test-e2e`. So `typecheck` is invisible to the strict-equality affected-graph cases.
3. `paigasus-kernel-ts:typecheck` inherits the bare definition. It has no `/rs` input, although
   its `tsc` reads both binding typings through `src/binding-parity.types.ts:12-13`.
4. `tsc` resolves `@paigasus/node-bindings` and `@paigasus/wasm` through `ts/node_modules`.
   pnpm installs a `file:` dependency as hard links. A rename-write of a committed file (napi,
   `generate-wasm`, `version-lockstep --write`, most editors, BSD `sed -i`) breaks the link,
   and the installed copy then stays old (`ts/CLAUDE.md`).
   - **MEASURED, main checkout, 2026-10-04:** both installed copies have a different inode
     from the committed files and differ in content (`index.d.ts` from line 36,
     `paigasus_wasm.d.ts` from line 41). So local `tsc` there checks against stale typings,
     with no error.
   - **MEASURED, fresh sma-536 worktree, 2026-10-04:** both installed copies are hard links to
     the committed files (same inode, link count 2, identical bytes).
   - CI installs fresh on every run, so CI reads the current content.
5. `paigasus-auth-ts`, `paigasus-ui-ts` and `paigasus-next-config-ts` also inherit the bare
   `typecheck`. They have no `@paigasus/*` dependency in `package.json`, so the bare task is
   correct for them. (`next-config/src/index.ts` names `@paigasus/kernel` only in a string
   list.)

An input on a file that `tsc` does not read is vacuous. SMA-560 §2.1 rejected the first SMA-536
design for this reason. So this design first makes sure that what `tsc` reads equals the
committed file, and then keys the tasks on the committed file.

### 1.1 Rejected mechanism: a tsconfig `paths` mapping

Revision 1 mapped both bindings to the committed `.d.ts` in `ts/tsconfig.base.json`
`compilerOptions.paths`. **MEASURED on 2026-10-04, then reverted:** `tsc` resolved to the
committed files as intended, but Turbopack (Next 16.3.4) also applied the mapping. It resolved
the kernel's `@paigasus/wasm` import to the `.d.ts`, which has no runtime code. Both
`iam-console-ts:build` and `gateway-console-ts:build` failed the SMA-634 wasm-chunk assertion
(`no paigasus_wasm_bg*.wasm chunk in the standalone tree`). The same builds passed without the
mapping. Playwright 1.63 also applies the nearest `tsconfig.json` `paths` at runtime
(`playwright/lib/common/index.js:953-1018`). Vite 8.3.1 does not (`resolve.tsconfigPaths`
defaults to `false`, `vite/dist/node/index.d.ts:2722-2724`), and the console-core vitest suite
passed with the mapping. A module-resolution change is therefore not safe in this repo.

## 2. Goals and non-goals

### Goals

- G1. A ts `tsc` task never reads binding typings that differ from the committed files. If the
  installed copy is stale, the task fails with the repair command, and does not pass silently.
- G2. `paigasus-kernel-ts:typecheck` becomes affected by a change to the binding typings.
- G3. A gate asserts that every ts task that runs `tsc` declares inputs for its whole
  `package.json` dependency closure, and runs the preflight when its closure holds a binding.
  New packages and new dependencies are covered with no hand-written list.
- G4. `ci/affected-graph/run.sh` sees `typecheck`, and its baselines include the new rows.

### Non-goals

- N1. vitest `test` tasks. vitest resolves the bindings through aliases in
  `vitest.config.ts`, so the required-input rule differs. A follow-up Linear issue holds this.
- N2. Module resolution does not change. `tsc`, Turbopack, Playwright and vitest resolve the
  bindings exactly as today (§1.1).
- N3. A hand-edit to committed glue with no Rust change. `paigasus-kernel-ts:test` already
  gates it (SMA-667 for napi, SMA-634 for wasm).
- N4. The runtime copy of the bindings (`.js` glue, `.wasm`, `.node`). The console vitest
  `setupFiles` already compare the installed wasm copy (SMA-634). The preflight compares
  typings only, because typings are what `tsc` reads.
- N5. `next build`'s own type-check pass in the two app `build` tasks. It is not a `tsc` task.
  Each app's `typecheck` covers the same program.
- N6. Own-package files outside `src/` that a package's `tsconfig.json` includes (for example
  `tests/**/*`), except for the kernel `typecheck`, which this design rewrites (§3.2).
- N7. Relative reads outside a package that `package.json` does not declare, such as
  `next-config/tests/boundaries.test.ts` importing `../../../eslint.config.js`.

## 3. Design

### 3.1 The installed-typings preflight

A new script `ts/scripts/check-installed-bindings.mjs` (Node, no dependencies, SPDX header):

- It finds every `file:` dependency in every `ts/packages/*/package.json` and
  `ts/apps/*/package.json`, in `dependencies`, `devDependencies` and `peerDependencies`.
- For each one, it resolves the installed directory as
  `realpath(<declaring package dir>/node_modules/<name>)`, and the committed directory from the
  `file:` path relative to the declaring package.
- It compares, byte for byte, the binding's `package.json` and every `.d.ts` entry in the
  binding's `files` list (today: `index.d.ts`; `paigasus_wasm.d.ts` and
  `paigasus_wasm_bg.wasm.d.ts`).
- Exit codes:
  - 0: every compared file is equal.
  - 1: a file differs, or the installed copy is missing. The message names each file and
    prints the repair command `rm -rf ts/node_modules && pnpm -C ts install` (the command
    `ts/CLAUDE.md` already gives).
  - 2: an infrastructure error, for example an unparseable `package.json` or a `file:` path
    that does not exist. It never exits 0 on an error.
- It is independent of the calling package. So one invocation with no arguments works from
  any task.

The tsc tasks whose closure holds a binding run it first:
`node <relative path>/ts/scripts/check-installed-bindings.mjs && pnpm exec tsc …`. Today these
are `paigasus-kernel-ts:build` and `:typecheck`, `paigasus-console-core-ts:build` and
`:typecheck`, `iam-console-ts:typecheck` and `gateway-console-ts:typecheck`. Each of them also
lists `/ts/scripts/check-installed-bindings.mjs` as an input.

Why this proves G1: `tsc` reads the installed copy. The preflight runs in the same task, before
`tsc`, and fails unless the installed copy equals the committed copy. So when `tsc` runs, it
reads bytes equal to the committed file, and an input on the committed file is real.

In CI the install is fresh, so the preflight passes. Locally, after a rename-write, the task
reds with the repair command, where today it passes against stale typings.

The script has its own tests (§4.2). The plan chooses the host test task. That task must list
the script and its test as inputs.

### 3.2 `paigasus-kernel-ts` tasks

- `typecheck` (new override; the default merge keeps the inherited inputs) becomes a script:
  the preflight, then `pnpm exec tsc -p tsconfig.json --noEmit`. It adds these inputs:
  - `/rs/crates/bindings/paigasus-node-bindings/index.d.ts`
  - `/rs/crates/bindings/paigasus-node-bindings/package.json`
  - `/rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts`
  - `/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts`
  - `/rs/crates/bindings/paigasus-wasm/package.json`
  - `/ts/scripts/check-installed-bindings.mjs`
  - `tests/**/*` and `vitest.config.ts`, because `tsconfig.json:14` includes them.
- No `deps: ['^:build']`. The issue text asked for it, but here `tsc` reads only committed
  files, and `^:build` would compile two Rust crates before every local `typecheck`. It orders
  nothing and does not confer affectedness (SMA-528).
- `build` adds the same typings inputs and the preflight. Its `tsc` reads the installed copies
  too.
- The comment at `ts/packages/paigasus-kernel/moon.yml:69-74` says "Do not add the two files
  here". It is replaced: the preflight proves that `tsc` reads bytes equal to the committed
  files, so the committed files are inputs. The committed `index.js` stays out of the inputs.
  `tsc` does not read it.
- The run-order warning at `moon.yml:282-283` ("do not run `generate-napi-glue` in one
  `moon run` with `test`") is extended to `build` and `typecheck`, which now hash
  `index.d.ts`.

### 3.3 Assertion A12 in `ci/affected-graph/cargo_moon_parity.py`

A12 follows the shape of A7 (`check_wrapper_upstream_inputs`): a derived task set, a floor,
containment, and both input buckets per task. It reads the live `moon query projects` output
through `moon_projects()`, which already exposes `language`, the joined command/script/args,
the resolved task `deps`, `inputFiles`, `inputGlobs` and `source_dir`
(`cargo_moon_parity.py:2104-2157`). It reads `package.json` files from disk. `root` is a required
positional argument, never defaulted, for the reason in A7's docstring (SMA-560 I3).

#### Task set

Every task of a `language: typescript` project whose resolved command or script contains `tsc`
as a bounded token, with the pattern `(^|[\s;&|(])(pnpm\s+exec\s+)?tsc(\s|$)`. Today: the
inherited `build` and `typecheck`, the kernel `build`, and the overrides listed in §1 item 1.

#### Closure

The transitive closure over `workspace:` and `file:` specifiers in `dependencies`,
`devDependencies` and `peerDependencies`. The walk includes the devDependencies of upstream
packages. That is needed: the apps reach `@paigasus/proto` through console-core's
devDependency, because `tsc` reads `console-core/testing/fake-iam.ts:62-63`.

Fail-closed rules, each a violation and never a skip:

- a `workspace:` name that maps to no package directory;
- a `link:` specifier (not used today; a new one must be handled on purpose);
- a missing or unparseable `package.json`;
- a `file:` path that does not exist. A `file:` path resolves relative to the declaring
  package.

The task's own package is excluded from its closure. A visited set stops cycles.

#### A12a: upstream inputs

Required inputs, as workspace-relative strings in the form moon reports them:

- For a workspace package at directory `D`:
  - `D/src/**/*` and `D/package.json`;
  - for each `exports` target: nothing if it is under `D/src/`; `D/<first segment>/**/*` if it
    is in another subdirectory (for example console-core `./testing/index.ts` gives
    `ts/packages/paigasus-console-core/testing/**/*`); the file itself if it is at the package
    root (for example next-config `./tsconfig.app.json`).
  - `exports` is read recursively. A condition object is walked to its string leaves. A `null`
    leaf is ignored. Any other shape is a violation.
- For a `file:` binding at directory `B`: `B/package.json` and each `.d.ts` entry in `B`'s
  `files` list.
- `/ts/tsconfig.base.json`, for every task in the set.
- `/ts/scripts/check-installed-bindings.mjs`, for every task whose closure holds a `file:`
  binding.
- `contracts:generate` as a **direct** entry in the task's `deps`, if the task's package or a
  closure member is the package named `@paigasus/proto`. The generated tree under
  `ts/packages/paigasus-proto/src/generated/` is an input, so the task must run after
  generation for a deterministic cache key. The package is found by its `name`, not by a
  hard-coded project id. "Direct" matches repo policy (`iam-console/moon.yml:301-305`) and what
  `moon_projects()` records.

Check: containment per task, by exact string match against moon's resolved form:
`required - (inputFiles ∪ inputGlobs)` must be empty. Strict equality is wrong here, because
these lists are hand-written and correctly hold other entries. A task that reports no input
bucket is a violation with the message "moon's output shape changed", never a skip.

#### A12b: the preflight runs before `tsc`

For every task in the set whose closure holds a `file:` binding, the resolved script must
invoke `ts/scripts/check-installed-bindings.mjs` before the first `tsc` token, joined with `&&`.
A missing invocation, an invocation after `tsc`, or an invocation joined with `;` or `||` is a
violation.

#### Floor

Rows are prefixed `FLOOR:`.

- The derived task set must contain `paigasus-kernel-ts:build`,
  `paigasus-kernel-ts:typecheck`, `paigasus-console-core-ts:typecheck`,
  `iam-console-ts:typecheck` and `gateway-console-ts:typecheck`.
- The closure of `@paigasus/kernel` must contain `@paigasus/node-bindings` and
  `@paigasus/wasm`. The closure of `@paigasus/console-core` must contain `@paigasus/kernel`.

#### Registration

- `collect_findings` gains two keys, `a12a` and `a12b`, not one concatenated key. README L1 says
  the key floor proves membership only, so a concatenated call could be deleted silently.
- `EXPECTED_FINDING_KEYS` (`cargo_moon_parity.py:4376`) gains both keys in the same commit.
- The `main()` PASS text (`:4533-4542`) and `ci/affected-graph/README.md:355` ("A1-A11") are
  updated.
- Every file that A12 reads is already an input of `repo:affected-smoke` (`moon.yml:197-217`):
  `.moon/**/*`, `ts/packages/*/moon.yml`, `ts/apps/*/moon.yml`, `ts/packages/*/package.json`,
  `ts/apps/*/package.json` and `rs/crates/*/*/package.json`. The plan verifies this with a
  `moon ci --base` run, not only with `moon run --force` (§4.3).

### 3.4 Rows that A12a will force

Predicted by reading the `moon.yml` files (spec-challenger count, about 26 rows on 10 tasks).
The plan measures A12a on the real tree first and fixes each row in the owning `moon.yml`.

| Task(s) | New requirement | `tsc` reads it? |
|---|---|---|
| `paigasus-sdk-ts`, `paigasus-discovery-ts` `build`/`typecheck` | `ts/packages/paigasus-proto/package.json` | Yes: it holds the `exports` map. |
| `paigasus-kernel-ts` `build`/`typecheck` | binding typings, binding `package.json`, preflight | Yes. |
| `paigasus-console-core-ts` `build`/`typecheck`, both app `typecheck` | napi `index.d.ts` and `package.json`, preflight | No: over-approximation. They reach the napi binding through the kernel but import only the wasm `.` entry. |
| `paigasus-app-shell-ts` `build`/`typecheck` | next-config `src`, `package.json`, `tsconfig.app.json`; proto `src`, `package.json`; `contracts:generate` | No: over-approximation. The app-shell `tsconfig.json:11-12` excludes the fixture that uses next-config, and discovery's `./client` graph does not reach proto (`discovery/src/core/service-of.ts:4`). |

The over-approximations are accepted, as A7 accepts the `.pyi` case
(`cargo_moon_parity.py:2081-2085`). The cost: a napi glue change also runs four more tasks, and
a proto regeneration or a next-config edit also runs app-shell `build` and `typecheck`. A
per-import closure would need a TypeScript resolver in the gate.

### 3.5 `ci/affected-graph/run.sh`

- The filter at line 117 adds `typecheck`. The filter then also admits `py:typecheck`
  (`.moon/tasks/python.yml:31`). No case touches `py/` today; the header comment says this.
- Every case is re-measured, not only those that touch `ts/`. For example,
  `wasm-artifact->console` touches an `rs/` path and gains rows. Each baseline comes from
  `moon query tasks --affected` JSON, one target per `tasks[project][task]`. A `grep` of
  `"target"` counts scheduled `deps` as selections and is not used.
- The existing case `napi-glue-dts->kernel-test` (`run.sh:772-773`) already touches
  `index.d.ts`. It is re-baselined, not duplicated.
- New case `wasm-typings->typecheck`: touch
  `rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts`. The measured set is the baseline. It
  must hold the `typecheck` of the kernel, console-core and both apps.
- The stale "typecheck is invisible" comments are updated: `run.sh:77-83`, `:337`,
  `:423-425`, `:617-619`, `:664`, `:682-683`, and `paigasus-sdk/moon.yml:38-41` and `:48-51`.

## 4. Verification

### 4.1 Red-first proof of G1

The proof must not depend on the hard-link state of the host. So it builds the stale state on
purpose, and measures the resolved path directly.

1. Run `pnpm exec tsc -p tsconfig.json --noEmit --traceResolution` in the kernel and in
   console-core. Record that both bindings resolve to the installed copy under
   `ts/node_modules/.pnpm/`.
2. **Before** the change: replace the installed `paigasus_wasm.d.ts` with a copy (new inode)
   that breaks `src/binding-parity.types.ts`, and leave the committed file unchanged. Run
   `moon run paigasus-kernel-ts:typecheck --force`. Record the result.
3. **After** the change: the same stale state makes the task fail at the preflight, with the
   file named and the repair command printed, before `tsc` runs.
4. Restore with `rm -rf ts/node_modules && pnpm -C ts install`, and confirm the task passes.
5. Repeat 2 to 4 for the napi `index.d.ts`.

### 4.2 Tests

Preflight script:

- equal copies give exit 0;
- one differing `.d.ts` gives exit 1, and the message names the file and the repair command;
- a missing installed copy gives exit 1;
- a differing binding `package.json` gives exit 1;
- an unparseable `package.json` gives exit 2;
- a fixture with no `file:` dependency gives exit 0.

A12 self-test rows:

- A clean fixture passes.
- A required input removed reds A12a, with the target and the input named.
- An `exports` target in a subdirectory is satisfied by `<dir>/**/*`, and reds without it.
- A conditional `exports` object is walked. An unknown `exports` shape reds.
- A missing direct `contracts:generate`, where proto is in the closure, reds A12a.
- No task matches `tsc`: the floor reds.
- A `workspace:` name with no package, a `link:` specifier, a missing `package.json` and an
  unparseable `package.json` each red.
- A missing preflight, a preflight after `tsc`, and a preflight joined with `;` each red A12b.
- A task with no input bucket reds with the "output shape changed" message.

### 4.3 Mutation battery on the real tree

Each mutation is applied alone. Each must red `repo:affected-smoke`. Each must parse and run,
so that the red comes from the assertion and not from a load error (see the memory note "A
mutation must compile to prove anything"). After any fix, the whole battery runs again.

- Delete each new input line in the kernel `typecheck` (a typecheck-only edit).
- Delete one upstream input from a `typecheck` that does not share a YAML alias with `build`.
  (console-core's `typecheck` uses the `*upstreams` alias at `moon.yml:28,55`, so a deletion
  there also changes `build`.)
- Remove the preflight from one task script. Move it after `tsc`.
- Remove `@paigasus/kernel` from console-core's `package.json` (the closure floor).
- Change a binding's `files` list to add a `.d.ts`.
- Delete `/ts/tsconfig.base.json` from the inherited `typecheck` inputs.
- Delete one direct `contracts:generate` dep from a proto consumer.
- Delete `a12a` or `a12b` from `EXPECTED_FINDING_KEYS`.
- Remove `typecheck` from the `run.sh` filter.

At least one mutation (the preflight removal) runs through `moon ci --base origin/main`, not
only `moon run --force`, to prove that the PR that makes it selects `repo:affected-smoke`.

### 4.4 Full graph and forced runs

- Force-run every task whose script or inputs change: the kernel `build`, `typecheck` and
  `test`, console-core `build` and `typecheck`, sdk, discovery and app-shell `build` and
  `typecheck`, and both app `typecheck` and `build` tasks (the SMA-634 wasm-chunk assertion).
- Before the push, run the full `moon ci` command from the root `CLAUDE.md` with
  `--base origin/main --include-relations`. `repo:affected-smoke` needs system bash 3.2 on the
  development Mac (root `CLAUDE.md`).

## 5. Documentation

- `ts/CLAUDE.md`: the paragraph "The `build` task runs `tsc`, and `tsc` reads the installed
  copy of `index.d.ts`" says that the preflight now fails such a task with the repair command,
  where it passed before.
- `ci/affected-graph/README.md` lists A12a and A12b, the over-approximations (§3.4) and the
  limits (N1, N5, N6, N7).
- `ci/CLAUDE.md` gets one line: a new ts workspace dependency needs matching `tsc` task inputs,
  and a new `file:` binding needs the preflight, or A12 reds `repo:affected-smoke`.

## 6. Risks and rollout

- R1. Local developers see a new red after a rename-write of binding typings, where today they
  get a silent stale pass. The message gives the one repair command.
- R2. The re-baselined `run.sh` cases grow. Each new row is measured, not hand-written.
- R3. A12's `tsc` token match does not see `tsc` behind a wrapper script. The floor catches the
  loss of a known task, not a new wrapper.
- R4. Rollout: the first PR changes the hash of every task whose inputs or script change, so
  CI runs them all once. Rollback is a revert of the PR. No data or published artifact changes.

## 7. Follow-up

- A Linear issue for vitest `test` tasks (N1).

## 8. Spec-challenger triage (revision 1 to revision 2)

Verdict on revision 1: APPROVE WITH CHANGES.

Folded in:

- BLOCKER, "`paths` is read by runtime tools": confirmed by measurement (§1.1). The mechanism
  changed to the installed-typings preflight (§3.1), with approval from Sven.
- BLOCKER, "A12b unreachable on the PR that deletes the mapping": the mapping is gone. A12b now
  reads only `moon.yml` and `package.json` files, which are all `repo:affected-smoke` inputs.
  §4.3 adds a `moon ci --base` mutation.
- MAJOR, export-target rule: subdirectory glob rule, root-file rule, condition objects, exact
  string match (§3.3).
- MAJOR, non-deterministic red-first proof: §4.1 now builds the stale state on purpose and
  records the resolved path with `--traceResolution`.
- MAJOR, verification does not run affected tools: §4.4 forces every changed task, including
  both app builds.
- MAJOR, unlisted forced rows: §3.4 lists them and marks each as read or over-approximated.
- MINOR: no `^:build` on the kernel `typecheck`; direct `contracts:generate` found by package
  name; fail-closed closure rules; separate keys `a12a`/`a12b` and the definite key tuple;
  the existing `napi-glue-dts->kernel-test` case; the full list of stale comments; the
  `py:typecheck` note; `/ts/tsconfig.base.json` required; the extended run-order warning; the
  mutation-battery additions; the kernel own-package inputs; the rollout section; the simpler
  alternative (adopted as the mechanism).

Not acted on, with the reason:

- MAJOR, "a package-level `paths` block switches the mapping off": no `paths` mapping exists
  in revision 2.
- MINOR, "map to the binding directory so the `package.json` input is real": no mapping. With
  normal resolution `tsc` reads the binding `package.json` (`types`), so the input is real.
- MINOR, "strict JSON parse of the base tsconfig" and "the mapping hides undeclared
  dependencies": both apply only to the rejected mapping.
- QUESTION, "is the app-shell over-approximation accepted?": accepted in §3.4, and listed for
  Sven's review at the approval gate.
