# affected-graph regression guard (SMA-409 / SMA-429)

`moon ci` *uses* the affected graph but never *asserts* it is correct: a deleted
`dependsOn` edge — or a dropped `@group(upstreams)` reference, the fileGroup that actually
confers affectedness in Moon 2.3.2 (not `--include-relations`, which SMA-528 measured to
change nothing in any probe, including the full 24-target CI shape) — makes the affected set
silently shrink, so CI under-builds and stays **green**. This guard closes that gap.

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
  command/args/script shape and A6's `inputGlobs` shape.
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
  moon's **resolved** `lint` `inputFiles` must contain `rs/Cargo.lock`, `rs/Cargo.toml` and
  `rs/rust-toolchain.toml`. The behavioural case proves the inputs take effect; A4 proves they are
  declared for crates no case names. It iterates every crate unconditionally — unlike A1-A3, which
  are guarded by `if want:` and so never reach the four crates with no in-tree dependencies.
- **A5** (in `cargo_moon_parity.py`) is A4's cross-stack twin (SMA-546): the tasks that COMPILE the
  FFI cdylibs live in the ts/py stacks, where A4's per-crate loop cannot reach them. A5 **derives**
  its targets — any task whose resolved `command` + `args` + `script` mentions `napi build`,
  `wasm-pack`, `maturin` or `--reinstall-package` — and requires each to declare `rs/Cargo.lock`,
  `rs/Cargo.toml`, `rs/rust-toolchain.toml` and `.prototools`. Deriving covers a future fourth
  binding task on day one; a `REQUIRED_FFI_TASKS` **floor** stops the derivation degrading to a
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
  `repo:release-parity*`, and `repo:version-lockstep`'s; SMA-553 / SMA-530 / SMA-576, each
  pinning `set -euo pipefail` alongside both of its invocations); `repo:actionlint`'s own
  self-test and mutation-battery calls inside `ci/actionlint/run.sh`
  (`ACTIONLINT_SH_CALL_SITES`, whole-line-matched — SMA-542); and
  `ci/release-parity/run.sh`'s own `--negative-control` logic — the flag parse, the guard,
  the assertion and the two report arms (`RELEASE_PARITY_SH_CALL_SITES`, whole-line-matched
  — SMA-530); **C5** every
  `moon ci` invocation in `ci.yml` is handed the WHOLE array — C1-C4 assert what is *in* `T`,
  and a subsetted `"${T[@]:0:5}"` leaves all four green while switching most of the graph
  off. C5's line matcher is deliberately BROADER than
  `assert_include_relations`' `moon ci +"` grep: mirroring it left both blind to a subsetted array
  behind a leading flag (`moon ci --base origin/main "${T[@]:0:5}"`). `moon ci` exits **0** on a target that resolves to nothing —
  measured, including the mixed case — so without C2 a renamed or mistyped entry is a silent no-op
  on every PR. Standalone cost is ~2.5s wall-clock (measured, mostly `moon query` subprocess
  startup, not CPU) — cheap enough to run inline inside `repo:affected-smoke` rather than justify a
  dedicated Moon task.

  Maintenance: adding a `repo:*` task means adding `:<name>` to `T` **and** to the command between
  `<!-- ci-targets:begin -->` / `<!-- ci-targets:end -->` in CLAUDE.md. A task that must stay out of
  `T` goes in `T_EXEMPT` with a required non-empty reason naming where it runs instead — an entry
  matching no `repo` task is itself reported, so exemptions cannot outlive their tasks.
  `runInCI: false` is not a general escape, because Moon then also drops the task from `moon run`
  under `CI=true` (`ts/moon.yml`). `REQUIRED_REPO_TASKS` is the floor that stops the comparison
  degrading to two empty sets. **`:affected-smoke` is load-bearing for every assertion in this
  file**: this gate runs *inside* it, so removing that one entry from `T` (and from CLAUDE.md)
  passes C1-C5 by never executing them, and takes the eight project cascade cases, the five task
  cases, A1-A6 and `assert_include_relations` with it. Never exempt or drop it — see the design
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
**pinned moon version** (currently 2.3.2). A4 additionally depends on `moon query projects`
emitting per-task `inputFiles` as a path-keyed object, A5 on it emitting per-task `command`,
`args` and `script`, and A6 on it emitting per-task `inputGlobs` the same shape as `inputFiles` and
per-project `language`. A moon upgrade that changes any of `inputFiles`, `inputGlobs`, `language`,
`command`, `args` or `script` — even benignly — will fail the guard, so re-grounding is a known step
of any moon bump. All three treat a missing key as a violation or an infrastructure error rather
than skipping, precisely so such a change cannot turn into a silent pass.
