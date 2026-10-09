# SMA-736 — A13: vitest tasks must key on their upstream inputs

- Linear: SMA-736 (follow-up of SMA-536, non-goal N1)
- Status: Draft, revision 2 (after the adversarial challenge, see section 10)
- Date: 2026-10-09
- Gate: `repo:affected-smoke` (`ci/affected-graph/cargo_moon_parity.py`)

## 1. Problem

SMA-536 added assertion A12. A12 checks that every ts task that runs `tsc` declares inputs for
its whole `package.json` dependency closure. A12 selects tasks with `TSC_TOKEN_RE`. A vitest task
runs `pnpm exec vitest run …`, so A12 never examines it.

vitest also reads upstream files. If a vitest task omits an upstream input, two things go wrong:

1. Moon serves a cached PASS after an upstream edit.
2. `moon ci` does not select the task on an upstream edit, because in Moon 2.5.3 only task
   `inputs` confer affectedness (ci/CLAUDE.md).

Nothing reds today. Examples in the current tree (found by reading `moon.yml`, to be confirmed by
the first run of A13):

- `paigasus-sdk-ts:test` and `paigasus-discovery-ts:test` do not list
  `ts/packages/paigasus-proto/package.json`.
- `paigasus-console-core-ts:test`, `iam-console-ts:test` and `gateway-console-ts:test` have
  `@paigasus/kernel` in their closure. The kernel depends on `@paigasus/node-bindings`. These tasks
  do not list the napi `index.js` or `package.json`.
- `paigasus-app-shell-ts:test` does not list the `@paigasus/next-config` or `@paigasus/proto`
  inputs that its typecheck lists, and it has no `contracts:generate` dep.
- The inherited `test` task (`.moon/tasks/typescript-project.yml`) does not list `tsconfig.json`
  or `/ts/tsconfig.base.json`, but vitest reads them (section 2, item 3).

## 2. What vitest reads, and how it differs from `tsc`

1. **Bindings.** `tsc` reads only the `.d.ts` files of a `file:` binding. vitest runs the JS glue
   and the `.wasm`, and never reads a `.d.ts`. So the vitest rule needs every `files` entry that is
   not a `.d.ts`.
2. **Aliases.** A vitest config can alias a package to another path. The kernel config aliases
   `@paigasus/node-bindings` to the committed
   `rs/crates/bindings/paigasus-node-bindings/index.js`, and `@paigasus/wasm` to
   `rs/crates/bindings/paigasus-wasm/.wasmpack-test-out/paigasus_wasm.js`. The second path is a
   scratch output that the task builds in its own script. Git does not track it.
3. **tsconfig.** vite's oxc transform loads the NEAREST `tsconfig.json` of every file that it
   transforms (vite 8.3.2 `transformWithOxc`; measured in SMA-511, recorded in
   `ts/apps/iam-console/vitest.config.ts:21-28`). That tsconfig and its `extends` chain change the
   output: for example `jsx` in `ts/packages/paigasus-ui/tsconfig.json`, and
   `verbatimModuleSyntax` in `ts/tsconfig.base.json`. Only the two app configs stop the lookup
   with `oxc: { tsconfig: false }`. vitest does not type-check, so the installed-typings preflight
   script is not read.
4. **Config lookup.** vitest 5.0.3 finds its config file in this order when there is no
   `--config` flag: `vitest.config.{ts,mts,cts,js,mjs,cjs}`, then `vite.config.{ts,mts,cts,js,mjs,cjs}`,
   in the `--root` directory or else the working directory. If no file exists, vitest runs with
   defaults. Two real tasks do that: `paigasus-proto-ts:test` and `commitlint-config-ts:test`.
5. **Installed copies.** Without an alias, vitest loads a `file:` binding from its pnpm-installed
   copy, not from the committed files. In CI, `pnpm install` runs on a fresh checkout, so the
   installed copy equals the committed files and an input on the committed file keys the task
   correctly. Locally, a git operation can replace a committed file and break the pnpm hard link
   (rs/CLAUDE.md). That is a local residual (non-goal N7).

