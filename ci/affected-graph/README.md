# affected-graph regression guard (SMA-409 / SMA-429)

`moon ci` *uses* the affected graph but never *asserts* it is correct: a deleted
`dependsOn` edge — or a dropped `@group(upstreams)` reference, the fileGroup that actually
confers affectedness in Moon 2.5.3 (`--include-relations` adds only `paigasus-kernel-py:build`
on top of it: SMA-595 re-measured the flag at the full 27-target CI shape and got 44 RunTasks
with it against 43 without, where SMA-528 had measured it to change nothing at all on 2.3.2)
— makes the affected set silently shrink, so CI under-builds and stays **green**. This guard
closes that gap.

`run.sh` feeds a synthetic touched-file to `moon query projects --affected --downstream
deep` and asserts the affected project set **equals** an exact expected set per known case
(default-deny; `repo`, which owns the whole tree as its source, is filtered out):

Each **project** case below proves only that the `dependsOn` **edge exists** — that
`moon query projects --affected --downstream deep` marks the downstream project affected. It does
NOT prove `moon ci` schedules that downstream's build/test/lint: `--downstream deep` is a
QUERY-time traversal, and `moon ci` was measured to use neither it nor any widening from
`--include-relations` (SMA-528 — see the task-case paragraph below). Proving the cascade actually
runs is the `*_ci` task cases' job.

- **contracts edit** → `contracts` + `paigasus-proto-{rs,py,ts}` + `paigasus-gateway-rs`
  + `paigasus-iam-rs` (SMA-442) + `paigasus-service-info-rs` (SMA-505).
- **derive-crate edit** → `paigasus-proto-derive-rs` + `paigasus-proto-rs` + `paigasus-gateway-rs`
  + `paigasus-iam-rs` + `paigasus-service-info-rs` (SMA-438/SMA-524). One-directional w.r.t.
  contracts: the derive crate is strictly upstream of `paigasus-proto`.
- **service-info edit** → `paigasus-service-info-rs` + `paigasus-iam-rs` + `paigasus-gateway-rs`
  (SMA-524). One-directional w.r.t. `paigasus-proto`.
- **kernel edit** → `paigasus-kernel-rs` + `paigasus-py-bindings-rs` + `paigasus-node-bindings-rs`
  + `paigasus-wasm-rs` + `paigasus-gateway-rs` + `paigasus-kernel-py` + `paigasus-kernel-ts`
  + `paigasus-kernel-parity-rs` (both language wrappers wrap their bindings, SMA-419/420/427)
  + `paigasus-iam-core-rs` + `paigasus-iam-rs` (SMA-441).
  Strict equality rejects any other project implicitly.
- **py binding edit** → `paigasus-py-bindings-rs` + `paigasus-kernel-py`; one-directional w.r.t.
  the kernel.
- **node binding edit** → `paigasus-node-bindings-rs` + `paigasus-kernel-ts`; one-directional
  w.r.t. the kernel.
- **wasm binding edit** → `paigasus-wasm-rs` + `paigasus-kernel-ts`; one-directional w.r.t. the
  kernel. `paigasus-kernel-ts` now has two upstream binding edges — `paigasus-kernel-rs →
  paigasus-node-bindings-rs → paigasus-kernel-ts` (napi) and `paigasus-kernel-rs →
  paigasus-wasm-rs → paigasus-kernel-ts` (wasm, SMA-427) — so a kernel edit reaches it via both.
- **parity-crate edit** → `paigasus-kernel-parity-rs`; one-directional w.r.t. the kernel (a parity
  edit must not rebuild the kernel). The py/ts parity tests list the corpus as a task `input`
  (cache-keying), which does not make them project-affected by a corpus-only edit.

It also runs several checks that the per-case project sets structurally **cannot** make:

