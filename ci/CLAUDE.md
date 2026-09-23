# paigasus-core — `ci/`

<!-- Project memory. Claude Code loads this file only when it reads a file in
this directory, so it costs nothing in a session that stays out of ci/.
The root CLAUDE.md holds the repo-wide rules and the two gate-checked blocks. -->

<!-- moon-diagnosis:ok -->

<!-- The mentions of moon's CI report below agree with the diagnosis
     procedure in the root CLAUDE.md, which is the authority. -->

## Affectedness and task inputs

- `repo:affected-smoke` has aborted **twice**, both times under a concurrent `moon ci` on 2.5.3,
  at ~2.4s against its usual 6–8s: once on SMA-595, which captured no output, and once on
  SMA-592, which captured its output. The two are matched on SYMPTOM SHAPE alone — a sub-3s abort
  under a concurrent `moon ci` — so nothing proves they are one and the same failure. Neither
  session reproduced it: four attempts on SMA-595 (warm, cold `.moon/cache`, cold `MOON_HOME`, and
  cold `rs/target` with cargo compiling alongside), and three more on SMA-592. An inherited
  `MOON_BASE` was tested and ruled out (the gate passes with it set). The gate is otherwise green
  everywhere. If you see a sub-3s `affected-smoke` failure, capture the full task output before
  re-running, because a re-run passes and destroys the evidence.
  (SMA-597 measured the mechanism: it is OVERWRITE, not discard. A passing re-run rewrites
  `stdout.log`, truncates `stderr.log`, rewrites `lastRun.json` and flips the `ciReport.json` row
  to `passed`. See the diagnosis procedure entry below.)
  **The mechanism below is measured on the ONE session that captured output (SMA-592), not on
  both.** In that occurrence the failure is an infrastructure ABORT, not a red verdict: the gate's
  own nested `moon query projects` dies with `Error: proto-shim:
  Failed to execute proto for the shimmed command: Permission denied (os error 13)`, writes
  nothing to stdout, and the reader then raises `JSONDecodeError: Expecting value: line 1 column
  1`, so `run.sh` prints `FATAL [contracts->proto]: moon query failed` and
  `== affected-graph guard ABORTED: infrastructure error (rc=2) ==`. So the proximate cause THERE
  is the **proto shim failing to exec `proto` with EACCES** while a `moon ci` runs concurrently —
  why the shim is briefly non-executable is still unknown, and SMA-595's four hypotheses above
  stay ruled out.
  Two consequences. The gate FAILS SAFE — rc=2 is distinct from rc=1, and it never reports a false
  green. And the `proto-shim` line is the tell, so grep the captured output for it: if that line
  is there, the failure is not about the affected graph at all, and re-running the task alone
  (`moon run repo:affected-smoke --force`) passes in the usual 6s. If it is absent, this entry
  does not explain your failure — diagnose it on its own terms.
  The NDJSON entry above is the same root tool, a different symptom; both mean a `moon`/`proto`
  call inside a gate is the fragile part of an agent-driven local run, never in CI.
- A `repo:*` task's `inputs` are now asserted **live**: `repo:input-liveness`
  (`ci/affected-graph/task_inputs.py`) fails if a declared glob matches zero tracked files or a
  declared file is untracked, so moving a directory a gate keys on reds CI instead of silently
  switching that gate off. It also asserts its OWN `inputs: ['**/*']` is unchanged — narrowing it
  for cost would make it stop noticing exactly the renames it exists to catch. A genuinely dead
  input needs an `ALLOW_DEAD_INPUT` entry with a reason (SMA-553).