## 3. Decisions

- **D1 — scope.** A13 examines every vitest invocation. Today that is twelve `test` tasks (ten
  with a config file, plus `paigasus-proto-ts:test` and `commitlint-config-ts:test`) and three
  `test-e2e` tasks (auth, discovery, console-core). `test-e2e` has `cache: false`, so it cannot
  serve a cached PASS, but a missing input still stops `moon ci` from selecting it.
- **D2 — alias source.** A static parse in Python, fail closed (approach A). The gate does not
  run Node and does not need `ts/node_modules`. Rejected: evaluating each config with Node (it
  needs the ts dependencies in the gate job, and it is slower); a JSON alias sidecar (it changes
  product test config, and the gate still needs a scan for inline aliases).
- **D3 — scratch alias.** An alias whose target git does not track is valid only on a task that
  A5 derives as an FFI task (`derive_ffi_tasks`), AND whose invocation names the target's
  directory as an `--out-dir` value. Such a task builds the scratch output in its own invocation,
  and A5 and A7 already assert the inputs of that build.
- **D4 — a new key.** A13 is a separate findings key `a13`, not a widening of A12. The required
  set differs, and a separate key gives a separate title and Fix text.
- **D5 — containment.** As A12a and A7: per task, `want ⊆ inputFiles ∪ inputGlobs`, exact
  strings in moon's resolved, slash-free form. Over-declaration is permitted. A declared glob that
  covers a wanted literal path does not satisfy it; the literal must be declared. This matches A12
  and keeps the comparison exact.
- **D6 — package granularity.** The required set is per package in the closure, not per imported
  file. A13 cannot see which upstream files a test imports (non-goal N2). So A13 over-approximates:
  for example, the console tasks must key on the napi glue, although no console test loads napi.
  Section 8, R2, states the cost.

## 4. Design

### 4.1 Task selection

`derive_vitest_tasks(projects)` returns every `<pid>:<task>` of a `language: typescript` project
whose resolved invocation matches:

```python
VITEST_TOKEN_RE = re.compile(r"(^|[\s;&|(])(pnpm\s+exec\s+)?vitest(\s|$)")
```

The boundaries are those of `TSC_TOKEN_RE`, so `vitest.config.ts`, `vitest-environment` and
`@vitest/…` do not match. A task whose invocation is `None` raises `MoonOutputError` (rc 2), as
in `derive_tsc_tasks`.

### 4.2 Config files

For each match of `VITEST_TOKEN_RE`, the gate reads the words after the token up to the next
`&&`, `||`, `;`, `|` or newline.

- `--config <f>`, `--config=<f>` and `-c <f>` name the config. The path is relative to the task's
  source directory and is normalized (`./x` becomes `x`; `..` segments are resolved). A path
  that does not exist is a row: `<target> runs vitest with <path>, which does not exist`.
- With no flag, the gate applies vitest's own lookup (section 2, item 4) in the task's source
  directory. The first file that exists is the config. If none exists, the task has no config;
  that is not a row.
- These are rows, never skips: `--root` or `-r` in the span; a `cd` anywhere in the invocation
  before the vitest call; a config path that holds `$`, a glob character or a quote.

One task can have more than one config (a script with two vitest calls). A13 unions the
required sets.

### 4.3 Required set