- **`proto->svc-info-deep`** asserts the affected *task* set (`moon query tasks --affected
  --downstream deep`), scoped to `build`, `test` and `lint` — the three tasks that carry `^:build`.
  `moon query projects --affected` follows `dependsOn` only and is blind to a task-level `^:build`,
  so deleting one keeps every project case **green** while `moon ci --include-relations` silently
  under-builds (SMA-429 F3, closed for build/test by SMA-524 and for lint by SMA-526). `lint`'s
  `^:build` is declared once, in `.moon/tasks/rust.yml`, rather than per-crate the way build/test
  declare theirs — so this case is also what catches a regression in that shared declaration.

  Every task case comes in two traversal modes, each with its own helper (`assert_task_case` /
  `assert_task_case_ci`, sharing a body in `_assert_task_case_impl`). The `deep` cases (this one,
  `lockfile->all-lint`) use `moon query tasks --affected --downstream deep` — what the TASK GRAPH
  would cascade — and are retained after SMA-528 as the only BEHAVIOURAL detector of a deleted
  `^:build`, since affectedness now comes from task inputs and a missing `^:build` would not move a
  `_ci` case's output at all. The `_ci` twins (`proto->svc-info-ci`, `lockfile->all-lint-ci`, and
  `kernel->consumer-tasks`, which has no `deep` twin — it is the case SMA-528 exists for) use no
  graph flags: the traversal `moon ci` actually uses. Measured relationship (SMA-528):
      `moon ci` RunTask set = (query-affected ∩ `ci.yml`'s `T` array ∩ `runInCI`) ∪ upstream-dep closure
  Both differences from a bare `--affected` query are benign for these cases — the `T` filter only
  removes tasks none of them assert (`build-release`), and the upstream-dep closure only adds
  builds. RE-MEASURE THIS ON A MOON BUMP, alongside A4's `inputFiles` shape, A5's
  command/args/script shape, A6's `inputGlobs` shape, and A7's reliance on all of those at once —
  it reuses A5's `command`/`args`/`script` derivation and reads both the `inputFiles` and
  `inputGlobs` buckets the way A4 does.
- **`lockfile->all-lint`** asserts that a `rs/Cargo.lock` touch schedules **every** crate's `lint`
  **and** the three tasks that compile the FFI cdylibs (`paigasus-kernel-ts:{build,test}`,
  `paigasus-kernel-py:test`). `rs/` has no Moon project, so the workspace files belong to `repo`
  and affectedness reaches both sets through task **inputs**, not through `dependsOn` — which is
  why no *project* case changes and this one is needed at all. Before SMA-534 that touch scheduled
  no crate task whatsoever, so every Dependabot Cargo PR was unlinted; before SMA-546 it still
  scheduled nothing that LINKS a cdylib or compiles `wasm32`, which clippy never does. The name is
  a deliberate misnomer — renaming it would break the `CLAUDE.md` procedure that greps for it.
  Its `_ci` twin, **`lockfile->all-lint-ci`**, asserts the same expected set under the no-flags
  traversal `moon ci` actually uses — expected to equal the `deep` set, since a `rs/Cargo.lock`
  touch reaches every row through task **inputs**, not `dependsOn`, so neither traversal-specific
  difference above applies to it.
- **`cargo-moon-parity`** (`cargo_moon_parity.py`) compares every crate's Cargo deps against Moon's own
  resolved graph, asserting each edge exists *and* schedules the upstream's build. The per-case sets
  assert only edges someone remembered to write a case for; this catches a crate added with **no**
  case — which is how SMA-524's bug survived a full review cycle. Edges intentionally declared without
  Cargo backing live in its `ALLOW_NO_CARGO_BACKING` table with a required reason string.
- **A4** (in `cargo_moon_parity.py`) is the generic twin of `lockfile->all-lint`: for every crate,
  moon's **resolved** `lint` `inputFiles` must contain `rs/Cargo.lock`, `rs/Cargo.toml`,
  `rs/rust-toolchain.toml` and `rs/.cargo/config.toml` (the fourth added by SMA-594 — cargo reads
  it by walking up from a cwd inside `rs/`, so it influences every compile and link). The behavioural case proves the inputs take effect; A4 proves they are
  declared for crates no case names. It iterates every crate unconditionally — unlike A1-A3, which
  are guarded by `if want:` and so never reach the four crates with no in-tree dependencies.
- **A5** (in `cargo_moon_parity.py`) is A4's cross-stack twin (SMA-546): the tasks that COMPILE the
  FFI cdylibs live in the ts/py stacks, where A4's per-crate loop cannot reach them. A5 **derives**
  its targets — any task whose resolved `command` + `args` + `script` mentions `napi build`,
  `wasm-pack`, `maturin` or `--reinstall-package` — and requires each to declare `rs/Cargo.lock`,
  `rs/Cargo.toml`, `rs/rust-toolchain.toml`, `rs/.cargo/config.toml` and `.prototools`. Deriving
  covers a future fourth binding task on day one; a `REQUIRED_FFI_TASKS` **floor** stops the derivation degrading to a
  vacuous PASS if a task ever stops matching the markers. A task with none of a `command`, a
  `script`, or any `args` aborts as infra (rc 2), never as a silent skip.
- **A6** (in `cargo_moon_parity.py`, SMA-528) asserts every crate's `build`/`test`/`lint` keys on its
  TRANSITIVE `dependsOn` closure's sources — `fileGroups.upstreams`, strict equality against moon's
  own Rust-restricted closure (`rust_closure()`, which excludes non-Rust build-scope parents like
  `contracts` and walks the transitive `dependsOn` closure rather than stopping at direct
  dependencies). No per-case task set can make
  this assertion: the `_ci` cases above only prove the specific pairs someone wrote a case for are
  wired, exactly the "no case at all" gap that let SMA-524's bug through, so A6 is the generic twin
  that iterates every crate. It is also the ONLY guard on `fileGroups.upstreams` at all — F5: a
  crate's own `moon.yml` is not an input to its own tasks (measured: a `fileGroups.upstreams` edit
  alone does not change any task's hash), so a stale or wrong group cannot red anything by itself.
  An intentional over-approximation (declared but outside the closure) needs a reason in
  `ALLOW_OVER_APPROXIMATION`, mirroring A2; a `REQUIRED_CLOSURE_EDGES` floor stops the closure
  derivation itself silently degrading to empty, mirroring `REQUIRED_FFI_TASKS`. A6 iterates crates
  by moon's reported `language: "rust"`, so a crate mislabelled in moon (a toolchain reshuffle, a
  hand-edited `language:`) drops out of A6's per-crate loop entirely; the floor catches this only for
  the crates named in `REQUIRED_CLOSURE_EDGES`. The general backstop is A4, which enumerates Cargo
  manifests from disk rather than trusting moon's `language` field, and `run.sh`'s
  `lockfile->all-lint` set, which lists every crate by hand.