- A new Rust crate reds `:affected-smoke` until it's added to the `lockfile->all-lint` expected set
  in `ci/affected-graph/run.sh` — that case lists **every** crate, so **every** new crate changes it
  (SMA-534) — and, if it `dependsOn` `paigasus-kernel-rs`, to the `kernel->bindings` set as well
  (strict-equality guard, SMA-409). The parity gate's A4 needs no update: a new crate inherits
  `lint`'s workspace inputs from `.moon/tasks/rust.yml`. That case now also carries three non-lint
  rows — `paigasus-kernel-ts:{build,test}` and `paigasus-kernel-py:test`, the tasks that link the
  cdylibs and compile `wasm32` (SMA-546) — so keep them when re-baselining; a new Rust crate does
  not change them. New workspace deps may need
  `rs/deny.toml` `[licenses] exceptions` or a dev-only
  `[advisories] ignore` (Rust); an npm/pip advisory needs a version bump — a pnpm-workspace
  `overrides:` selector or `uv lock --upgrade-package` — or a justified `osv-scanner.toml`
  waiver; a dep consumed only by a later commit needs a temporary
  `[package.metadata.cargo-machete] ignored` allowlist (prune once consumed).
- **Standing rule: A `{ workspace = true }` in-tree dep needs a hand-written `dependsOn`; a
  `path =` dep needs none. Neither `dependsOn` nor `^:build` selects a downstream — only task
  `inputs` confer affectedness.**
  Moon 2.5.3's Rust toolchain resolves `path = "…"` Cargo deps into the project graph **automatically**
  (`moon query projects` labels them `source=implicit`), but does **not** resolve `workspace = true`
  inheritance. So a `{ workspace = true }` in-tree dep — the repo's default form — **must** be
  hand-declared in `dependsOn`, while a `path` dep needs nothing. This is why the drift was scattered
  rather than systematic, and it is the opposite of the "Cargo path deps are NOT auto-synced" claim
  that SMA-389 recorded and SMA-524 disproved. Either way the project edge alone is **not enough**:
  `dependsOn` is what `moon query projects --affected` follows, and a task-level `^:build` is what
  actually schedules the upstream's build under `moon ci --include-relations` — neither implies the
  other. `repo:affected-smoke` now asserts both generically for every crate
  (`ci/affected-graph/cargo_moon_parity.py`), so a new in-tree dep that forgets either one reds CI
  instead of silently under-building (SMA-524).
  Neither is enough on its own either: task `inputs` are the **only** thing that confers
  affectedness in Moon 2.5.3. `dependsOn` and `^:build` schedule an upstream's build but never
  **select** a downstream — a dependent runs only if independently affected. `--include-relations`
  is very nearly, but no longer entirely, inert: re-measured at the full 27-target shape on 2.5.3
  (SMA-595) it selects exactly ONE task the same command without it does not,
  `paigasus-kernel-py:build` — 44 RunTasks against 43, stable across repeated runs. On 2.3.2 the
  two sets were byte-identical (SMA-528). One added `build` is NOT a dependent closure, so the rule
  above still holds; do not read the flag as a working cascade. **Re-run that A/B on the next moon
  bump** — the delta moved once and can move again. Every Rust crate therefore
  declares its transitive upstream sources in `fileGroups.upstreams`, consumed by build/test/lint via
  `@group(upstreams)` in `.moon/tasks/rust.yml`. Omitting the group is a hard graph-load error
  (`project::unknown_file_group`) for every moon command; mis-declaring it reds
  `repo:affected-smoke`'s A6 — and **nothing else can**, because a crate's own `moon.yml` is not an
  input to its tasks, so a wrong group otherwise serves a cached PASS. `^:build` has a second job
  here: it orders `contracts:generate` before a downstream that keys on
  `paigasus-proto/src/generated/**`, so removing it as "vestigial" would make those cache keys
  nondeterministic.
- Broad `inputs: ['**/*']` Moon tasks (e.g. `repo:actionlint`) stay cheap only because
  `.moon/workspace.yml`'s `hasher.ignorePatterns` filters gitignored trees out of the hash walk.