For a task `T` with source directory `own`, config files `C` and closure `K =
ts_closure(root, own, names, rows)` (A12's walk, unchanged), `want(T)` is:

1. **Base.** `ts/pnpm-lock.yaml`. Every vitest task reads installed packages.
2. **Configs.** Each config file in `C`, as a repository-relative path.
3. **Workspace packages.** For each workspace package `D` in `K`: `D/package.json`,
   `D/src/**/*`, and each `exports` target outside `src/` (`D/<head>/**/*` for a target in a
   subdirectory, `D/<file>` for a root file). This is the A12 rule. It moves into a shared helper,
   `workspace_package_inputs`, that A12 and A13 both call, so the two rules cannot drift.
4. **Bindings.** For each `file:` binding `B` in `K`: `B/package.json` and `B/<entry>` for each
   string entry in `files` that does not end in `.d.ts`. Entries are normalized (`./index.js`
   becomes `index.js`). These are rows: an absent `files` key; a `files` value that is not a list;
   an entry that is not a string; an entry that holds a glob character; an entry that is a
   directory.
5. **tsconfig.** Unless every config in `C` sets `tsconfig: false` (section 4.4, step 7), and for
   the own package and for each workspace package `D` in `K`: `<pkg>/tsconfig.json` and every file
   in its `extends` chain. A relative `extends` resolves against the file's directory. Any other
   `extends` form (a package specifier, an array) is a row, never a skip. A missing
   `tsconfig.json` adds nothing; vite then uses the next tsconfig up the tree, which is outside
   the packages and is not required. The current tree gives `ts/tsconfig.base.json` through every
   package's `extends`.
6. **Aliases.** For each alias in a config of `C` whose target is outside `own`:
   - If the key starts with `@paigasus/`, it must be in `K` or be the own package name. Else a
     row: `<target> aliases <key> in <config>, but <key> is not in its package.json closure`.
   - If git tracks the resolved target, `want` gains that path.
   - If git does not track it, D3 applies. Else a row: `<target> aliases <key> to the untracked
     path <path>, but the task does not build it there (no FFI build with --out-dir <dir>)`.
   - A target outside the repository root is a row.
   - An alias whose target is inside `own` adds nothing (non-goal N1).
7. **Proto dep.** If the own package is `@paigasus/proto`, or `@paigasus/proto` is in `K`, `T`
   must have a direct `contracts:generate` dep. This is A12a's rule and row text.

Not in `want`: `ts/scripts/check-installed-bindings.mjs` and the `.d.ts` entries of a binding.
vitest does not read them.

### 4.4 Config parser

`vitest_config_facts(text, config_rel, rows)` returns the aliases as a list of `(key,
target_rel)` pairs, with the target relative to the repository root, and a flag `tsconfig_off`.

1. **Scanner.** One pass, with one state at a time: code; `'…'`; `"…"`; `` `…` `` with escapes
   and nested `${…}` (which returns to code state until the matching `}`); `// …`; `/* … */`.
   Comment state has priority: a quote inside a comment does not open a string. The scanner
   removes the comments and keeps the strings. This is the class of defect that ts/CLAUDE.md
   records for `stripComments` (SMA-639), so the self-test covers each state.
2. **Imports.** A relative `import` (`from './…'` or `from '../…'`) is a row: A13 cannot read a
   config that is split across files. `mergeConfig` or `extends` in a config is a row for the same
   reason. Imports of packages (`vitest/config`, `node:url`, `vite-plugin-wasm`) are permitted.
3. **Bindings.** Collect `const <id> = '<lit>'` (or `"<lit>"`) and
   `const <id> = fileURLToPath(new URL('<lit>', import.meta.url))`. The second form resolves
   `<lit>` relative to the config's directory.
4. **Alias properties.** Find each `alias` property: the word `alias` or `'alias'` or `"alias"`
   as a key, then `:`. Its value must start with `{`; read up to the matching `}`. These are
   rows: a value that does not start with `{`; a shorthand `alias` with no `:`.
5. **Entries.** Split the object on top-level commas. An empty last entry (a trailing comma) is
   skipped. Each other entry is `<key>: <value>`, where `<key>` is a quoted string or a bare
   identifier. A spread (`...x`) or a computed key is a row.
6. **Values.** The value must be a string literal that is a relative path, or an identifier from
   step 3. A string literal that is not a path (a bare package name) is kept only for keys that do
   not start with `@paigasus/`, and it adds nothing. Any other value for a `@paigasus/` key is a
   row: `<config>: the alias <key> has a value A13 cannot resolve`.
7. **`tsconfig: false`.** `tsconfig_off` is true when the code contains `tsconfig: false` (as a
   whole property, after step 1).
8. **Projects.** A string entry in a `projects` array is a row: vitest loads it as another config
   or a glob, which A13 does not follow.

The parser is deliberately narrow. Anything outside this grammar that can carry an alias is a
row, never a skip.

### 4.5 Tracked-file check

`collect_findings(projects, crates, root, tracked)` gains a `tracked` argument, and
`check_ts_vitest_inputs` takes it too. In production, `main()` builds it once with
`task_inputs.tracked_files(root)`, which raises `MoonOutputError` (rc 2) on an empty set. The git
call clears `GIT_DIR` and `GIT_INDEX_FILE` from its environment. The self-test passes a fixed set.
The arity self-test, which runs on a temporary directory that is not a git repository, passes a
fixed set too.

### 4.6 Gate reachability check

A13 also reports a row for each config file that it reads and that no input glob of
`repo:affected-smoke` matches. The globs come from the resolved `inputGlobs` of that task in
`projects`. So a config with a new name or in a new place cannot hide from the gate.

### 4.7 Floors

A13 asserts containment. A containment check whose `want` set empties passes and asserts
nothing (A7's lesson). Three floors guard against that:

- `REQUIRED_VITEST_TASKS`: `paigasus-kernel-ts:test`, `paigasus-console-core-ts:test`,
  `iam-console-ts:test`, `gateway-console-ts:test`, `paigasus-auth-ts:test-e2e`. A floor task
  that A13 does not derive is a row.
- `REQUIRED_VITEST_ALIASES = {"paigasus-kernel-ts:test": {"@paigasus/node-bindings",
  "@paigasus/wasm"}}`. A floor alias that the parser does not return is a row.
- **Real-corpus pin.** A self-test row parses every tracked `vitest*.config.*` file and compares
  the result with a literal table of `(config, sorted aliases, tsconfig_off)`. A parser change
  that alters what it finds in a real config reds the self-test.

### 4.8 Function and row shape

```python
def check_ts_vitest_inputs(projects, root, tracked, floor=REQUIRED_VITEST_TASKS,
                           alias_floor=REQUIRED_VITEST_ALIASES):
```

`root` and `tracked` are positional and required, for the reason in A7's docstring (SMA-560 I3).
Row order: floor rows, then walk and parser rows, then per-task rows, all deduplicated with
`dict.fromkeys`. Per-task omission rows read `<target> inputs omit <entry>`. A task that reports
no `inputFiles` or `inputGlobs` bucket is a row, as in A12a.

**Walk rows.** A12a already reports the package.json walk rows of the `tsc` tasks. A13 reports
only the walk rows that A12a does not, so one broken manifest prints under one title.

### 4.9 Registration

- `EXPECTED_FINDING_KEYS` gains `"a13"` after `"a12b"`. `collect_findings` gains the `a13` tuple
  in the same position. Its title ends with a `Fix:` sentence that names the input or dep to add
  and points at `ci/affected-graph/README.md`.
- Update the PASS sentence in `main()`, the header comment of `cargo_moon_parity.py`, the
  self-test message that counts the assertions (`cargo_moon_parity.py:5055`), and the count
  comment at `ci/actionlint/run.sh:2117`.

### 4.10 Reachability of the gate

`repo:affected-smoke` (root `moon.yml`) does not list the vitest configs today. A13 reads them
from disk, so an edit to a config alone would not schedule the gate. Add:

- `ts/packages/*/vitest*.config.*`
- `ts/apps/*/vitest*.config.*`
- `ts/packages/*/tsconfig*.json`, `ts/apps/*/tsconfig*.json` and `ts/tsconfig.base.json`, because
  section 4.3 item 5 reads the `extends` chains.

Add the same globs to `T_AFFECTED_SMOKE_REQUIRED_INPUTS` in `ci/actionlint/run.sh`, so that
removing them reds `repo:actionlint`. `repo:input-liveness` checks that each glob matches tracked
files; all do.

### 4.11 Input and dep fixes

Run A13 on the real tree. Fix every row by adding inputs or deps in the owning `moon.yml`, or in
`.moon/tasks/typescript-project.yml` for the inherited `test` task. Add no product code. If a row
looks wrong rather than missing, stop and ask.

### 4.12 `ci/affected-graph/run.sh` re-baseline

New inputs change the set of tasks that an edit selects. Re-measure every affected case with the
no-flag command quoted in its comment, and extend each expected CSV with the newly selected
`:test` and `:test-e2e` rows. Do not derive the sets; measure them.

The existing cases `napi-glue-js->kernel-test` and `napi-glue-dts->kernel-test` change. Rename
`napi-glue-js->kernel-test` to `napi-glue-js->vitest` and re-baseline it; it is the control case
for A13. Add no duplicate case.

On the development Mac, `run.sh` needs system bash 3.2. Run it from a bash-only shim directory
(memory: affected-smoke hang).

## 5. Tests

`self_test()` gains A13 rows in the A12 style: copy a fixture, break one thing, and assert one
exact row string.

- **T1** The clean fixture passes.
- **T2** One mutation for each part of `want`: `ts/pnpm-lock.yaml`; the config file; an upstream
  `src/**/*` glob; an upstream `package.json`; an `exports` target outside `src/`; a non-`.d.ts`
  `files` entry of a binding (the `.wasm`); an upstream `tsconfig.json`; `ts/tsconfig.base.json`
  through `extends`; a tracked alias target outside `own`.
- **T3** An untracked alias target: on a task that is not an FFI task (row); on an FFI task
  without the matching `--out-dir` (row); on an FFI task with it (passes).
- **T4** An alias to a `@paigasus/` package that is not in the closure (row). An alias whose
  target is inside `own` (no requirement).
- **T5** Parser: an alias value that is not an object; a shorthand `alias`; a quoted `'alias'`
  key (read); an unresolvable `@paigasus/*` value; a spread entry; a trailing comma (no row); a
  commented-out alias in `//` and in `/* */` form (ignored); `//` inside a string (not a
  comment); an apostrophe, a double quote and a backtick inside each comment form (no state
  error); a template literal with an escaped backtick and `${…}`; a relative import (row);
  `mergeConfig` (row); a string entry in `projects` (row); `tsconfig: false` detection; a
  `fileURLToPath(new URL(…))` binding resolves correctly.
- **T6** Config lookup: `--config f`, `--config=f`, `-c f`, `--config ./f` (normalized); two
  vitest calls in one script; a missing explicit config (row); no flag and no file (no row, no
  config); no flag with only `vitest.config.mts` (that file is selected); `--root` (row); `cd`
  before vitest (row); a config path with `$` (row).
- **T7** The proto dep; the task floor; the alias floor; a task with no input bucket; a `None`
  invocation (rc 2); a non-typescript decoy that is ignored.
- **T8** Bindings: an absent `files` key; `files` that is not a list; an entry with a glob
  character; a `.d.ts` entry (not required); a `./`-prefixed entry (normalized).
- **T9** tsconfig: an `extends` package specifier (row); an `extends` array (row); `tsconfig:
  false` in every config (no tsconfig requirement).
- **T10** The gate reachability check: a config that no affected-smoke input glob matches (row).
- **T11** The real-corpus pin (section 4.7).
- **T12** The shared helper: A12a's existing self-test rows still pass after the move to
  `workspace_package_inputs`. A broken manifest that A12a reports does not print again under
  `a13`.

**Red-first proof.** Each new row must fail before its code exists. In addition, with the `a13`
tuple deleted from `collect_findings`, the arity self-test must red. With
`check_ts_vitest_inputs` reduced to `return []`, T2 must red. Record both measurements in the
plan.

`--negative-control` needs no new case: it already runs `--self-test`.

## 6. Documents

- `ci/affected-graph/README.md`: an A13 bullet after A12. Replace the "vitest `test` tasks are out
  of scope" text (lines 312–313) with a pointer to A13.
- `ci/CLAUDE.md`: extend the "new ts workspace dependency" rule to vitest tasks.
- The SMA-536 spec stays as written. It is the record of that issue.

## 7. Non-goals

- **N1** Own-package files of a task other than its config and `tsconfig.json` (`tests/**`,
  `setupFiles`, in-package aliases). The inherited `@group(tests)` and the hand-written lists
  cover them.
- **N2** Per-file import analysis. A13 works per package (D6).
- **N3** A vitest call behind a wrapper script. The task floor catches the loss of a known task,
  not a new wrapper.
- **N4** Playwright and `next build`.
- **N5** The inputs of a scratch alias's build. A5 and A7 own them (D3).
- **N6** Proving that each required input changes the result of a test. The rule is "does this
  file influence the output", as for A10.
- **N7** A stale pnpm-installed copy of a binding on a developer host (section 2, item 5). CI
  installs fresh. The console `setupFiles` already compare the installed wasm with the committed
  files (SMA-634); `console-core:test-e2e` does not. An A12b-style preflight for vitest tasks is
  out of scope.

## 8. Risks

- **R1** The static parser can miss an alias form that a future config uses. Mitigation: any
  form outside the grammar is a row, the alias floor reds if the kernel aliases disappear, and the
  real-corpus pin reds on any change in what the parser finds.
- **R2** Over-approximation cost (D6). The new inputs make some edits select more tasks. The
  known cases: an edit to the napi glue (`index.js`, `package.json`) selects
  `paigasus-console-core-ts:test`, `iam-console-ts:test` and `gateway-console-ts:test`; the two
  app tests pull in `~:build` and `repo:next-env-drift`. An edit to an upstream package's source
  selects `test-e2e` tasks that today list fewer upstreams; for example
  `paigasus-console-core-ts:test-e2e` (Docker) would key on auth, sdk and proto. The napi glue
  changes only with a kernel API change, which already edits `ts/packages/paigasus-kernel/src`
  and so already selects these tasks. The `test-e2e` cost is new. The plan measures the extra
  selections with the `run.sh` re-baseline.
- **R3** The gate now calls git. A failure is rc 2, never a false green.

## 9. Open question for approval

- **Q1** Accept the `test-e2e` over-approximation (R2), or narrow it? The alternative is a
  reviewed exception table that lets a `test-e2e` task omit named closure members, with a reason
  for each entry. The recommendation is to accept: the table needs per-file knowledge that A13
  cannot check (N2), so an entry can go stale and nothing reds.

## 10. Changes after the adversarial challenge (revision 2)

Folded in:

- The default config follows vitest's own lookup; no config file is not a row (was a BLOCKER: two
  real tasks would red with rows that no input can fix). D1 now counts twelve `test` tasks.
- tsconfig files and their `extends` chain are required, unless every config sets
  `tsconfig: false`. Section 2 item 3 corrects the false premise of revision 1.
- The parser is a specified single-pass scanner. Shorthand alias, relative imports,
  `mergeConfig`, `extends`, and string `projects` entries are rows. A real-corpus pin is added.
- An absent `files` key is a row; `.d.ts` entries are not required; entries are normalized.
- D3 also requires the `--out-dir` match.
- The existing `napi-glue-js->kernel-test` case is renamed and re-baselined, not duplicated.
- Section 4.11 covers deps too (app-shell needs `contracts:generate`).
- `tracked` plumbing reuses `task_inputs.tracked_files`.
- Walk rows print once, under `a12a`.
- `ts/pnpm-lock.yaml` is required.
- Path normalization, `--root` and `cd` rows, trailing comma, non-`@paigasus/` aliases outside
  `own`, the reachability row, and the two count updates.
- Section 2 item 5 and N7 record the installed-copy residual.

Rejected: none. Section 9 asks one question for approval (the cost of R2).