- **A7** (in `cargo_moon_parity.py`, SMA-560) is A6's cross-stack twin: the py/ts wrapper
  projects — `paigasus-kernel-py`, `paigasus-kernel-ts` — carry hand-written `/rs/...` globs
  that ARE the ADR-0005 cross-binding guarantee, but A6 iterates `language == "rust"` only, so
  those wrappers were asserted by nothing generic. The kernel->wrapper edge specifically WAS
  already covered, by one hand-written `run.sh` case (`kernel->consumer-tasks`); what nothing
  covered is every OTHER upstream in a wrapper's closure, any new wrapper, and the
  under-declarations A7's first run found. A7 does not hand-write its task set either: it derives
  it from `derive_ffi_tasks()`, shared with A5, so a new wrapper's `napi build` (or equivalent) is
  examined on day one, even while it still declares zero inputs — the exact shape a hand-written
  list could not catch. Unlike A6, A7 asserts CONTAINMENT (`want <= observed`), not strict
  equality: a wrapper's globs are hand-written per task and legitimately mixed with non-closure
  entries under `rs/crates/` — the SMA-433 parity-vector corpus, and each binding's
  `package.json` / `pyproject.toml` — so strict equality would report those correct entries as
  violations. It reads BOTH moon input buckets, PER `(project, task)`, never unioned across a
  wrapper's tasks: `Cargo.toml` lands in `inputFiles`, `src/**/*` in `inputGlobs`, and a wrapper's
  `build` and `test` declare different sets, so a one-bucket or task-unioned read would pass the
  very under-declaration this check exists to catch. Per closure member it demands
  `<upstream>/src/**/*`, `<upstream>/Cargo.toml`, and — each when one exists ON DISK —
  `<upstream>/build.rs` plus any `<upstream>/*.pyi` stub (SMA-594') — which is why the repo
  `root` is a REQUIRED positional argument and never defaults: a `root=None` default made the
  build.rs half opt-in, so a call site that stopped passing it went
  on printing PASS while the two `paigasus-node-bindings/build.rs` lines could be deleted from
  `ts/packages/paigasus-kernel/moon.yml` for free. `REQUIRED_WRAPPER_CLOSURE` is its
  anti-vacuity floor, and it is edge-based rather than a task-name list for a reason specific to
  containment: a containment check whose `want` set empties is VACUOUSLY satisfied — it prints
  PASS having asserted nothing — and a task-name floor cannot see that, because the tasks are
  still examined even when their required upstream edges have gone missing.
- **A8** (in `cargo_moon_parity.py`, SMA-601) is the only assertion in this file about a task's
  FLAGS rather than its graph position. Every task whose resolved invocation reaches cargo must
  pass `--locked`, because an unlocked cargo re-resolves the dependency graph and REWRITES an
  inconsistent `Cargo.lock` in place. That is how five Dependabot PRs merged a truncated lock
  through a green required check: the first cargo task repaired the lock, and every later gate —
  `:deny` included — audited a resolution the PR never shipped. A8 matches the same resolved
  `command` + `args` + `script` blob A5 derives from, never file text: a text scan of
  `moon.yml`/`.moon/tasks/*.yml`/`rs/Dockerfile`/`ci/**/*.sh` was measured at 45 matches of which
  only ~14 were real invocations (`moon.yml`'s `echo "cargo tree failed …"` sits on an EXECUTING
  line), while the resolved blob measured 60 matches with 0 false positives.
  Since SMA-605 the match is not literal-only. `cargo_matches` merges three arms: the literal
  `cargo <verb>`, a cargo-NAMED variable in command position (`"$CARGO_BIN" build`), and the
  `CARGO=<path> <tool>` environment prefix. The first two are cleared by `--locked`; the third
  NEVER is, because the flag would reach the tool and not the cargo behind it, so it carries the
  same wrapper rule `FFI_MARKERS` does. Arm 1 reports **zero** rows on the corpus and is labelled
  forward cover in the code; arm 2 reports exactly one, at
  `ci/release-parity/ecosystems/release-plz.sh:152`, waived with a measured reason.
  Script-following is now transitive over `source` / `.` statements, cycle-guarded and confined
  to the repo (`script_source_refs`, `task_script_closure`, floored by
  `REQUIRED_SOURCED_SCRIPTS`). Bare `ci/**/*.sh` MENTIONS are deliberately not followed:
  measured at six edges, all comments or pin-array constants, one waiver, zero true positives.
  The two match kinds are NOT treated alike, and conflating them is measurably vacuous. A literal
  `cargo <verb>` match (`CARGO_INVOCATION_RE`) is cleared by `--locked` in the blob, because the
  blob IS the invocation. An `FFI_MARKERS` match is a WRAPPER whose own cargo call takes no flag,
  so it ALWAYS needs an `ALLOW_UNLOCKED_CARGO` entry with a non-empty reason, whether or not
  `--locked` appears anywhere in its script — `paigasus-kernel-ts:build` runs an unlocked
  `napi build` beside a `wasm-pack build … -- --locked`, so a blob-level test would green a task
  that still repairs the lock. A task matching both kinds is governed by the wrapper rule.
  `--frozen` is deliberately NOT accepted: it implies `--offline`, which false-reds on a cold
  cargo cache. Three tasks are the standing residual, each measured: `napi build` exposes no
  `--locked` and no cargo passthrough (and cargo has no env-var equivalent); `uv sync
  --reinstall-package` drives maturin, which drives cargo, with no flag path through either; and
  `wasm-pack build … -- --locked` DOES forward the flag to the cargo build it wraps, but wasm-pack
  makes its own unlocked cargo call BEFORE that build and repairs the lock there — against a
  truncated 176-package lock it exits 0 and rewrites the lock to 548. `REQUIRED_LOCKED_TASKS` is
  its anti-vacuity floor, for the reason `REQUIRED_FFI_TASKS` carries: a derived set that shrinks
  to empty asserts nothing while still printing PASS.
  A cargo call inside a `ci/**/*.sh` a Moon task invokes IS in scope since SMA-599
  (`check_cargo_locked_scripts`): the derivation follows the script and classifies each line
  under the conservative rule — only a heredoc BODY, a `#` comment tail and a bracketed OPERATOR
  SPAN are excluded from what the shell executes as a command. That last one is three shapes, not
  one: `$(( … ))`, a bare `(( … ))` arithmetic command, and `[ … ]` (an array subscript, a
  `[[ ]]` test, a glob); all three are blanked in the quote MASK only, so a `<<` inside them is a
  shift and a `#` a base marker while the code text survives and still classifies. Quoted string
  literals are NOT stripped, so a cargo verb sitting inside one reports like any other and is
  waived by hand (`ALLOW_UNLOCKED_CARGO_SCRIPT`) rather than silently excluded. **A10 does not
  share that line classifier**, and the README used to claim it did: `check_cargo_config_inputs`
  scans the RAW referenced file text, comments and heredoc bodies included, as its own docstring
  says. The direction is safe — a raw scan can only pull a task INTO scope, never out of it — so
  its failure mode is a row a human dismisses, not a silently missed one. What the two DO share is
  `derive_cargo_tasks`, and A10 follows a task's scripts for **every** kind, not only `script`:
  `derive_cargo_tasks` assigns `literal` on any cargo verb anywhere in a blob, prose included, so
  a benign `echo "running cargo check"` beside the invocation used to take that gate out of A10's
  scope entirely (SMA-599 review; the fix is a no-op on today's corpus and closes the hole).
  Each cargo invocation gets its OWN row — `finditer`, not `search` — and reads `--locked` from
  its own tail, bounded by the next invocation in the segment. Reading only the first match hid a
  nested unlocked call behind a locked one, the single failure direction the conservative rule
  claims it cannot have; `ci/actionlint/run.sh:3715` is the one live instance and is waived.
  Two limits remain. The scan is PATH-INSENSITIVE (spec L1) — it reports a line the task's
  arguments may never reach, which is why `repo:version-lockstep`'s `cargo update -w` is waived
  rather than excluded. And following is shell-only: `ops/nats/check-subjects.sh` and the three
  `.py` gate entrypoints are unfollowed. Since SMA-605 a script invoking another through a
  `source` / `.` statement IS followed — transitively, cycle-guarded, over executable text only
  (a `source` in a heredoc body is not executed and does not resolve) — but a script invoked as a
  plain command from another script still is not.
  Separately, the literal-cargo half still tests `--locked` PER BLOB, not per invocation: a
  task's own `command`/`args` field chaining two cargo calls with the flag on only one of them
  would pass — the same vacuity class the wrapper rule closes, left open here because a
  per-invocation split needs a shell parse the gate deliberately does not do for a task's own
  command field.
  Nothing is wrong today: exactly one task has more than one regex match
  (`repo:wasm-getrandom-free`, whose second match sits inside an `echo`), and its real cargo call
  is locked.
- **A9** (in `cargo_moon_parity.py`, SMA-604) is the only assertion in this file about a consumer
  OUTSIDE this repo. Every workspace crate must be reachable through **Dependabot's** expansion of
  `rs/Cargo.toml`'s `[workspace] members`, which is weaker than Cargo's: `expand_workspaces`
  (`cargo/lib/dependabot/cargo/file_fetcher.rb`) lists exactly ONE directory level below a glob's
  literal prefix, so `crates/*/*` yields `crates/libs`, `crates/services`, `crates/bindings` and
  then drops all three, because `File.fnmatch?("crates/*/*", "crates/libs")` is false. That
  resolved to **zero** members, leaving Dependabot to build its sandbox from the five crates still
  reachable through `path =` in `[workspace.dependencies]` — a 5-of-13 workspace that cargo
  re-resolved to 176 packages against 543. Both known symptoms follow from it: the recurring
  truncated `rs/Cargo.lock` A8 exists to survive, and a hard red on every `cargo in /rs` job from
  2026-08-17 on (`cargo update -p serde:1.0.228` reports `Locking 0 packages` in the reduced
  workspace, since serde 1.0.229 needs `serde_core =1.0.229` and `serde_core` is not in the `-p`
  set). A8 and A9 are therefore the two halves of the same lockfile story: **A8 catches a truncated
  lock once it exists; A9 removes the thing that writes one.**
  A9 does not encode a rule of its own — it TRANSCRIBES the Ruby into
  `dependabot_expand_member`, because a rule restated in this file's terms would drift away from
  the expander it models. The self-test measures both forms against a synthetic tree and fails if
  they ever agree, since a two-level glob resolving to anything would mean the model has stopped
  reproducing the bug. Two rows, deliberately: a glob that resolves to nothing, and — separately —
  a crate directory no entry reaches, which is the same shrunken-sandbox failure one crate at a
  time and would survive a check that only tested the zero-resolve case. Nothing else in the repo
  can see this: `cargo metadata` is identical either way, every Moon task is identical, and every
  other assertion here is identical. **Adding a crate directory** (a fourth sibling of
  `libs`/`services`/`bindings`) needs a new `members` entry; adding a crate inside an existing one
  does not.
- **A10** (`check_cargo_config_inputs` in `cargo_moon_parity.py`, SMA-599, findings key `a10`) is
  every Moon task whose cargo subcommand COMPILES or LINKS, with cwd inside `rs/`, keying on
  `rs/.cargo/config.toml`. Scope is a CONJUNCTION: the verb predicate (`CONFIG_SENSITIVE_VERBS`,
  deliberately NOT A8's `LOCK_RESOLVING_VERBS` — reusing that list would have left `cargo fmt` in
  scope by an accident of A8's lock-oriented list, not a stated exclusion) and a cwd rule that
  reads RAW text (with variable substitution ordered LONGEST NAME FIRST, so a `R=…` assignment
  cannot eat a later `$RS_DIR`), so `RS_DIR="$REPO_ROOT/rs"` … `cd "$RS_DIR"` still resolves.
  `cargo fmt`, `cargo tree`, `cargo metadata`, `cargo deny` and `cargo machete` are out of scope
  BY VERB — none compiles or links. That verb test is the ONLY thing excluding `repo:deny` and
  `repo:machete`, and the cwd half never runs for either: `deny` IS derived (it is in
  `LOCK_RESOLVING_VERBS`) but is absent from `CONFIG_SENSITIVE_VERBS`, so `_cwd_inside_rs` is
  never called; `machete` is absent from `LOCK_RESOLVING_VERBS` and is never derived at all.
  `repo:machete` runs `cargo machete rs` — a bare path ARGUMENT, not `--manifest-path`. The
  `--manifest-path` measurement belongs to `repo:deny` and stays load-bearing for decision D2,
  which is why a bare `rs`-containing argument must never confer a cwd: MEASURED on cargo 1.95.0,
  a malformed `rs/.cargo/config.toml` fails cwd=rs/ at rc 101 but leaves cwd=root+`--manifest-path`
  at rc 0, so `--manifest-path` does not move cargo's config walk. A10 reads moon's RESOLVED inputs, so its
  four inherited lines in `.moon/tasks/rust.yml` (`build`/`build-release`/`test`/`lint`) each
  cover thirteen crates — deleting one reds thirteen tasks. `ALLOW_MISSING_CARGO_CONFIG` ships
  EMPTY; every exclusion is structural. Counts on today's corpus: 59 tasks declare the file, A10
  examines 58 of them, and the one it does not — `paigasus-kernel-py:test` — is asserted by **A5**
  instead, whose `FFI_TASK_INPUTS` splat already demands it. A10 does NOT close the general problem: it shares
  `derive_cargo_tasks`'s VERB LIST with A8, `LOCK_RESOLVING_VERBS`, so a
  subcommand outside that list (`cargo llvm-cov`, `insta`, `udeps`, `bloat`) yields an empty
  derivation and stays invisible to A10 too (spec L11). SMA-605 added two INDIRECTION arms
  without touching that verb list, so L11 is unchanged. A10 carries its own sensitive variants
  of both: `CARGO_VAR_CMD_SENSITIVE_RE`, built from `CONFIG_SENSITIVE_VERBS` rather than reused
  from A8 — reusing A8's would pull `"$CARGO_BIN" tree` into A10's scope with nothing to red it
  — and the `CARGO=` prefix, which makes a task sensitive UNCONDITIONALLY, since A10 cannot read
  the subcommand the redirected tool will run.
- **`ci-targets`** (`ci_targets.py`, SMA-541) asserts `ci.yml`'s hand-written `moon ci` target array
  is complete and live: **C1** every CI-eligible `repo:*` task appears in `T=(…)` and — strict
  equality, not a subset — nothing in `T` names a `repo` task that is switched off; **C2** every `T`
  entry resolves to a CI-eligible task somewhere in the graph; **C3** CLAUDE.md's marker-delimited
  command mirrors `T` token-for-token in order and keeps its `--base origin/main
  --include-relations` tail; **C4** four separate haystacks all still carry the call site(s) that
  make some other gate run at all — this gate's own invocation in `ci/affected-graph/run.sh`
  (`RUN_SH_CALL_SITES`, substring-matched, each already carrying its own `|| RC=1` propagation
  suffix); a self-scheduled gate's invocation inside its own `moon.yml` task script
  (`SELF_SCHEDULED_GATES`, whole-line-matched — `repo:input-liveness`'s, the three
  `repo:release-parity*`, `repo:version-lockstep`'s, (SMA-572) `repo:affected-smoke`'s,
  `repo:publish-metadata`'s, `repo:error-code-single-site`'s and `repo:actionlint`'s, and
  (SMA-587/SMA-572) `repo:http-extractor-envelope`'s;
  SMA-553 / SMA-530 / SMA-576 / SMA-572, each pinning `set -euo pipefail` alongside every one
  of its invocations — two for `repo:input-liveness` and the `repo:release-parity*` tasks,
  three for `repo:version-lockstep` (which also invokes `--self-test`), three each for
  `repo:affected-smoke`, `repo:publish-metadata`, `repo:error-code-single-site` and
  `repo:http-extractor-envelope`, and one
  bare invocation line for `repo:actionlint` — `ci/actionlint/run.sh`, no `set -euo pipefail`
  to pin, since a single command's status IS the script's status); `repo:actionlint`'s own
  self-test and mutation-battery calls inside `ci/actionlint/run.sh`, and — as of SMA-572/
  SMA-573 — check 8e's own production call site AND its two tables' `-ge` arity floors
  (`ACTIONLINT_SH_CALL_SITES`, whole-line-matched, column 0 — SMA-542/SMA-572, now nine
  entries); and
  `ci/release-parity/run.sh`'s own `--negative-control` logic — the flag parse, the guard,
  the assertion and the two report arms (`RELEASE_PARITY_SH_CALL_SITES`, whole-line-matched
  — SMA-530); **C6** (SMA-592) `contracts:generate`'s authored `inputs` still equal
  `CONTRACTS_GENERATE_INPUTS` exactly — strict equality, both moon input buckets, the injected
  `.moon/*` glob filtered first. That task is not a `repo:*` task, so C1 and C2 never look at it,
  but `ci.yml`'s codegen-drift step delegates its freshness to that task's cache key: the step
  runs `moon run contracts:generate` and diffs the three generated dirs, so an input dropped from
  the task makes the step regenerate nothing and diff the committed output against itself — a
  vacuous PASS. Exact equality, not containment: an edit to how the repo's codegen is keyed
  should stop a human, and the constant is cheap to update deliberately. `moon ci` exits **0** on a target that resolves to nothing —
  measured, including the mixed case — so without C2 a renamed or mistyped entry is a silent no-op
  on every PR. Standalone cost is ~2.5s wall-clock (measured, mostly `moon query` subprocess
  startup, not CPU) — cheap enough to run inline inside `repo:affected-smoke` rather than justify a
  dedicated Moon task.

  > **C5** the `moon ci` branch block in `ci.yml` matches `MOON_CI_BRANCH_BLOCK` verbatim — eight
  > lines, indentation included — beginning immediately after the sole `T=(…)` line, and the file
  > carries exactly two command-position `moon ci` lines. Exact literals replaced a regex-based
  > shape rule in SMA-554, after that rule was bypassed four times during SMA-541's own review;
  > an exact comparison has no tail to enumerate. The anchor closes the decoy case: a verbatim copy
  > pasted elsewhere cannot bring its own `T=` line, because `parse_t` rejects a second one.
  >
  > C5 is a **second opinion, not the primary guard**. `ci/actionlint/run.sh`'s check 8b already
  > pins the same three invocation lines as exact literals, and check 8d **executes** the block
  > against a stubbed `moon` on four GitHub event paths — which is the only control that sees a
  > step-level `if: false` or an `if false; then … fi` wrap, since both leave every line
  > byte-identical. C5's value is that it is scheduled independently of `repo:actionlint`. Editing
  > those eight lines therefore has **four** co-update sites; C5's failure message lists them all.

  Maintenance: adding a `repo:*` task means adding `:<name>` to `T` **and** to the command between
  `<!-- ci-targets:begin -->` / `<!-- ci-targets:end -->` in CLAUDE.md. A task that must stay out of
  `T` goes in `T_EXEMPT` with a required non-empty reason naming where it runs instead — an entry
  matching no `repo` task is itself reported, so exemptions cannot outlive their tasks.
  `runInCI: false` is not a general escape, because Moon then also drops the task from `moon run`
  under `CI=true` (`ts/moon.yml`). `REQUIRED_REPO_TASKS` is the floor that stops the comparison
  degrading to two empty sets. **`:affected-smoke` is load-bearing for every assertion in this
  file**: this gate runs *inside* it, so removing that one entry from `T` (and from CLAUDE.md)
  passes C1-C6 by never executing them, and takes the eight project cascade cases, the five task
  cases, A1-A10 and `assert_include_relations` with it. Never exempt or drop it — see the design
  doc's L6.
  Not covered: whether a `repo:*` task's `inputs` still match anything — see the follow-up in the
  design doc's L3.

  A script-pinned gate must also have its `inputs` pinned (`SELF_TASK_EXPECTED_GLOBS`) or
  carry a reasoned `SELF_TASK_GLOBS_EXEMPT` entry; an exemption naming no script-pinned
  gate, or one with a blank reason, is itself reported. The registries were equality-paired
  until SMA-530 — a plain subset would have let `repo:affected-smoke` be script-pinned
  later without pinning the inputs that make every pin in this file reachable. The function
  that asserts this pairing, `check_registry_pairing`, is not called from `main()` — it is
  exercised only via the `--self-test` path, which CI reaches through
  `repo:affected-smoke` → `ci/affected-graph/run.sh --negative-control` → run.sh:404's
  `python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1`, a line pinned by
  `RUN_SH_CALL_SITES` above and mirrored by `ci/actionlint/run.sh`'s check 8c.
  Each value there is the gate's WHOLE authored input set, globs first then literal files,
  because moon resolves a wildcard entry into `inputGlobs` and a literal path into
  `inputFiles`: `repo:version-lockstep` (SMA-576) declares sixteen literal paths and no glob
  at all, so the glob-only comparison this replaced would have read every one of them as
  absent — and, being `got != expected or files`, was unsatisfiable for a file-only gate.

  SMA-572 added four more script-pinned gates to this pairing. Three carry exact
  `SELF_TASK_EXPECTED_GLOBS` entries — `repo:publish-metadata` (eleven literal paths/globs),
  `repo:error-code-single-site` (three) and `repo:actionlint` (`("**/*",)`, the premise every
  check in `ci/actionlint/run.sh` — 8, 8b, 8c, 8d and 8e — relies on, itself pinned back from
  `SELF_TASK_EXPECTED_GLOBS["actionlint"]`). The fourth, `repo:affected-smoke`, is instead
  registered in `SELF_TASK_GLOBS_EXEMPT` with a delegation reason: its own twenty inputs are
  the most load-bearing list in the repo — every pin in this file is reachable only because
  `moon.yml` still lists `moon.yml` itself among them — so an exact-match copy here would make
  `repo:affected-smoke` the sole judge of its own reachability, the exact defect the delegation
  exists to avoid, and would red on every legitimate glob addition, since the list genuinely
  grows. Enforcement instead lives in check 8e of `ci/actionlint/run.sh`, a gate scheduled
  independently of this one, whose production call site and two table arity floors are pinned
  back here by `ACTIONLINT_SH_CALL_SITES`.

  SMA-587/SMA-572 added a fifth script-pinned gate to this pairing after the original four:
  `repo:http-extractor-envelope` (SMA-587), which also carries an exact
  `SELF_TASK_EXPECTED_GLOBS` entry — its whole authored input set, two globs. `moon.yml`'s own
  comment on the task says the first glob is DELIBERATELY IDENTICAL to
  `ci/http-extractor/check.py`'s `SCAN_GLOB`, so an exact pin is what stops scheduling and
  scanning from drifting apart.

  Put plainly: `SELF_SCHEDULED_GATES["affected-smoke"]`'s third line — the bare
  `ci/affected-graph/run.sh` invocation — has NO true-positive coverage from this file. Any
  state in which that line is missing is a state in which `check_registry_pairing`, and every
  other assertion in `ci_targets.py`, never runs at all: `ci/affected-graph/run.sh` exits
  inside its own `--negative-control` branch, before reaching the real suite, so deleting the
  bare invocation line leaves only the control — which asserts against synthetic fixtures and
  exits 0 — with nothing in this file able to see the deletion. Its real enforcement is check
  8e's `T_AFFECTED_SMOKE_REQUIRED_SCRIPT`, read IN ORDER for exactly this reason.
- **`task-inputs`** (`task_inputs.py`, SMA-553) asserts every `repo:*` task's declared `inputs`
  still match a tracked file — the layer below `ci-targets`, which proves only that a gate is
  *wired*. **I1** no glob matches zero tracked files; **I2** every file input is tracked, by exact
  set membership (a wildcard-free pathspec prefix-matches a directory, so asking git would pass for
  any directory path); **I3** every task declares at least one input of its own, after subtracting
  Moon's injected `.moon/*.{…}` glob, which is present on every task and makes a "resolved" input
  set never empty; **I4** every pattern is one the gate will evaluate — braces, character classes
  and pathspec magic are rejected loudly rather than skipped; **I5** the anti-vacuity floors,
  including a **composition** guard requiring the inputs common to every `repo` task to be exactly
  that one injected glob, and a `**/*` assertion on this gate's own task.
  Scheduled by its own `repo:input-liveness` task rather than from `run.sh`: the verdict depends on
  the whole tracked tree, and `repo:affected-smoke`'s narrow inputs would serve a cached PASS on
  exactly the rename that kills a gate. Two live-fire canaries run on every invocation, so a
  matcher stuck reporting "live" cannot pass vacuously. `ALLOW_DEAD_INPUT` ships empty and requires
  a reason. Scope is `repo` only — the other 27 projects carry 98 legitimately-dead convention
  globs inherited from `.moon/tasks/{rust,typescript,python}.yml`. Standalone cost is ~6.0s
  wall-clock (measured, median of 3 alternating `moon run repo:input-liveness --force` runs,
  warm) — an order of magnitude below the ~35s a broadened `repo:affected-smoke` would cost on
  every PR, which is why this lives in its own task rather than folded into `run.sh` (design
  doc D2).

It also asserts every `moon ci` invocation in `.github/workflows/ci.yml` carries
`--include-relations`. NOTE (SMA-528): the flag was measured to change NOTHING in every probe run,
including the full 24-target `ci.yml` shape — do not read this assertion as evidence the cascade
works. It is kept because removing it on that evidence is an unforced risk and it remains the
documented mechanism should moonrepo fix the dependent traversal upstream. What actually carries the
cascade is `@group(upstreams)`, asserted by the `_ci` task cases above and by A6.

Run locally: `moon run repo:affected-smoke` (or `ci/affected-graph/run.sh`).
`repo:affected-smoke` runs `--negative-control` first and then the real suite, so the proof that
these assertions can report red is executed by CI rather than left as a manual step (SMA-534).
Run the control alone: `ci/affected-graph/run.sh --negative-control`.

## Maintenance — expected sets are exact (default-deny, SMA-429)

Each case asserts the affected set (minus `repo`) **equals** its expected set exactly — there is
no separate must-exclude list and no forbid enumeration. Cross-stack isolation is enforced
implicitly: any project that appears but isn't in the expected set fails the case.

- A project **unrelated** to a case never enters its downstream set, so it never appears → no
  maintenance (this is what the old hand-maintained forbid-regex existed to track).
- A project that **legitimately** becomes a new dependent (e.g. a future wasm kernel binding)
  makes the case fail with an `unexpected` entry → confirm the new edge is intended, then add the
  one project to that case's expected set.
- A **task** case (`assert_task_case`/`assert_task_case_ci`, e.g. `proto->svc-info-deep`) works at `pid:task`
  granularity, not project granularity, so its set can also grow without any new dependent
  project: widening the task-name filter itself (e.g. `lint` joining `build`/`test` in SMA-526)
  makes every already-listed project pick up a new `pid:task` row at once → same fix, confirm
  the new rows are intended, then add them to the case's expected set.
- `lockfile->all-lint` lists **every** Rust crate, so **adding a Rust crate always changes it** —
  unlike the project cases, which only change when the new crate joins a specific dependency chain.
  A4 needs no update in that situation: the new crate inherits `lint`'s inputs from
  `.moon/tasks/rust.yml`, which is the point of declaring them there. The case's three
  `build`/`test` rows are the FFI tasks (SMA-546) and are unaffected by adding a Rust crate; A5
  covers them, and likewise needs no update unless a *new* FFI-compiling task appears.
- A new Rust crate (SMA-528) must declare `fileGroups.upstreams` in its own `moon.yml` — a missing
  group is a hard graph-load error for every moon command, not a silent gap, so this cannot ship
  unnoticed. Adding an in-tree dep to an *existing* crate changes that crate's transitive `dependsOn`
  closure and therefore A6's expectation for it: `fileGroups.upstreams` must gain the new upstream's
  `src/**/*` and `Cargo.toml` entries, or A6 fails with an `inputs omit ...` row.

The expected sets are a snapshot of `moon query --affected --downstream deep` output at the
**pinned moon version** (currently 2.5.3). A4 additionally depends on `moon query projects`
emitting per-task `inputFiles` as a path-keyed object, A5 on it emitting per-task `command`,
`args` and `script`, and A6 on it emitting per-task `inputGlobs` the same shape as `inputFiles` and
per-project `language`. A7 depends on all four at once — it reuses A5's `command`/`args`/`script`
derivation to find its task set, reads the same `inputFiles`/`inputGlobs` pair A4 and A6 read (both
buckets, not one), and reads `language` to exclude the Rust projects A6 already covers — so it has
no assumption of its own beyond what A4-A6 already state. A8 depends on the same
`command`/`args`/`script` join A5 does, and on nothing else. A moon upgrade that changes any of
`inputFiles`, `inputGlobs`, `language`, `command`, `args` or `script` — even benignly — will fail
the guard, so re-grounding is a known step of any moon bump. All five treat a missing key as a
violation or an infrastructure error rather than skipping, precisely so such a change cannot turn
into a silent pass.