- **Standing rule: What makes a contracts change select the service tests is
  `@group(upstreams)`, not `^:build`.**
  Adding a **new error-code emission site** in Rust reds `repo:error-code-single-site` until the file
  is added to `ci/error-registry/check.py`'s `MANIFEST` — as `emits` (which also requires a
  membership test asserting every code it emits resolves via `ErrorReason::from_wire_reason`),
  `asserts`, or `excluded` with a stated reason. The gate matches the registry's **declared**
  vocabulary, so it cannot see a code you invented and never added to
  `contracts/proto/paigasus/common/v1/error.proto`; adding the code there is what makes it
  resolvable on any consumer. Code **removal** needs no gate — both service crates carry
  `test: deps: ['^:build']`, so a contracts change already runs their membership tests.
  (Until SMA-528 this was aspirational: `^:build` schedules an upstream's build, it does not make a
  crate affected. What makes it true is `@group(upstreams)` — both service crates' `test` now key on
  `paigasus-proto`'s sources, so a contracts change that regenerates them selects the test.)

## Registering a new `repo:*` gate

- A new `repo:*` gate reds `:affected-smoke` until it is in **both** `ci.yml`'s `T=(…)` array and
  the marker-delimited command above — `ci/affected-graph/ci_targets.py` asserts the two agree, and
  that every `T` entry still resolves to a CI-eligible task. That last half matters because
  `moon ci` exits **0** on a target that resolves to nothing (even with real targets around it), so
  a typo is otherwise a silent no-op on every PR. A gate that must stay out of `T` needs a
  `T_EXEMPT` entry with a reason — `runInCI: false` is NOT a general escape, since Moon then drops
  the task from `moon run` under `CI=true` too (see the comments in `ts/moon.yml`). `T` must also
  stay a single-line bash array (SMA-541).
- `repo:actionlint` and `repo:affected-smoke` now **guard each other**, and neither can guard
  itself (SMA-542). `ci/actionlint/run.sh`'s check 8 asserts `:affected-smoke` is still in
  `ci.yml`'s `T=(…)` array, that no `moon` line discards its exit status (a `||`/`&&`/`;`/`|`
  tail), and that no step's `continue-on-error:` value suppresses it (anything but the literal
  `false`) — escape-hatched per line via `COE_SKIP`, keyed by BOTH the line number and the line's
  own text, so a shifted entry stops matching instead of silently absorbing a different occurrence
  that lands on the vacated line. In return, `ci_targets.py`'s `ACTIONLINT_SH_CALL_SITES` pins
  `run_self_tests` and `selftest_mutation_battery` as **whole lines** in `run.sh` (a substring
  match would survive deleting the call, since the name is a prefix of its own definition). That
  pin only works because `repo:affected-smoke` lists `ci/actionlint/**/*` in its `inputs` — remove
  that and the pin stays green on exactly the PR that breaks it. Adding a seventeenth-and-later
  `*_self_test` table means bumping `SELF_TEST_COUNT` (currently 16 — SMA-579 added the eleventh,
  `release_guard_self_test` at check 10, SMA-601 the twelfth, `cargo_lock_step_self_test` at
  check 8f, SMA-603 the thirteenth, `release_plan_self_test` at check 11, SMA-597 the
  fourteenth, `doc_diagnosis_self_test` at check 12, and SMA-647 the fifteenth,
  `early_exit_reader_self_test` at check 13, and SMA-612 the sixteenth,
  `pipe_capacity_self_test` (the full-gate preflight)): the gate asserts
  invocations AND definitions. The cycle's
  second half is now closed too (SMA-542 residual closure): check 8c
  in `ci/actionlint/run.sh` pins `ci/affected-graph/run.sh`'s own two call sites into
  `ci_targets.py`, mirroring `ci_targets.py`'s `RUN_SH_CALL_SITES` from the other, independently
  scheduled file — see `ci/actionlint/README.md`'s Limitations section (L6) for what residual
  still remains (a single combined edit deleting both gates' own call sites at once, the same
  bounded shape as the `T`-array cycle above).
