# paigasus-core

Public, Apache-2.0 polyglot monorepo for Paigasus, orchestrated by Moon. Four workspaces:
`contracts/` (protobuf + buf), `rs/` (Cargo: libs/bindings/services), `py/` (uv),
`ts/` (pnpm). **Status: bootstrapping** — workspaces are scaffolded issue-by-issue, so a
dir may hold only a README until its setup issue lands.

## Setup & commands

Tooling runs through [Moon](https://moonrepo.dev), pinned via proto in `.prototools`.
First-time setup: see [CONTRIBUTING.md](./CONTRIBUTING.md#local-development) (`proto install` → `moon`).

- `moon ci :build` / `moon ci :test` — affected build/test graph. **Moon 2.x needs explicit
  targets**; bare `moon ci` errors in non-TTY.
- Task output style is set in `.moon/tasks.yml` (`taskOptions.outputStyle`,
  currently `buffer-only-failure`). Moon 2.5.3 still has no per-invocation CLI flag
  for it (re-checked on the 2.5.3 bump, SMA-595); to stream a specific task locally, set
  `options.outputStyle: 'stream'` on the task definition. Note moon 2.4.3 changed the
  setting to apply only to TRANSITIVE targets, not the primary one you name.
- Rust (in `rs/`): `cargo build --workspace`, `cargo fmt --check`,
  `cargo clippy --workspace -- -D warnings`, `cargo nextest run --workspace`.

## Architecture

- `contracts/` — proto source of truth; buf generates Rust/Py/TS bindings (ADR-0004).
- `rs/crates/{libs,bindings,services}/` — `libs` = pure crates (e.g. `paigasus-kernel`),
  `bindings` = FFI shims (PyO3/napi/wasm), `services` = binaries. Service crates follow
  hexagonal architecture; libs/bindings do not.
- Cross-language behavior lives once in `paigasus-kernel` (Rust), bound to Py/Node/WASM —
  never reimplemented per language (ADR-0005).

## Conventions

- Every source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0`
  (`#` for Python).
- Branches: `feature/sma-NNN-<slug>` off `main` (NOT the old `sven/...` form).
- Conventional commits with a workspace scope: `feat(rs): …`, `fix(contracts): …`.
- Rust crates use **edition 2024 + rust-version 1.95**, even when an issue's AC says 2021.
- Significant choices get a Notion ADR before code; conventions live in the Notion
  Development Guidelines (both linked from CONTRIBUTING.md).

## Project memory map

This file holds the repo-wide rules. Workspace detail lives in a `CLAUDE.md` next to
the code, which Claude Code loads only when it reads a file in that directory.

| File | Holds |
|---|---|
| `rs/CLAUDE.md` | Cargo, the lockfile, nextest, container images |
| `ts/CLAUDE.md` | Next, vitest, Tailwind, Playwright, the console zones |
| `ci/CLAUDE.md` | the affected graph, `repo:*` gate registration, actionlint |
| `.github/CLAUDE.md` | release-plz, wheels, publishing, workflow guards |
| `contracts/CLAUDE.md` | codegen drift and the FFI bindings |

Add a new rule to the file that owns the directory it applies to. Keep this root file
small: it is loaded into every session, and the nested files are not.

Deeper detail for a CI gate lives in `ci/<gate>/README.md`. Read that before you change
a gate. Do not copy it here.

## Gotchas

### Moon and proto

- Moon is 2.5.3 and proto is 0.61.1 (SMA-595): `vcs.client` (not `manager`), `codeowners.sync`
  (not `syncOnRun`), Python/uv toolchains keyed `unstable_python` + a separate `unstable_uv`.
  The two pins are **coupled**: moon 2.5.3's Python toolchain plugin requires proto >= 0.60.0,
  and moon reads the proto CLI at the fixed path `~/.proto/bin/proto` — neither `PATH` order nor
  the `.prototools` pin overrides that, so a moon bump can hard-fail with
  `proto::tool::minimum_version_requirement` until the local proto BINARY moves. `proto upgrade`
  reports the target version and then no-ops inside an agent session; upgrade it from a normal
  shell, or set `PROTO_HOME` to an isolated root and `proto install` into it.
- **Standing rule: Export `PROTO_REPORTER=text` at the top of any gate script that captures
  `proto` output. A proto-SHIMMED tool is not exempt (SMA-609). Only `repo:release-parity` was
  ever affected, and the `unset AI_AGENT …` workaround is obsolete for these gates.**
  `proto` prints **NDJSON on stdout** when it detects an agent environment (`AI_AGENT`,
  `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`), including a `Detected an AI agent environment…`
  preamble line. That breaks any captured `$(proto <subcommand> …)`: the variable becomes a JSON
  blob, not a path, and the `||` fallbacks never fire because proto **exits 0** — it succeeded, it
  just answered in a different language. It is a property of proto's reporter, not of `proto bin`.
  **Pass `--reporter text`** (or `PROTO_REPORTER=text`) on any proto call whose stdout you capture;
  `ci/release-parity/ecosystems/release-plz.sh` is the worked example, and it also asserts
  `[ -x ]` on the result so a future regression fails at the assignment rather than 87 lines later
  (SMA-596). This is NOT new in proto 0.61.1; 0.58.1 behaves identically (measured both, SMA-595).
  A proto-**shimmed** tool (`uv`, `node`, `release-plz`) was recorded here as a different case —
  the shim execs the tool, so captured stdout is the tool's — "measured for the two cases below,
  not proven generally". **That carve-out is now DISPROVEN and must not be relied on** (SMA-609).
  A captured `uv` shim call leaked the preamble into `$(...)` twice on merged `main`: once from
  `repo:ruff-ci`'s binary resolver, once from `repo:actionlint` check 10's `--fixture-count`. Both
  died with a JSON blob where a path or an integer belonged. It is INTERMITTENT — the same command
  succeeds on the next run — so a green does not clear a captured shim call. Directly measured on
  the shim: the default reporter yields `{"type":"message",…}` on stdout, `PROTO_REPORTER=text`
  yields none. Every gate script that captures shim output therefore **exports
  `PROTO_REPORTER=text` once at the top** (`ci/actionlint/`, `ci/ruff/`, `ci/release-plan/`,
  `ci/release-parity/`), rather than prefixing each call site, so a future capture inherits it.
  The two binary resolvers additionally pipe through `tail -n1`; note that alone is NOT
  sufficient — on the multi-line NDJSON error the last line is still JSON, and what preserves
  fail-closed there is the `[ -x ]` assertion, not the tail.
  **Scope, corrected.** Only `repo:release-parity` was ever affected — NOT all three.
  `ci/release-parity/run.sh` sources exactly ONE ecosystem module per invocation, and only
  `release-plz.sh` invoked the proto CLI; `-py` resolves through `uv run` and `-ts` through `node`,
  and both were measured green in the same agent session that showed `release-parity` aborting.
  The earlier claim here that all three abort was a regression against SMA-530's own spec, which
  had the scope right. The `unset AI_AGENT CLAUDECODE CLAUDE_CODE_ENTRYPOINT` workaround is **no
  longer needed for these gates**; it may still matter for other proto oddities (see the entry
  above). **Residual:** nothing gates this. A new captured proto call written the broken way reds
  nothing (SMA-596 D4) — this bullet is the only control.
  Both ecosystem modules now resolve their tool with **no fallback** and assert `[ -x ]`, exiting 2
  with the harness's own `infrastructure error (rc=2)` classifier in the message — the module is
  sourced by `run.sh`, so an exit fires during the source and `run.sh` never reaches its own abort
  lines. That classifier is what keeps such a failure greppable.
- Bash tool PATH lacks the proto-managed CLIs; prefix commands with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so moon/uv/buf/nextest resolve to
  the repo-pinned versions (shims first).
- `moon query projects --json` **errors** on Moon 2.5.3 too (`unexpected argument '--json' found`,
  re-checked on the 2.5.3 bump, SMA-595) —
  bare `moon query projects` already emits JSON. **Measure its exit status UNPIPED (2):** `jq`
  returns 0 on empty input, so `moon query projects --json | jq …` reports 0 unless `pipefail`
  is set, and the failure reads as "the reader found nothing" rather than "the flag is invalid".
  That is not hypothetical — it cost a cycle on this very branch, where the first measurement
  read `head`'s status through a pipe and recorded exit 0.
- `moon query tasks --affected` emits each selected task's `deps[]`, and every dep entry carries a
  `"target"` key of its own. So `grep -o '"target": "[^"]*"'` over the raw JSON counts SCHEDULED
  upstreams as if they were SELECTIONS — it reported 15 tasks for a `.prototools` edit where the
  real answer is 12. Parse the JSON and take one target per `tasks[project][task]`. This matters
  because scheduled-vs-selected is the exact distinction every affectedness measurement in this
  repo turns on; an extraction that conflates the two cannot measure it. It inflated this branch's
  own spec table before the numbers were re-derived.

### Before you push: the full gate graph

- Per-project Moon tasks (`<proj>:build/test/lint/fmt`) do NOT run the repo-level gates
  (e.g. `:deny`, `:osv`, `:machete`, `:affected-smoke`, codegen-drift, CODEOWNERS). Before pushing
  new crates/deps/proto, run the full graph like CI does. The command between the markers below is
  gated against `ci.yml`'s `T=(…)` array by `repo:affected-smoke` — keep the two identical, and do
  not remove **or quote** the markers: a second copy of either one anywhere in this file, even
  inside backticks in prose, makes the count 2 and reds the gate (SMA-541):
  <!-- ci-targets:begin -->
  `moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site
  :http-extractor-envelope :input-liveness :promtool :observability-drift
  :nats-permissions :release-parity :release-parity-py :release-parity-ts
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci
  :next-public-free :test-e2e
  --base origin/main
  --include-relations`
  <!-- ci-targets:end -->

### Diagnosing an unattributed `moon ci` failure

- **FIXED (SMA-663): a `cargo metadata` error that names a `.napi-stage-<random>` path under
  `rs/crates/bindings/`.** `napi build` (@napi-rs/cli 3.10.3) stages every output in a sibling
  directory `.<crate>.napi-stage-<random>` that has no `Cargo.toml`. The old
  `crates/bindings/*` members glob matched it. Every concurrent `cargo metadata` call then
  failed with exit 101, for the full life of that directory. Only `paigasus-kernel-ts:build`
  and `:test` stage. Every other task in the error, `paigasus-kernel-py:test` included, was a
  victim. It reached 9 of 9 `moon ci` runs on SMA-658. Re-runs did not clear it. The fix: every
  `rs/Cargo.toml` `members` entry is a literal path. `repo:affected-smoke`'s A11 reds on a glob
  character there. Measured with a forced overlap of 200 napi builds: 2212 of 4624 concurrent
  `cargo metadata` calls failed with the glob. 0 of 2441 calls failed with literal entries.
  Moon does not count the staging directory as a project. The project count was 33, with or
  without a probe directory. **This error must not occur any more.** If it does, it is a new
  defect. Check first for a glob back in `members`, or a new crate path outside the list.
- The procedure below is MEASURED on moon 2.5.3
  (SMA-597); re-take it on a bump. It is for **local** runs — in CI see the note at the end.
  <!-- moon-diagnosis:begin -->
  **Step 0 — capture before you re-run anything.** A re-run overwrites every artifact holding the
  evidence, and a PASSING re-run is just as destructive as another failing one: it rewrites
  `stdout.log`, truncates `stderr.log` to zero bytes, rewrites `lastRun.json`, and flips the
  action to `passed` in `ciReport.json` (all four measured). Copy `.moon/cache/ciReport.json` and
  `.moon/cache/states/<project>/<task>/` somewhere outside the repo first.

  **Step 1 — which task, what command, what exit code.**
  ```bash
  jq '.actions[] | select(.status=="failed")
      | {label, error,
         exec: (.operations[] | select(.meta.type=="task-execution") | {command, exitCode})}' \
     .moon/cache/ciReport.json
  ```
  There is **no action-level `exitCode` key** — `has("exitCode")` is `false`. The widely copied
  query projects `{label, status, exitCode}` and so reports `null` for a key moon never writes,
  which is why this file has a reputation for being empty. It is not: the real exit code and the
  full command are in `operations[]`, on the entry whose `meta.type` is `task-execution`.

  **Step 2 — why.** `cat .moon/cache/states/<project>/<task>/stdout.log` and `stderr.log`. This is
  the only place task output exists. It works for a task that never started: a missing binary
  leaves `stdout.log` empty and `stderr.log` holding `command not found` (exit 127).

  **Step 2a — prove the logs belong to this run. Mandatory.** Compare the report action's
  `finishedAt` against `lastRun.json`'s `lastRunTime`. **If they disagree, stop** — the logs are
  from a different run and pairing them with step 1's command yields a confident wrong answer.
  Two measured causes: `moon run` writes `runReport.json` and does NOT touch `ciReport.json`, so a
  `moon run` re-run desynchronises them; and a cache HIT rewrites neither, so a log can be
  arbitrarily older than the run you are looking at.

  **Step 3 — if it still does not reproduce.** `moon run <target> --force`. Note that
  `buffer-only-failure` prints a FAILING task's output but discards a PASSING one's, and that this
  re-run desynchronises the report and the logs per step 2a.

  **What cannot work:** no `--summary` level (`none`/`minimal`/`normal`/`detailed`) and no
  `outputStyle` (`stream`/`buffer`/`none`) puts stdout or stderr into `ciReport.json`; all seven
  cells are byte-identical, and a key walk over the whole failing action finds no output field at
  any depth. `--log-file` captures moon's own tracing, not a task's stdio. Do not re-litigate this.

  **In CI this procedure does not apply as written.** There is no shell and the runner is
  destroyed, so step 0 is unexecutable; and `ci.yml` restores `.moon/cache` across runs, so a
  cache-hit task's logs may be from an older commit entirely. Use the `moon-diagnostics` artifact
  that `ci.yml` uploads on failure.
  <!-- moon-diagnosis:end -->

### Repo conventions and cross-cutting traps

- `.github/CODEOWNERS` is Moon-generated — don't hand-edit.
- `vcs.hooks` is intentionally empty; lefthook will own `.git/hooks` (SMA-371).
- A fresh `git worktree` starts with **no installed deps** (empty `ts/node_modules`, no
  `py/.venv`, unfetched cargo) — but lefthook's git hooks are shared across worktrees via the
  common `.git`, so `commit-msg` runs `commitlint` and **fails the commit** (`commitlint not
  found`) until deps exist. After `git worktree add`, provision the worktree before committing:
  `proto install` → `pnpm -C ts install` (installs commitlint + re-syncs hooks via its
  `prepare` step) → `uv sync` in `py/` → `cargo fetch` in `rs/`. Do **not** bypass the hook with
  `--no-verify`; install the deps.
- Never name a source file with a base name that is a **Windows reserved device name**
  (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`) — `PRN.<ext>` etc. are reserved
  too, so git can't check the file out on Windows (`error: invalid path …`). The Linux-only
  `CI` gate passes; only a Windows matrix job catches it — `prebuild`'s `build win32-x64-msvc`
  and `wheels`' `wheel win-amd64`. Both DO carry a `pull_request` trigger, but a PATH-FILTERED
  one (`.moon/**`, `.prototools`, `ts/pnpm-lock.yaml`, …), so whether a PR sees the failure
  depends on what else the PR touches, not on the bad file: SMA-448 (`prn.rs` →
  `resource_name.rs`) was green on the PR and red on `main`, while SMA-511
  (`ts/apps/iam-console/lib/prn.ts` → `prn-tenancy.ts`) reddened both Windows legs on the PR
  because it also touched `.moon/**`. The rule is language-neutral: it bit a `.rs` file and a
  `.ts` file the same way. An underscore/hyphen suffix (`prn_canonical`, `prn-fields`,
  `prn-tenancy`) is fine.
- The root `.gitignore`'s bare `build/` rule (line 41) silently ignores ANY directory named
  `build/` anywhere in the tree, not only a top-level one — `ts/apps/*/tests/build/` included. A
  file already tracked there stays tracked, so the trap is invisible until someone adds a NEW file
  under such a directory and it never gets committed. SMA-511 renamed its own directory to
  `tests/build-guard/` to avoid it, rather than fighting the ignore rule.

### This development Mac only

- **Standing rule: No single local bash runs every gate. `repo:affected-smoke` needs system bash
  3.2. `repo:ruff-ci`, `repo:next-public-free` and `repo:publish-metadata` need bash 4+.
  `repo:actionlint`'s full gate needs bash 5 AND a healthy pipe: it completes under Homebrew
  bash 5.3.15 when the host pipe holds 65536 bytes, and has no working local bash when the pipe
  holds 512.**
  CORRECTED AGAIN (SMA-664, MEASURED 2026-09-20): `ci/actionlint/run.sh` DOES complete locally
  under Homebrew `/opt/homebrew/bin/bash` 5.3.15 — twice, in 47 s each, rc 0, with no FAIL row.
  The gate's own preflight reported `pipe capacity 65536 bytes (floor 8192)` on both runs. So
  the flat claim below is conditional on the SMALL-PIPE state, not a property of the gate: read
  the preflight line before you conclude there is no local verdict. The same session measured
  the bash 3.2 half unchanged — the gate finished in 47 s and printed exactly the two FALSE
  `cargo-lock-step` rows. The 2026-09-14 facts below stay as written; the host was in the
  small-pipe state then and is not now.
  LOCAL ONLY, CORRECTED (SMA-512): no local bash currently runs `ci/actionlint/run.sh` to
  completion. The 512-byte pipe (see the next entry) is most probably the same cause; nobody
  measured the pipe state on these 2026-09-14 runs (spec §2). When the host is in that
  small-pipe state, the full gate now exits rc 2 in seconds under either bash, instead of
  hanging. The two candidates still failed differently before that fix. Under system `/bin/bash` 3.2.57 the gate
  does not deadlock — it still prints the two FALSE `cargo-lock-step` self-test failures — but it
  also does not finish: measured running past one hour without completing. Under Homebrew
  `/opt/homebrew/bin/bash` 5.3.15 the gate DEADLOCKS instead: measured three times independently on
  2026-09-14, at 0.0% CPU with 0.00s cumulative CPU time and zero live children after ten to fifteen
  minutes each time. So `/opt/homebrew/bin/bash` is NOT a working substitute for this gate — it is
  worse, not better — and an earlier version of this bullet recommending it was wrong. Keep the 3.2
  fact: it is still true, it just does not mean 3.2 finishes the gate either. CI is unaffected: it
  runs neither of these bash builds. MEASURED (SMA-647, 2026-09-18): `--self-test` ALONE is
  different. Under `/bin/bash` 3.2.57 it finishes in about 7 s with rc 1, and its only failures are
  the same two false `cargo-lock-step` rows, so it is a usable local check when those two rows are
  its only failures. Under Homebrew bash 5.3.15 it deadlocks too (0.20 s of CPU in 420 s). On the
  same day `/opt/homebrew/bin/bash ci/publish-metadata/run.sh --negative-control` also hung (0.01 s
  of CPU in 300 s, after its categories self-test), although SMA-645 recorded a pass for it.
  The affected-graph suite (`ci/affected-graph/run.sh`) is the
  opposite case: it needs system `/bin/bash` 3.2, because bash 5.3.15 deadlocks on a `while read`
  fed by a here-string over roughly 512 bytes on this class of machine. Keep both facts together:
  fixing one gate's bash version by copying the other's breaks it.
  MEASURED (SMA-512): a third gate pair needs bash 4+. `ci/ruff/run.sh` (lines 114, 239) and
  `ci/next-public/run.sh` (lines 151-198) both call `mapfile`, a bash-4+ builtin absent from system
  `/bin/bash` 3.2.57 — under it, both gates fail every self-test row (`mapfile: command not found`,
  read as an ordinary assertion failure, not an infrastructure error). Both pass cleanly under
  `/opt/homebrew/bin/bash` 5.3.15.
  MEASURED (SMA-645): `repo:publish-metadata` is a THIRD bash-4+ gate, and it fails differently —
  `ci/publish-metadata/run.sh:662` uses `declare -A` (an associative array), so under 3.2 the gate
  dies at once with `declare: -A: invalid option` on **stderr** and an EMPTY `stdout.log`, rather
  than failing self-test rows the way a `mapfile` gate does. Read an empty stdout plus a one-line
  `declare`/`mapfile` stderr as a bash-version artifact, never as a finding: the same commit passed
  `/opt/homebrew/bin/bash ci/publish-metadata/run.sh` ("all checks passed"). Expect more gates in
  this class — grep a failing gate for `declare -A` and `mapfile` before diagnosing anything else.
  So on this class of machine, no single local bash satisfies every
  gate: `repo:affected-smoke` needs 3.2 (no `mapfile`, and no here-string deadlock);
  `repo:ruff-ci`, `repo:next-public-free` and `repo:publish-metadata` need 4+; and
  `repo:actionlint` needs bash 5 and a
  healthy pipe, per the correction above. A local full-graph `moon ci` run must pick one bash for the
  whole invocation, then re-run the gates that need the other bash directly
  (`<bash-binary> ci/<gate>/run.sh`) and read those results instead of the `moon ci` verdict for
  them — `repo:actionlint` yields a local verdict only when its pipe preflight passes. CI runs a single Linux bash and
  never sees this split.
- **A new pipe on the development Mac can hold only 512 bytes, and two local hangs come from
  it** (SMA-612). A new pipe on that host holds 512 bytes (M4), not the nominal 16384 that
  kqueue reports (M5). The healthy macOS value on this host is not measured (spec D3). The pipe
  does not grow when it fills (M6). The Bash tool sandbox is not the cause: the same 512-byte
  limit appears with the sandbox disabled (M10). Mechanism 1: actionlint 1.7.12 writes a whole
  `run:` script into shellcheck's stdin before it starts shellcheck. A script over the pipe's
  capacity then blocks forever, and actionlint busy-loops (M2, M3). Mechanism 2: Homebrew bash
  5.x writes a here-string into a pipe before its reader starts. A write of 512 bytes finishes,
  and a write of 513 bytes hangs (M9). The gate's self-tests then deadlock at about 0% CPU (M8).
  `repo:actionlint` now runs a pipe-capacity preflight before its self-tests in full-gate mode.
  It exits rc 2 with a `small` message on a host below the 8192-byte floor. `--self-test` mode
  does not probe and still hangs under Homebrew bash on such a host. Remove the probe only per
  spec decision D9 in
  `docs/superpowers/specs/2026-09-19-sma-612-actionlint-pipe-capacity-design.md`. A `.prototools`
  actionlint version bump reds the gate on purpose until then (SMA-654). Other gates that use here-strings
  (for example `repo:affected-smoke`) still hang under bash 5.x on such a host. This entry does
  not fix them.

## Workflow

Specs/plans live in `docs/superpowers/specs/` and `docs/superpowers/plans/` (date-prefixed,
per Linear issue). Work flows brainstorm → spec → plan → implement. Linear keys are `SMA-NNN`;
PRs auto-link to Linear by branch name (don't attach links manually).
