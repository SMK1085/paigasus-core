# SMA-536: ts `tsc` tasks read fresh binding typings, and a gate holds their upstream inputs

- **Linear:** [SMA-536](https://linear.app/smaschek/issue/SMA-536)
- **Date:** 2026-10-04
- **Status:** Draft, waiting for approval
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

The state of the tree on 2026-10-04 (read from the files, not measured unless marked):

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
   No tsconfig has a `paths` mapping. pnpm hard-links a `file:` dependency at install time,
   and a rename-write of the committed file breaks that link (`ts/CLAUDE.md`).
   **MEASURED on 2026-10-04 in the main checkout:** both store copies have a different inode
   from the committed files, and both differ in content (`index.d.ts` from line 36,
   `paigasus_wasm.d.ts` from line 41). So local `tsc` runs check against stale binding
   typings today. CI installs fresh, so CI reads the current content.
5. `paigasus-auth-ts`, `paigasus-ui-ts` and `paigasus-next-config-ts` also inherit the bare
   `typecheck`. They have no `@paigasus/*` dependency in `package.json`, so the bare task is
   correct for them. (`next-config/src/index.ts` names `@paigasus/kernel` only in a string
   list.)

An input on a file that `tsc` does not read is vacuous. SMA-560 §2.1 rejected the first SMA-536
design for this reason. So this design first makes `tsc` read the committed typings, and then
keys the tasks on them.

## 2. Goals and non-goals

### Goals

- G1. Every library `tsc` run reads the committed binding typings, not the pnpm store copy.
- G2. `paigasus-kernel-ts:typecheck` becomes affected by a change to the binding typings.
- G3. A gate asserts that every ts task that runs `tsc` declares inputs for its whole
  `package.json` dependency closure. New packages and new dependencies are covered with no
  hand-written list.
- G4. `ci/affected-graph/run.sh` sees `typecheck`, and its baselines include the new rows.

### Non-goals

- N1. vitest `test` tasks. vitest resolves the bindings through aliases in
  `vitest.config.ts`, not through tsconfig `paths`, so the required-input rule differs. A
  follow-up Linear issue holds this.
- N2. The `tsc` of the two Next.js apps keeps the store copy locally. Each app defines its own
  `paths` (`@/*`), which replaces the base block. CI installs fresh, so CI is correct.
- N3. A hand-edit to committed glue with no Rust change. `paigasus-kernel-ts:test` already
  gates it (SMA-667 for napi, SMA-634 for wasm).
- N4. Runtime module resolution. No vite tsconfig-paths plugin exists in the repo, and vitest
  and Next.js do not read the new mapping. Runtime behaviour does not change.

## 3. Design

### 3.1 `paths` mapping in `ts/tsconfig.base.json`

Add to `compilerOptions`:

```json
"paths": {
  "@paigasus/node-bindings": ["../rs/crates/bindings/paigasus-node-bindings/index.d.ts"],
  "@paigasus/wasm": ["../rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts"]
}
```

- `paths` without `baseUrl` resolves relative to the tsconfig that declares it. That is
  `ts/`, for every package that extends the base.
- Every library package inherits the block. This covers `paigasus-kernel-ts` and
  `paigasus-console-core-ts`. console-core compiles the kernel `src/wasm.ts`, which imports
  `@paigasus/wasm`.
- The two apps extend `@paigasus/next-config/tsconfig-app` and declare their own `paths`.
  A child `paths` block replaces the parent block, so the apps do not inherit the mapping
  (N2).
- typescript-eslint reads the same tsconfig, so typed lint rules also read fresh typings.

### 3.2 `paigasus-kernel-ts` task inputs

- `typecheck` (new override, `merge` default so it keeps the inherited inputs) adds:
  - `/rs/crates/bindings/paigasus-node-bindings/index.d.ts`
  - `/rs/crates/bindings/paigasus-node-bindings/package.json`
  - `/rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts`
  - `/rs/crates/bindings/paigasus-wasm/package.json`
  - `deps: ['^:build']`, for ordering only. It does not confer affectedness (SMA-528).
- `build` adds the two `.d.ts` files. Its `tsc` now reads them directly.
- The comment at `ts/packages/paigasus-kernel/moon.yml:69-74` says "Do not add the two files
  here". Its reason was that `tsc` read the file through the hard link and the test gate held
  it equal to the generator. With the mapping, `tsc` reads the committed file itself, so the
  comment is replaced with that reason. The committed `index.js` stays out of the inputs:
  `tsc` does not read it.

### 3.3 Assertion A12 in `ci/affected-graph/cargo_moon_parity.py`

A12 follows the shape of A7 (`check_wrapper_upstream_inputs`): a derived task set, a floor,
containment, and both input buckets per task. It reads the live `moon query projects` output
through `moon_projects()`, and `package.json` and `ts/tsconfig.base.json` from disk. `root` is a
required positional argument, never defaulted, for the reason recorded in A7's docstring
(SMA-560 I3).

#### A12a: upstream inputs of every `tsc` task

- **Task set.** Every task of a `language: typescript` project whose resolved command or script
  runs `tsc` as a command word. Today: the inherited `build` and `typecheck`, the kernel
  `build`, and the `typecheck`/`build` overrides listed in §1 item 1.
- **Closure.** The transitive closure over `workspace:` and `file:` specifiers in
  `dependencies`, `devDependencies` and `peerDependencies`. Package names map to directories
  through the `name` field of each `ts/packages/*/package.json` and `ts/apps/*/package.json`,
  and through the path of a `file:` specifier. The closure uses `package.json`, not Moon
  `dependsOn`, because `package.json` is what `tsc` resolves.
- **Required inputs** (workspace-relative, as moon reports them):
  - For a workspace package at directory `D`: `D/src/**/*`, `D/package.json`, and each
    `exports` target that is not under `D/src/`.
  - For a `file:` binding at directory `B`: `B/package.json` and `B/<types>`, where `<types>`
    is the binding's `package.json` `types` field. A binding with no `types` field is a
    violation, never a skip.
  - If the project is `paigasus-proto-ts`, or `@paigasus/proto` is in its closure:
    `contracts:generate` in the task `deps`. The generated tree under
    `ts/packages/paigasus-proto/src/generated/` is an input, so the task must run after
    generation for a deterministic cache key.
- **Check.** Containment per task: `required - (inputFiles ∪ inputGlobs)` must be empty.
  Strict equality is wrong here, because these lists are hand-written and correctly hold other
  entries (own sources, test globs, the parity vectors). A task that reports no input bucket is
  a violation with the message "moon's output shape changed", never a skip.
- **Floor.** Rows are prefixed `FLOOR:`.
  - The derived task set must contain `paigasus-kernel-ts:build`,
    `paigasus-kernel-ts:typecheck`, `paigasus-console-core-ts:typecheck`,
    `iam-console-ts:typecheck` and `gateway-console-ts:typecheck`.
  - The closure of `@paigasus/kernel` must contain `@paigasus/node-bindings` and
    `@paigasus/wasm`. The closure of `@paigasus/console-core` must contain `@paigasus/kernel`.

#### A12b: the `paths` mapping

Every `file:` dependency in any ts `package.json` that resolves under `rs/crates/bindings/`
must have an entry in `ts/tsconfig.base.json` `compilerOptions.paths`. The entry must equal
exactly one path, and that path must resolve to the binding's `types` file. If the base file
does not parse as JSON, A12b reports a violation, never a skip.

Without A12b, a removed mapping would leave A12a green while `tsc` reads the store copy again.
That is the vacuous state SMA-560 rejected.

#### Accepted over-approximation

The closure is per package, not per import. console-core reaches `@paigasus/node-bindings`
through the kernel, but it only imports the wasm entry (`.` export). So A12a demands the napi
`index.d.ts` and `package.json` inputs from console-core and from both apps. The cost: a napi
glue change also runs three `typecheck` tasks. A7 accepts the same kind of cost for `.pyi`
stubs (`cargo_moon_parity.py:2081-2085`).

#### Registration

- `collect_findings` gains the key `a12`. If a key floor tuple exists for the parity findings,
  it is re-baselined in the same commit.
- `self_test()` gains the rows in §4.2.

### 3.4 `ci/affected-graph/run.sh`

- The filter at line 117 adds `typecheck`.
- Every task case that touches ts gains `typecheck` rows. Each baseline is re-measured from
  `moon query tasks --affected` JSON, taking one target per `tasks[project][task]`. A `grep`
  of `"target"` counts scheduled `deps` as selections and is not used.
- New case `wasm-typings->typecheck`: touch
  `rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts`. The expected set holds the
  `typecheck` of the kernel, console-core and both apps, plus each other task that already
  keys on that file. The measured set is the baseline.
- New case `napi-typings->typecheck`: touch
  `rs/crates/bindings/paigasus-node-bindings/index.d.ts`. Same approach.
- The comment at lines 423-425 that says `typecheck` is invisible is updated.

### 3.5 Inputs that A12a will force

A12a may find gaps beyond the kernel. Example: `paigasus-app-shell-ts` reaches
`@paigasus/proto` through discovery, so it needs `contracts:generate` in its `deps`. The plan
measures A12a on the real tree first, and fixes each reported gap in the owning `moon.yml`.

## 4. Verification

### 4.1 Red-first proof

1. **Before** any change: make a hand-edit to the committed napi `index.d.ts` that breaks
   `src/binding-parity.types.ts`. Run `moon run paigasus-kernel-ts:typecheck --force`.
   Expected: PASS, because `tsc` reads the store copy. Record the result. Revert the edit.
2. **After** the change: the same edit makes the task fail. The same edit also makes
   `paigasus-kernel-ts:typecheck` appear in `moon query tasks --affected`.
3. Repeat 1 and 2 with the wasm `paigasus_wasm.d.ts`.

### 4.2 A12 self-test rows

- A clean fixture passes.
- A fixture with one required input removed reds A12a, with the target and the input named.
- A fixture with no `contracts:generate` dep, where proto is in the closure, reds A12a.
- A fixture where no task runs `tsc` reds the floor.
- A fixture with a missing `paths` entry reds A12b. A fixture with a wrong target reds A12b.
- A fixture binding with no `types` field reds A12a.
- A fixture task with no input bucket reds with the "output shape changed" message.

### 4.3 Mutation battery on the real tree

Each mutation is applied alone, and each must red `repo:affected-smoke`:

- Delete each new input line in `ts/packages/paigasus-kernel/moon.yml`.
- Delete one upstream input from `paigasus-console-core-ts:typecheck`.
- Delete each `paths` entry in `ts/tsconfig.base.json`.
- Delete one `contracts:generate` dep from a proto consumer.
- Remove `typecheck` from the `run.sh` filter.

Each mutation must compile and run, so the red comes from the assertion and not from a parse
error. After any fix, the whole battery runs again.

### 4.4 Full graph

Before the push, run the full `moon ci` command from the root `CLAUDE.md` with
`--base origin/main --include-relations`. `repo:affected-smoke` needs system bash 3.2 on the
development Mac (see the root `CLAUDE.md`).

## 5. Documentation

- `ts/CLAUDE.md`: the paragraph "The `build` task runs `tsc`, and `tsc` reads the installed
  copy of `index.d.ts`" changes. Library `tsc` now reads the committed typings through
  `ts/tsconfig.base.json` `paths`. The `rm -rf ts/node_modules && pnpm -C ts install` advice
  stays, for runtime and for the apps.
- `ci/affected-graph/README.md` lists A12 and its limits (the over-approximation, N1, N2).
- `ci/CLAUDE.md` gets one line: a new ts workspace dependency needs matching task inputs, or
  A12 reds `repo:affected-smoke`.

## 6. Risks

- R1. Typed ESLint rules now read fresh typings. If the store copy and the committed file
  differ on a developer host, lint output can change on that host. CI is not affected.
- R2. The re-baselined `run.sh` cases grow. Each new row is measured, not hand-written.
- R3. A12a uses a `tsc` command-word match. A task that runs `tsc` through a wrapper script
  is not matched. The floor catches the loss of a known task, not a new wrapper.

## 7. Follow-up

- A Linear issue for vitest `test` tasks (N1).