- **Standing rule: A new gate of this shape carries five registry obligations. Not all five are
  self-enforcing — the `repo:ruff-ci` entry below holds the measured list.**
  `repo:workflow-credentials` (SMA-593) asserts that no `pull_request`/`pull_request_target`
  workflow **declares** a credential — a `secrets` key, `id-token: write`, `permissions:
  write-all`, or a `secrets` context read. It says nothing about whether one could **obtain**
  a credential by another path; the README's Non-goals list those, and no control in this repo
  audits per-scope `permissions:` breadth (`repo:actionlint` does not, and zizmor is named in
  prose but runs nowhere). A new gate of this shape carries **five** registry obligations, and
  missing any one of them reds `:affected-smoke`, not this gate: `ci.yml`'s `T=(…)` array, the
  marker-delimited command above, `SELF_SCHEDULED_GATES` (its four `moon.yml` lines —
  `set -euo pipefail`, `--self-test`, `--negative-control`, the real run),
  `SELF_TASK_EXPECTED_GLOBS` (both literal `inputs`), and
  `T_AFFECTED_SMOKE_REQUIRED_INPUTS` in `ci/actionlint/run.sh`, which floors
  `ci/workflow-credentials/**/*` among `:affected-smoke`'s own inputs — without that last one
  the script pin below stays green on exactly the PR that breaks it.
  (Correction, SMA-539: "missing any one of them reds `:affected-smoke`" overstates it —
  `SELF_SCHEDULED_GATES` and `SELF_TASK_EXPECTED_GLOBS` validate only entries already present as
  KEYS, so omitting either one was never self-enforcing on its own; see the `repo:ruff-ci` entry
  below for the measurement and the partial fix.)
  `WORKFLOW_CREDENTIALS_SH_CALL_SITES` pins **seven** lines in `run.sh`, and three of them are the
  ASSERTION: with only the flag parse, the dispatch arm and the two report lines pinned,
  deleting every `_expect` and `grep` row left all four byte-identical and the control exited 0
  having asserted nothing (MEASURED). SMA-647 split the one assertion line into three: the capture
  of the real run, the guard on its exit status, and the match. The old pipe failed OPEN when the
  checker failed, so deleting the status guard alone is a regression, and it is pinned too.
  The **exit codes differ between the checker and the wrapper, deliberately**:
  `workflow_credentials.py` exits **3** for an assertion failure, and `run.sh` maps 3 -> 1 and
  everything else -> 2. `uv` itself exits 1 on a failed resolution, so a shared code would let
  a PyPI outage read as "a workflow declares a credential". Do not "normalize" the checker to 1.
  `EXPECTED_PR_SUBJECTS` is a **hand-maintained strict-equality** pin of the seven subject
  filenames — a new `pull_request`-triggered workflow reds this gate until someone adds it,
  which is the point, so re-baseline it deliberately rather than loosening the comparison.
