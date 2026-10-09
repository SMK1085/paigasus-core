# SMA-736 — A13: vitest tasks must key on their upstream inputs

- Linear: SMA-736 (follow-up of SMA-536, non-goal N1)
- Status: Draft, revision 1
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
  do not list the napi `index.js`, `index.d.ts` or `package.json`.
- `paigasus-app-shell-ts:test` does not list the `@paigasus/next-config` or `@paigasus/proto`
  inputs that its typecheck lists.

## 2. Why A12 does not cover it

The `tsc` rule and the vitest rule differ in three places:

1. **Bindings.** `tsc` reads only the `.d.ts` files of a `file:` binding. vitest runs the JS glue
   and the `.wasm`. So vitest needs every entry of the binding's `files` list.
2. **Aliases.** A `vitest.config.ts` can alias a package to another path. The kernel config
   aliases `@paigasus/node-bindings` to the committed
   `rs/crates/bindings/paigasus-node-bindings/index.js`, and `@paigasus/wasm` to
   `rs/crates/bindings/paigasus-wasm/.wasmpack-test-out/paigasus_wasm.js`. The second path is a
   scratch output that the task builds in its own script. Git does not track it.
3. **Compiler config.** `tsc` reads `ts/tsconfig.base.json`. vitest strips types and does not
   type-check. The console configs also set `oxc.tsconfig: false`. So A13 does not require the
   base tsconfig or the installed-typings preflight script.

## 3. Decisions

- **D1 — scope.** A13 examines every vitest invocation: the ten `test` tasks and the three
  `test-e2e` tasks (auth, discovery, console-core). `test-e2e` has `cache: false`, so it cannot
  serve a cached PASS, but a missing input still stops `moon ci` from selecting it.
- **D2 — alias source.** A static parse in Python, fail closed (approach A). The gate does not
  run Node and does not need `ts/node_modules`. Rejected: evaluating each config with Node (it
  needs the ts dependencies in the gate job, and it is slower); a JSON alias sidecar (it changes
  product test config, and the gate still needs a scan for inline aliases).
- **D3 — scratch alias.** An alias whose target git does not track is valid only on a task that
  A5 derives as an FFI task (`derive_ffi_tasks`, `FFI_MARKERS`). Such a task builds the scratch
  output in its own invocation, and A5 and A7 already assert the inputs of that build. No new
  table.
- **D4 — a new key.** A13 is a separate findings key `a13`, not a widening of A12. The required
  set differs, and a separate key gives a separate title and Fix text.
- **D5 — containment.** As A12a and A7: per task, `want ⊆ inputFiles ∪ inputGlobs`, exact
  strings in moon's resolved, slash-free form. Over-declaration is permitted.

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

For each match of `VITEST_TOKEN_RE` in the invocation, the gate reads the words after the token
up to the next `&&`, `||`, `;`, `|` or newline. In that span it reads `--config <f>`,
`--config=<f>` or `-c <f>`. If there is no flag, the config is `vitest.config.ts`. The path is
relative to the task's source directory. One task can have more than one config (a script with
two vitest calls); A13 unions the required sets.

A config file that does not exist is a row: `<target> runs vitest with <path>, which does not
exist`.

A config path that holds a shell variable (`$`) or a glob character is a row, never a skip.

### 4.3 Required set

For a task `T` with source directory `own` and config files `C`, `want(T)` is:

1. Each config file in `C`, as `<own>/<config>`.
2. For each member of `ts_closure(root, own, names, rows)` (A12's walk, unchanged):
   - **Workspace package** `D`: `D/package.json`, `D/src/**/*`, and each `exports` target outside
     `src/` (`D/<head>/**/*` for a target in a subdirectory, `D/<file>` for a root file). This is
     the A12 rule. It moves into a shared helper, `workspace_package_inputs`, that A12 and A13
     both call, so the two rules cannot drift.
   - **`file:` binding** `B`: `B/package.json` and `B/<entry>` for **every** string entry in
     `files`. A `files` value that is not a list, or an entry that is not a string, or an entry
     that holds a glob character, is a row.
3. For each `@paigasus/*` alias in a config of `C` (section 4.4):
   - The alias key must be in the closure of `T`, or be the own package name. Else a row:
     `<target> aliases <key> in <config>, but <key> is not in its package.json closure`.
   - If git tracks the resolved target path, `want` gains that path.
   - If git does not track it, `T` must be in `derive_ffi_tasks(projects)`. Else a row:
     `<target> aliases <key> to the untracked path <path>, but the task does not build it (it is
     not an FFI task, see A5)`.
   - A target outside the repository root is a row.
4. **Proto dep.** If the own package is `@paigasus/proto`, or `@paigasus/proto` is in the
   closure, `T` must have a direct `contracts:generate` dep. This is A12a's rule and row text.

Not in `want`: `ts/tsconfig.base.json`, `ts/scripts/check-installed-bindings.mjs`,
`ts/pnpm-lock.yaml`, and aliases whose key is not `@paigasus/*` (`server-only`, `next/headers`,
`next/cache`). Those point at in-package test doubles or at npm packages.

### 4.4 Alias parser

`vitest_aliases(text, config_rel, rows)` returns a list of `(key, target_rel)` pairs, with the
target relative to the repository root.

1. **Comments.** Remove `//` line comments and `/* */` block comments. A small scanner skips the
   content of `'…'`, `"…"` and `` `…` `` literals, so `//` inside a string is kept.
2. **Bindings.** Collect `const <id> = '<lit>'` (or `"<lit>"`) and
   `const <id> = fileURLToPath(new URL('<lit>', import.meta.url))`. The second form resolves
   `<lit>` relative to the config's directory.
3. **Alias objects.** Find each `alias` property (`alias:` as a word). Its value must start with
   `{`. Read up to the matching `}`. A value that does not start with `{` is a row:
   `<config>: an alias value is not an object literal, so A13 cannot read it`.
4. **Entries.** Split the object on top-level commas. Each entry is `<key>: <value>`, where
   `<key>` is a quoted string or a bare identifier. A spread (`...x`) or a computed key is a row.
5. **Values.** For a key that starts with `@paigasus/`, the value must be a string literal or an
   identifier from step 2. Else a row: `<config>: the alias <key> has a value A13 cannot
   resolve`. A string literal that is a relative path resolves relative to the config's
   directory. A bare string literal that is not a path (no `.` or `/` prefix) is a row.
6. Keys that do not start with `@paigasus/` are ignored after step 4.

The parser is deliberately narrow. Anything outside this grammar that names `@paigasus/` in an
alias object is a row, never a skip.

### 4.5 Tracked-file check

`check_ts_vitest_inputs` takes a `tracked` parameter: a set of repository-relative paths. In
production, `main()` builds it once with `git -C <root> ls-files -z`. A failure of that call is an
infrastructure error (rc 2). The self-test passes a fixed set.

### 4.6 Floors

A13 asserts containment. A containment check whose `want` set empties passes and asserts
nothing (A7's lesson). Two floors guard against that:

- `REQUIRED_VITEST_TASKS`: `paigasus-kernel-ts:test`, `paigasus-console-core-ts:test`,
  `iam-console-ts:test`, `gateway-console-ts:test`, `paigasus-auth-ts:test-e2e`. A floor task
  that A13 does not derive is a row.
- `REQUIRED_VITEST_ALIASES = {"paigasus-kernel-ts:test": {"@paigasus/node-bindings",
  "@paigasus/wasm"}}`. A floor alias that the parser does not return is a row. This catches a
  parser that stops finding aliases.

### 4.7 Function and row shape

```python
def check_ts_vitest_inputs(projects, root, tracked, floor=REQUIRED_VITEST_TASKS,
                           alias_floor=REQUIRED_VITEST_ALIASES):
```

`root` and `tracked` are positional and required, for the reason in A7's docstring (SMA-560 I3).
Row order: floor rows, then the package.json walk rows and parser rows, then per-task rows, all
deduplicated with `dict.fromkeys`. A task that reports no `inputFiles` or `inputGlobs` bucket is
a row, as in A12a. Per-task omission rows read `<target> inputs omit <entry>`.

### 4.8 Registration

- `EXPECTED_FINDING_KEYS` gains `"a13"` after `"a12b"`. `collect_findings` gains the `a13` tuple
  in the same position. Its title ends with a `Fix:` sentence that names the input to add and
  points at `ci/affected-graph/README.md`.
- The PASS sentence in `main()` and the header comment of `cargo_moon_parity.py` name A13.

### 4.9 Reachability of the gate

`repo:affected-smoke` (root `moon.yml`) does not list the vitest configs today. A13 reads them
from disk, so an edit to a config alone would not schedule the gate. Add:

- `ts/packages/*/vitest*.config.ts`
- `ts/apps/*/vitest*.config.ts`

Add the same two globs to `T_AFFECTED_SMOKE_REQUIRED_INPUTS` in `ci/actionlint/run.sh`, so that
removing them reds `repo:actionlint`. `repo:input-liveness` checks that each glob matches tracked
files; both do.

### 4.10 Input fixes

Run A13 on the real tree. Fix every row by adding inputs in the owning `moon.yml`. Add no
product code. If a row looks wrong rather than missing, stop and ask.

### 4.11 `ci/affected-graph/run.sh` re-baseline

New inputs change the set of tasks that an edit selects. Re-measure every affected case with the
no-flag command quoted in its comment, and extend each expected CSV with the newly selected
`:test` and `:test-e2e` rows. Do not derive the sets; measure them.

Add one control case: `napi-glue-js->console-test`. It edits
`rs/crates/bindings/paigasus-node-bindings/index.js`, and its expected set holds the console
`test` tasks.

On the development Mac, `run.sh` needs system bash 3.2. Run it from a bash-only shim directory
(memory: affected-smoke hang).

## 5. Tests

`self_test()` gains A13 rows in the A12 style: copy a fixture, break one thing, and assert one
exact row string.

- **T1** The clean fixture passes.
- **T2** One mutation for each part of `want`: the config file; an upstream `src/**/*` glob; an
  upstream `package.json`; an `exports` target outside `src/`; a non-`.d.ts` `files` entry of a
  binding (the `.wasm`); a tracked alias target.
- **T3** An untracked alias target on a task that is not an FFI task is a row. The same target on
  an FFI task passes.
- **T4** An alias to a package that is not in the closure is a row.
- **T5** Parser: an alias value that is not an object; an unresolvable `@paigasus/*` value; a
  spread entry; a commented-out `@paigasus/*` alias (ignored); `//` inside a string literal (not
  a comment); a `fileURLToPath(new URL(…))` binding resolves correctly.
- **T6** Config flags: `--config f`, `--config=f`, `-c f`, two vitest calls in one script, a
  missing config file, a config path with `$`.
- **T7** The proto dep; the task floor; the alias floor; a task with no input bucket; a `None`
  invocation (rc 2); a non-typescript decoy that is ignored.
- **T8** `files` that is not a list, and a `files` entry with a glob character.
- **T9** The shared helper: A12a's existing self-test rows still pass after the move to
  `workspace_package_inputs`.

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

- **N1** Own-package files of a task (`tests/**`, `setupFiles`, in-package aliases). The
  inherited `@group(tests)` and the hand-written lists cover them.
- **N2** A relative import that leaves the package without a declared dependency.
- **N3** A vitest call behind a wrapper script. The task floor catches the loss of a known task,
  not a new wrapper.
- **N4** Playwright and `next build`.
- **N5** The inputs of a scratch alias's build. A5 and A7 own them (D3).
- **N6** Proving that each required input changes the result of a test. The rule is "does this
  file influence the output", as for A10.

## 8. Risks

- **R1** The static parser can miss an alias form that a future config uses. Mitigation: any
  `alias` value outside the grammar is a row, and the alias floor reds if the kernel aliases
  disappear from the parse.
- **R2** Over-approximation. A13 requires inputs that a specific test file never imports (for
  example the napi files for a console test). This selects more tasks than strictly needed. The
  same trade-off is accepted for A12 (SMA-536 §3.4).
- **R3** `git ls-files` adds a git dependency to the parity script. The gate already runs inside a
  git checkout in CI and locally. A failure is rc 2, never a false green.