- **Standing rule: Registration is seven obligations. Only T-membership, the CLAUDE.md mirror
  and — once `SELF_SCHEDULED_GATES` holds a key — `SELF_TASK_EXPECTED_GLOBS` are self-enforcing.
  A brand-new gate with no registry entries at all still passes.**
  `repo:ruff-ci` (SMA-539) lints `ci/**/*.py` against `py/pyproject.toml`'s rule set — a new `ci/`
  Python file must pass it. It routes through `uv run --locked --project py` rather than a
  dedicated uv project: `repo:actionlint`'s `inputs: ['**/*']` strictly supersets `repo:ruff-ci`'s,
  so the `py` environment is materialised once per CI run regardless of which task reaches it
  first — one lockfile therefore means one ruff version, shared with `py:lint`, with no second
  lockfile to drift out of step. `ruff format` is deliberately NOT gated over `ci/`: MEASURED,
  `line-length = 200` would rewrite 2,998 of ~15,600 existing lines, joining hand-wrapped lines and
  collapsing hand-aligned fixture tables in files that are roughly 60% comment by design. The
  corpus is derived with `git ls-files -- ':(glob)ci/**/*.py' 'ci/*.py'`, not the bare
  `'ci/**/*.py'` alone: git matches a pathspec's `**` without `FNM_PATHNAME`, so the literal `/` is
  still required and a top-level `ci/foo.py` would be missed — `'ci/*.py'` (its `*` spans `/`) and
  the `:(glob)`-magic form each independently fix it, and the gate keeps both, which is mutually
  redundant and deliberately so.
  **Registration is seven obligations, not six** — `ci.yml`'s `T=(…)` array, the CLAUDE.md
  marker-delimited command, `SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS`, a script pin
  (`RUFF_SH_CALL_SITES`, ten lines), `REQUIRED_REPO_TASKS` — the last because this gate
  carries a `--negative-control`, the same reasoning that put the three `release-parity*` tasks
  and `workflow-credentials` on that floor — and `moon.yml`'s `repo:affected-smoke` task listing
  `ci/ruff/**/*` among its own `inputs` (`moon.yml:222-226`), floored by a
  `T_AFFECTED_SMOKE_REQUIRED_INPUTS` entry in `ci/actionlint/run.sh` (`:2134-2138`) — the same
  reachability pair the `workflow-credentials` entry above names for that gate: without it, a PR
  editing `ci/ruff/**` does not schedule `repo:affected-smoke` at all, so none of the other six
  obligations' pins can ever fire on the PR that breaks them.
  **This also corrects a belief this repo has been operating on.** MEASURED: `repo:affected-smoke`
  does NOT red when a brand-new `repo:*` gate is added to `ci.yml`'s `T` array with none of the
  other obligations done at all. `SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS` and
  `REQUIRED_REPO_TASKS` are hand-maintained tables that validate only entries already present as
  KEYS — so a brand-new gate with NO entries at all in any of the three still passes, the same
  true statement the `workflow-credentials` entry above makes for its own registration. That is
  narrower than it first looks, though: `orphan_globs` is not "a key with no task" (nothing there
  needs a task) but a `SELF_TASK_EXPECTED_GLOBS` KEY with no matching `SELF_SCHEDULED_GATES`
  entry — i.e. the reverse-pairing direction — and `check_registry_pairing` (called with all-`None`
  at `ci_targets.py:2647`, which resolves to the LIVE registries) genuinely IS exercised in
  production: it runs on the `--self-test` path that `repo:affected-smoke`'s own
  `--negative-control` invokes (`ci/affected-graph/run.sh:413`), so deleting only
  `SELF_SCHEDULED_GATES["ruff-ci"]` reds it. Obligation 4 (`SELF_TASK_EXPECTED_GLOBS`) is
  therefore transitively self-enforcing once obligation 3 (`SELF_SCHEDULED_GATES`) is in place —
  it is `REQUIRED_REPO_TASKS` and the reachability pair above that remain unpaired, so of the
  seven, T-membership, the CLAUDE.md mirror, and (once `SELF_SCHEDULED_GATES` exists)
  `SELF_TASK_EXPECTED_GLOBS` are self-enforcing; the `workflow-credentials` entry above overstated
  the ORIGINAL claim ("missing any one of them reds `:affected-smoke`") and is corrected there.
  SMA-539 closes part of the remaining gap with `check_self_scheduled_coverage` in
  `ci_targets.py`: every `repo:*` task whose resolved `script:` mentions `--self-test` or
  `--negative-control` must now have a `SELF_SCHEDULED_GATES` entry, with a reasoned
  `SELF_SCHEDULED_COVERAGE_EXEMPT` table that ships EMPTY. It is deliberately scoped to that ONE
  registry — the same treatment for `SELF_TASK_EXPECTED_GLOBS` or `REQUIRED_REPO_TASKS` would red
  the real repo, since several tasks legitimately lack those (`affected-smoke`'s own globs are
  pinned by check 8e instead; the three `release-parity*` tasks route through
  `SELF_TASK_GLOBS_EXEMPT`).
- `ci/affected-graph/ci_targets.py` derives its verdict AND its report from ONE list (SMA-638).
  `collect_findings` returns 23 `(key, rows, title)` triples; `main()` reads
  `if not any(rows for _, rows, _ in findings)` for the verdict and iterates that same list for the
  report, so a check folded into one and not the other cannot exist — the defect SMA-638 reported
  for `check_tailwind_guard_invocations`, which applied to every check in the file. What stops the
  list being SHRUNK is `EXPECTED_FINDING_KEYS`, a 23-key tuple whose non-emptiness, arity and exact
  key sequence `self_test()` asserts. So adding or removing a check reds the gate until that tuple
  is re-baselined, and the re-baseline is a deliberate act, never a mechanical edit to clear a red.
  The floor proves MEMBERSHIP, not semantics: a key whose `rows` are always empty satisfies it.
  Separately, `RUN_SH_CALL_SITES` (in that file) and `T_AFFECTED_GRAPH_CALL_SITES` (in
  `ci/actionlint/run.sh`) now hold **four** entries each, not two — the two `ci_targets.py`
  invocations, plus `ci/affected-graph/run.sh`'s `--negative-control` flag parse and its `NEGATIVE`
  branch guard, because `run.sh` initialises `NEGATIVE=0` and deleting either line let the control
  fall through and run the real suite twice at exit 0. The two tables are hand-mirrored and
  **nothing asserts the copies agree**, so every edit to one must be made to the other. Both are
  SUBSTRING pins (`site not in run_sh_text`; `grep -qF`): MEASURED, deleting a pinned line reds both
  gates, but COMMENTING IT OUT leaves both green. That is the documented limit
  (`ci/affected-graph/README.md`, L2), not a defect.

## Workflow linting and shell rules

- Workflow trigger filters are gated by `repo:actionlint`. Write `branches:`, `paths:` **and their
  `-ignore` variants** as **block sequences**, never the inline `branches: [main]` form — the
  gate's extractor does not parse inline flow and fails all four keys loudly rather than skipping
  them in silence. Every wildcard-free
  `branches:` entry must resolve as `refs/remotes/origin/<name>`; a branch that does not exist yet,
  or any entry carrying a glob character (`*`, `?`, `+`, `[]` — `+` included, since GitHub reads it
  as a quantifier), needs a justified `BRANCH_SKIP` entry in `ci/actionlint/run.sh`. A typo'd
  branch name otherwise disables a workflow silently and permanently (SMA-540).
- `ci/actionlint/run.sh`'s check 10 must route **every** exit status of its `release_guard_py`
  wrapper, not only the guard's own 2. `run.sh` is `set -uo pipefail` with **no `-e`**, so an
  unrouted status leaves `$RG_OUT` empty, the read loop finds nothing and the gate exits 0 having
  asserted nothing — measured at rc 127, the status a **missing `uv`** produces from the wrapper
  rather than from the guard. Do not read "missing uv aborts the gate" as a property of this
  routing: on the full-gate path that abort comes from `release_guard_self_test`'s
  `|| infra`. The wrapper also passes `--locked`, so the gate cannot re-lock `py/uv.lock` as a
  side effect (SMA-579).
- PyYAML coerces five shapes every workflow parser in `ci/` must handle: a bare `on:` key parses
  as the boolean `True`; `if: false` and `continue-on-error: false` parse as the boolean `False`;
  `continue-on-error: "false"` (quoted) parses as the **string** `"false"`, not the boolean;
  `needs:` may be a scalar string rather than a list, and iterating a string in Python yields its
  **characters**, not the string itself (SMA-579).
- GitHub Actions supports YAML **anchors and aliases** (since 2025-09-18) but **not merge keys**
  (`<<:`) — a workflow using one keeps the literal, unmerged key rather than erroring (SMA-579).
- `repo:actionlint` now runs shellcheck over every workflow `run:` block, sourced from
  `shellcheck-py` pinned in `py/uv.lock` (bounded specifier `>=0.11.0.1,<0.12`), resolved via
  `uv run --locked --project py` and asserted with `[ -x ]`. It FAILS CLOSED at rc 2 — there is
  deliberately no fallback to whatever `shellcheck` a host happens to have, the silent-downgrade
  failure SMA-525 refused. shellcheck's own GitHub release ships 13 platform archives and no
  checksums asset (re-measured 2026-09-02), which is why it is not a proto plugin; three of
  `shellcheck-py`'s pinned digests were verified by hand against koalaman's release assets, and a
  version bump re-opens that check.
  **Coverage limit, stated plainly so "the inline bash is linted now" isn't read as more than it
  is.** MEASURED: actionlint replaces `${{ }}` expressions with inert placeholders before handing
  the script to shellcheck. `rm -rf $TARGET` fires `SC2086`; the structurally identical
  `rm -rf ${{ github.event.inputs.target }}` fires nothing. So the entire GitHub-expression
  interpolation class — the dominant quoting/injection hazard in `release.yml` and `wheels.yml` —
  is uncovered by this gate. zizmor is the tool for that class and runs nowhere in this repo. Also
  structural, not a configuration gap: `SC2148` and `SC2164` can never fire, because actionlint
  supplies the shell itself and injects `set -e`.
- `ci/actionlint/run.sh` check 12 requires a `<!-- moon-diagnosis:ok -->` (or `:superseded`) marker
  on ANY file that names `ciReport.json`, unless the file is listed in `CIREPORT_MENTIONS_ALLOWED`.
  This is broader than the `doc_diagnosis_self_test` entry above says: it is not only about this
  file's own diagnosis procedure block. A new plan or spec that quotes the procedure, or otherwise
  mentions `ciReport.json`, reds the gate until it carries the marker or is added to the allowlist.
- **A pipe into a reader that can exit early is a false red under `pipefail`** (MEASURED, SMA-647).
  `grep -q` stops at its first match. `grep -m N` stops after N matches. `head` stops after N
  lines. `awk … exit` stops at its `exit`. Each one stops before it reads the rest of its input. A
  producer that writes again after that gets SIGPIPE and exits 141. Under `pipefail` the pipeline
  status is then 141, although the reader found the match. On Linux in CI the race is rare and
  needs CPU load, so a re-run passes: it caused three false check-12 reds in `repo:actionlint`
  (PRs 223, 255, 258). On macOS with BSD grep 2.6.0 it is near-certain: the old check-12 probe
  missed on 500 of 500 runs against the real block. Use one of three forms instead. When the
  producer's status does not
  matter, use process substitution: `grep -qF -- "$lit" < <(printf '%s' "$block")`. When the
  status matters, capture the producer into a variable, check its status, then match the variable
  the same way; declare a `local` on its own line, and under `set -e` write the capture as the left
  side of `||`. In place of `head -1` or `grep -m1 … | sed`, use `sed -n 1p`; in place of
  `awk '…{print $2; exit}'`, take the first match in awk's `END` block. Do not use a here-string:
  Homebrew bash 5.3.15 deadlocks on one over about 512 bytes (see the LOCAL ONLY entry). Do not use
  `>/dev/null` in place of `-q`, and do not use a `sed` script with `q`. `ci/actionlint/run.sh`
  check 13 bans the pattern in every tracked `*.sh`, workflow, `moon.yml`, `.moon/**/*.yml` and
  `lefthook.yml`. The rule reads only the next command word after the pipe, so a reader inside a
  subshell or a brace group, a reader behind a wrapper word, and a `|&` pipe are not seen — see
  `ci/actionlint/README.md` L33–L41. Its allowlist, `EARLY_EXIT_READER_ALLOWED`, ships empty. Its
  fixtures live in `ci/actionlint/fixtures/early-exit/*.txt`, because check 13 also scans
  `run.sh`. Check 12 keeps
  a second opinion on each missing literal: a `literal-disagreement` row means `grep` and a bash
  `case` match disagree, which is evidence of a second mechanism. Keep that run's whole output for
  SMA-647 before you re-run.
- `crane config` accepts a **registry reference only**. MEASURED against crane 0.22.1:
  `crane config "oci-archive:in/paigasus-iam-amd64.oci.tar"` fails with `Error: fetching config:
  parsing reference … could not parse reference`, and with a missing file it tries to resolve a
  host named `oci-archive`. `oci-archive:` is a syft/skopeo transport, not a crane one. Read an
  image config from a local archive with `ci/images/release_decision.py labels <archive>` instead
  (SMA-658 C1); it prints `version=` and `revision=` from the same labels.
