# SMA-528 — make the downstream cascade real in `moon ci`

**Status:** revised after adversarial review (2026-08-19)
**Issue:** [SMA-528](https://linear.app/smaschek/issue/SMA-528) — *ci: `moon ci --include-relations`
does not cascade tasks, so no kernel consumer's tests run*
**Prior art:** SMA-409/429 (affected-graph guard), SMA-524 (Cargo↔Moon parity), SMA-526 (lint
propagation), SMA-534 (`Cargo.lock` lint inputs), SMA-546 (FFI workspace inputs)

---

## 1. Problem

A change to `rs/crates/libs/paigasus-kernel/src/lib.rs` runs the kernel's own tasks and the binding
wrappers, and **not one consumer's test suite** — not `paigasus-kernel-parity-rs:test` (the ADR-0005
cross-binding guarantee), not `paigasus-iam-rs:test`, `paigasus-iam-core-rs:test` or
`paigasus-gateway-rs:test`.

`repo:affected-smoke` asserts the cascade with `moon query … --downstream deep`. `ci.yml` runs
`moon ci … --include-relations`. **Those are not the same traversal**, so the gate is green while the
tasks it names never run.

## 2. Root cause — measured, not inferred

Moon 2.3.2 marks a task affected **iff one of its own declared `inputs` matches a changed file.**
Graph flags decide which nodes are *candidates*; they never confer affectedness.

Each row below feeds a synthetic touched file via `--stdin` and reads the `RunTask` actions out of
`.moon/cache/ciReport.json` (`moon run` rows read `runReport.json`). `T` is `ci.yml`'s exact
24-entry array.

### Touched file: `rs/crates/libs/paigasus-kernel/src/lib.rs`

| # | command | RunTask actions |
|---|---|---|
| F | `moon ci :build --stdin` | kernel-rs:build, kernel-ts:build, node-bindings-rs:build, wasm-rs:build |
| A | `moon ci :build --stdin --include-relations` | **identical to F** |
| B | `moon ci :build --stdin --include-relations --downstream deep` | identical + kernel-ts:test |
| — | `moon ci :lint --stdin` | kernel-rs:lint |
| — | `moon ci :lint --stdin --include-relations` | kernel-rs:lint |
| C | `moon ci :lint --stdin --downstream deep` | kernel-rs:lint |
| D | `moon ci :lint --stdin --include-relations --downstream deep --upstream deep` | kernel-rs:lint |
| E | `moon run :lint --affected --stdin --downstream deep` | kernel-rs:lint |
| G | `moon run paigasus-kernel-rs:build --downstream deep` (explicit target, **no** `--affected`) | **full 42-task cascade** |
| **K** | **`moon ci "${T[@]}" --stdin --include-relations`** — the real CI shape | **16 tasks**: kernel-py:test, kernel-rs:{build,fmt,lint,test}, kernel-ts:{build,test}, node-bindings-rs:build, py-bindings-rs:build, wasm-rs:build, repo:{actionlint,error-code-single-site,machete,parity-corpus-drift,publish-metadata,wasm-getrandom-free} |
| **L** | **`moon ci "${T[@]}" --stdin --include-relations --downstream deep`** | **byte-identical to K** |
| **M** | `moon query tasks --affected` (no graph flags) | 14 entries: K minus {node-bindings-rs:build, py-bindings-rs:build, wasm-rs:build}, plus kernel-rs:build-release |

### Touched file: `rs/crates/libs/paigasus-proto/src/lib.rs`

| # | command | RunTask actions |
|---|---|---|
| H | `moon ci :lint --stdin` | contracts:generate, proto-derive-rs:build, **proto-rs:lint** |
| I | `moon ci :lint --stdin --include-relations` | **identical to H** |
| J | `moon query tasks --affected --downstream deep` (what the gate asserts) | gateway-rs:lint, iam-rs:lint, proto-rs:lint, **service-info-rs:lint** |

### Findings

**F1 — `moon ci` schedules affected tasks plus their *upstream dependencies*. A dependent runs only
if independently affected.** In K, `node-bindings-rs:build` / `py-bindings-rs:build` /
`wasm-rs:build` appear not because they are affected (M omits all three) but because they are
upstream deps of affected tasks; `--upstream deep` is moon's default. `gateway-rs:build` declares
`deps: ['^:build']` on `kernel-rs:build` and is **not** scheduled. So `dependsOn` and `^:build`
**schedule** an upstream; neither **selects** a downstream.

**F2 — no flag combination on `moon ci` reproduces the asserted cascade, at the real CI shape.**
K and L are byte-identical across all 24 targets: `--downstream deep` adds nothing. Probe B's lone
delta (`kernel-ts:test` under a single `:build` target) does not survive the multi-target array —
with `:test` already in `T`, that task is selected by K anyway, so B was an artifact of the
one-target shape rather than evidence of a working cascade. Moon's own docs state `moon ci`
"additionally runs affected targets dependencies *and* dependents" and that it pre-fills
`--downstream=direct`; **both claims are false in 2.3.2** on this evidence. The one-flag fix is
therefore ruled out by measurement at the shape CI actually runs, not by extrapolation from a
narrower probe.

**F3 — the hole is wider than the issue states.** Probes H/I show SMA-526's `lint` cascade — added
specifically so "an upstream change that tripped `-D warnings` in a CONSUMER" could not ship green —
is equally inert in CI. This is not only about kernel consumers' tests.

**F4 — `moon query tasks --affected` is *not* equal to `moon ci`'s selection.** Comparing K and M,
the relationship is:

```
moon ci RunTask set  =  (query-affected  ∩  T array  ∩  runInCI)  ∪  upstream-dep closure
```

M contains `kernel-rs:build-release` (absent from `T`); K contains three upstream-only builds M never
lists. The two differ in **both** directions. §4.2's re-pointing therefore replaces one proxy with a
*characterized* proxy, not with the real thing — see §4.7, which is the honest treatment of that.

**F5 — a crate's `moon.yml` is not an input to its own tasks.** Measured: `moon run
paigasus-kernel-parity-rs:fmt` reported hash `12d26cbd` before and `12d26cbd` after adding a
`fileGroups.upstreams` block to that crate's `moon.yml`. `.moon/tasks.yml` lists `/.moon/*` files as
`implicitInputs`; per-project `moon.yml` is not among them. **Consequence: a wrong, empty or stale
`upstreams` group cannot red anything by itself — it serves a cached PASS.** A6 is the sole guard
against it, which is why §4.1 makes A6 strict-equality with an anti-vacuity floor rather than a
subset check.

Therefore **three** gates are vacuous with respect to what CI executes: `assert_case` (project graph,
`--downstream deep`), `assert_task_case` (task graph, `--downstream deep`), and
`assert_include_relations` (asserts the presence of a flag measured to change nothing in F/A, H/I,
and K/L). All green; none measures `moon ci`.

## 3. Approach

Task `inputs` are the only thing Moon's affected model responds to, so the fix declares them.

The **primary** justification is cache correctness, not scheduling: a consumer whose inputs omit its
upstreams replays a cached PASS built against a *different* upstream. That is a correctness bug
independent of which tasks get selected, and it is the same argument
`ts/packages/paigasus-kernel/moon.yml` and SMA-546 already make. Fixing scheduling is the
second-order benefit.

This is **not a new pattern**: `paigasus-kernel-ts:build` already declares
`/rs/crates/libs/paigasus-kernel/src/**/*` (SMA-420, extended by SMA-546), and that is exactly why
`paigasus-kernel-ts` appears in M while `gateway`/`iam` do not. SMA-534 used the same mechanism for
`/rs/Cargo.lock`. The fix generalizes a proven pattern to every crate.

### 3.1 Shape

`.moon/tasks/rust.yml`, once, on the three tasks that run in CI:

```yaml
tasks:
  build:
    inputs: ['@group(sources)', 'Cargo.toml', '@group(upstreams)']
  test:
    inputs: [..., '@group(upstreams)']
  lint:
    inputs: [..., '@group(upstreams)']
```

Every `rs/crates/*/*/moon.yml`:

```yaml
fileGroups:
  upstreams:
    - '/rs/crates/libs/paigasus-kernel/src/**/*'
    - '/rs/crates/libs/paigasus-kernel/Cargo.toml'
```

A leaf crate declares `upstreams: []`.

Verified against moon 2.3.2 before acceptance:

- **A project-defined fileGroup resolves inside an *inherited* task's `inputs`.** This is the
  property the design actually relies on — the crates declare a fileGroup, not per-task `inputs`,
  and the consuming `inputs` list lives in `.moon/tasks/rust.yml`. Evidence: with the group declared
  on `paigasus-kernel-parity-rs` and `@group(upstreams)` added to the inherited `build`, that task
  became affected by a kernel edit while every other crate's did not.
- **A crate that omits the group hard-fails at graph load** with
  `project::unknown_file_group … Has this group been configured?`. Verified. This is the point of
  routing through a fileGroup: a new crate physically cannot load until it declares its upstreams,
  so default-deny holds *before* any gate runs — which matters precisely because of F5, where a
  silently-absent declaration would otherwise be invisible. Empty groups are accepted, so a leaf
  crate satisfies it honestly.
- **`mergeInputs: 'append'` is the default**, so nothing inherited is displaced. Verified by reading
  a resolved task from `moon query projects`: the crate's own `src/**/*`, `tests/**/*`,
  `**/*_test.rs`, `Cargo.toml` and `/rs/.config/nextest.toml` all survive alongside the new entries.
  (Recorded for completeness — the design adds no per-crate `inputs`, so this is a guarantee it
  leans on rather than a mechanism it uses.)

**Rejected alternative — a default `upstreams: []` in `.moon/tasks/rust.yml`.** It would remove the
repo-wide graph-load cliff during landing (§4.8), but it also removes the hard-fail, which is the
main reason for choosing a fileGroup over per-task `inputs`. Combined with F5 — where an absent or
wrong group is otherwise undetectable at runtime — a silent default is the worse failure mode. The
cliff is transient and confined to one PR with the ordering in §4.8; the hard-fail is permanent
protection. Rejected deliberately.

### 3.2 Closure source: Moon `dependsOn`, restricted to Rust projects

`upstreams` is the **transitive closure over Moon's `dependsOn`, filtered to `language: rust`
projects**.

- *Why `dependsOn` and not Cargo:* `paigasus-gateway-rs → paigasus-kernel-rs` is a deliberate
  Cargo-unbacked over-approximation recorded in `cargo_moon_parity.py`'s `ALLOW_NO_CARGO_BACKING`.
  Deriving from Cargo alone would drop it and contradict the existing `kernel->bindings` project
  case. A1 already asserts `dependsOn ⊇ cargo deps`, so the Moon closure is the safe superset.
- *Why filtered:* Moon injects `contracts` into `projects[...]["deps"]` as a build-scope parent via
  the `contracts:generate` task dep — which is exactly why `NON_CARGO_PARENTS = {"contracts"}`
  exists (`cargo_moon_parity.py:64`). `contracts` has no `src/**/*` and no `Cargo.toml`, so an
  unfiltered closure would demand globs matching nothing for `paigasus-proto-rs`,
  `paigasus-service-info-rs`, `paigasus-iam-rs` and `paigasus-gateway-rs`. The exclusion is
  expressed by reusing `NON_CARGO_PARENTS` as the closure's exclusion set, with its reason string
  extended to cover this second use.
- *Proto source files are deliberately out of the closure.* A `contracts/proto/**` edit reaches
  consumers today because the generated Rust is committed in the same PR and lives under
  `paigasus-proto/src/generated/`, which the closure already covers. A `buf.gen.yaml`-only change
  that alters codegen without touching a `.proto` would not be caught — noted in §9 as a follow-up
  rather than silently assumed away.
- *Dev-dependencies are included* (a deliberate over-approximation): `cargo_crates()` folds
  `dev-dependencies` into `deps`, so a crate's `build` will key on an upstream it does not compile
  in that task. Stated as a decision, not inherited by accident; the cost is a rare extra rebuild
  and the alternative is a second, divergent closure.

Closure must be **transitive**: per F1 there is no propagation through `^:build`, so with
`A → B → C`, an edit to `C` would otherwise reach `B` and stop.

### 3.3 Per-upstream entries

Exactly two entries per upstream crate, in this form:

```
/rs/crates/<layer>/<name>/src/**/*      → resolves into inputGlobs  as rs/crates/<layer>/<name>/src/**/*
/rs/crates/<layer>/<name>/Cargo.toml    → resolves into inputFiles  as rs/crates/<layer>/<name>/Cargo.toml
```

The manifest is included because a feature flip or dependency change in an upstream genuinely
changes what a consumer compiles and never reaches it through `src/`.

**The brace form `{src/**/*,Cargo.toml}` is forbidden.** It is not merely a cosmetic optimization:
it would move the manifest from `inputFiles` into `inputGlobs` and thereby change which bucket A6
must read (§4.1). Fixing the two-entry form in the spec keeps the gate's contract stable.

Upstream `tests/**/*` is excluded — an upstream's tests do not change what a consumer compiles.

`fmt` and `build-release` are excluded: `fmt` is crate-local by construction, `build-release` does
not run in CI, and neither carries `^:build` today.

## 4. Gate changes — `ci/affected-graph/`

### 4.1 New assertion A6 (`cargo_moon_parity.py`)

For **every** Rust crate, the set of upstream-source entries in moon's *resolved* `build`, `test` and
`lint` inputs must **equal** the crate's transitive closure — strict equality, matching the
default-deny model SMA-429 moved this guard onto.

Three details are load-bearing:

1. **Read both buckets.** A6 asserts against `set(inputFiles.keys()) | set(inputGlobs.keys())`.
   SMA-534 measured, and §3.3 restates, that plain paths land in `inputFiles` and globs in
   `inputGlobs`; the `Cargo.toml` half of every pair lives in the former. Reading `inputGlobs`
   alone — as the pre-review draft of this spec said — would make the manifest half of A6 unable to
   fire, which is the same class of bug this issue exists to fix. An absent *either* key is a
   violation or an infrastructure error, never a skip, exactly as A4 treats it.
2. **Strict equality, with an escape hatch.** The observed set is every resolved input entry matching
   `rs/crates/*/*/{src/**/*,Cargo.toml}`; it must equal the derived closure. A subset check would
   let a removed `dependsOn` edge leave stale globs in place forever and let a copy-pasted
   `upstreams` block over-approximate permanently — unbounded, invisible CI cost in the exact
   dimension this change already spends heavily. Intentional over-approximation goes in an
   `ALLOW_OVER_APPROXIMATION` table with a required non-empty reason, mirroring
   `ALLOW_NO_CARGO_BACKING`.
3. **An anti-vacuity floor.** The closure is derived from `moon query projects`' dependency key; a
   moon rename or reshape would empty every closure and A6 would print PASS for thirteen crates —
   the degradation mode `REQUIRED_FFI_TASKS` exists to stop for A5. A6 carries
   `REQUIRED_CLOSURE_EDGES`, asserted *before* the input check: at minimum `paigasus-iam-rs` ⊇
   {`paigasus-kernel-rs`, `paigasus-proto-rs`} and `paigasus-kernel-parity-rs` ⊇
   {`paigasus-kernel-rs`}.

A6 iterates every crate unconditionally — like A4, and unlike A1–A3, which are guarded by `if want:`
and so never reach crates with no in-tree dependencies.

A6 adds no Moon task: it runs inside `cargo_moon_parity.py`, which runs inside `repo:affected-smoke`.
**`ci.yml`'s `T` array and CLAUDE.md's marker block are therefore unchanged**, and `ci_targets.py`
C1–C5 need no update.

### 4.2 `assert_task_case` gains a CI-traversal mode; the `--downstream deep` mode stays

The task helper is parameterized by traversal, and cases declare which one they assert:

- **`assert_task_case_ci`** — `moon query tasks --affected`, no graph flags. Asserts (a
  characterized proxy for) what CI selects. This is the measurement fix for issue item 2.
- **`assert_task_case_deep`** — the existing `--downstream deep` traversal, **retained**.

Retaining the deep mode is a correction to the pre-review draft. `run.sh` and `README.md` state that
the task cases exist because the project query is blind to `^:build` and "This case sees it" — and it
sees it *only* via the downstream traversal. Once affectedness comes from inputs, deleting a
`^:build` from a `moon.yml` would change no CI-traversal case's output, leaving only A3, which
asserts the declaration and never its effect. SMA-524 exists because a declaration-only assertion
was not enough, so dropping the deep mode would re-open that hole while closing this one. Both modes
are kept and each case is labelled with what it proves.

`proto->service-info-tasks` and `lockfile->all-lint` each gain a CI-traversal twin; their existing
deep assertions stay.

### 4.3 New task case `kernel->consumer-tasks`

Issue item 3. Pins a kernel source edit to `paigasus-kernel-parity-rs:test` — the highest-value
consumer and the one whose absence is least obvious — under **both** traversals.

The expected set is **enumerated in the plan from intent, not pasted from output**. Under strict
equality over `build|test|lint` across all projects it is substantially larger than the five
projects the pre-review draft named: it must also include `paigasus-kernel-rs`,
`paigasus-py-bindings-rs`, `paigasus-node-bindings-rs`, `paigasus-wasm-rs` (three rows each) plus
`paigasus-kernel-ts:{build,test}` and `paigasus-kernel-py:test` — on the order of thirty rows. The
implementation derives it by hand and diffs it against observed output, explaining any difference,
rather than snapshotting whatever moon printed.

### 4.4 `assert_case` kept, comments corrected

The project cases still guard `dependsOn`, which is what schedules an upstream's build through
`^:build`, so they retain real value and stay. What changes is their framing: the header comment,
the per-case comments and `README.md` all currently present them as proof that the cascade works.
They are rewritten to state what the query actually proves — that the *project* edge exists — and to
name F1 explicitly so the next reader does not re-derive it.

### 4.5 `assert_include_relations` kept, comment corrected

`--include-relations` stays in `ci.yml`; removing a flag is an unforced risk and it remains the
documented mechanism should Moon fix F2 upstream. But the gate's comment — "the edges are inert
without it" — is false and is rewritten to record that the flag produced **no delta** in F/A, H/I and
K/L, i.e. in every probe including the real 24-target shape. No probe in which it changes the
`RunTask` set was found; the spec says so plainly rather than implying one exists.

### 4.6 Negative controls

- A6 gains synthetic-violation cases under `cargo_moon_parity.py --self-test`, one per sub-assertion
  (missing glob, missing manifest, over-approximation without an allowlist entry, neutered
  derivation vs `REQUIRED_CLOSURE_EDGES`).
- A new **`expect_red_task`** helper is required: today's `expect_red` (`run.sh:289`) calls
  `assert_case`, the *project* helper, so there is no task-case negative control anywhere. The new
  helper wraps `assert_task_case`, and the **two existing task cases gain controls too** — they have
  none today.

### 4.7 What the CI-traversal cases actually prove — stated, not assumed

Per F4 the query is not equal to `moon ci`'s selection. The relationship is documented in `run.sh`
alongside the helper:

```
moon ci RunTask set = (query-affected ∩ T array ∩ runInCI) ∪ upstream-dep closure
```

Both differences are benign for these cases — the `T` filter can only *remove* tasks the cases do not
assert (`build-release`), and the upstream-dep closure can only *add* builds — but they are recorded
as a **measured assumption re-checked on every moon bump**, joining A4's `inputFiles` shape and A5's
`command`/`args`/`script` shape in the README's existing "a moon upgrade breaks this" list.

A per-CI-run assertion grounded in a real `moon ci` was considered and rejected on cost: there is no
dry-run in moon 2.3.2 (`--plan`, `--no-actions` and `--cache` do not suppress execution), so the gate
would have to actually run tasks on a cold CI cache. Instead the real run is a **one-time acceptance
step** (§6.4) plus a re-measurement step on moon bumps.

### 4.8 Landing order

The naive order breaks the repo mid-change: if `.moon/tasks/rust.yml` gains `@group(upstreams)`
before every crate declares it, graph load fails for **every** moon command, `run.sh` aborts rc 2,
and the first visible symptom is `ci.yml`'s earlier `moon run ts:commitlint` step failing with
`project::unknown_file_group` — confusing and unbisectable.

One PR, in this order:

1. `fileGroups.upstreams` in all thirteen `rs/crates/*/*/moon.yml` **and** the template (inert —
   nothing consumes the group yet).
2. `@group(upstreams)` into `.moon/tasks/rust.yml`'s `build`/`test`/`lint`.
3. Gate changes (A6, the task-case modes, the new case, the negative controls).
4. `ci.yml` cache-key discriminator (§5).
5. Docs.

### 4.9 Template — `.moon/templates/rust/moon.yml`

The scaffold emits `id`/`layer`/`language` and, for the service archetype, `dependsOn`. It emits no
`fileGroups`, so under §3.1 a generated crate hard-fails graph load repo-wide — total, not localized,
and on exactly the path a new contributor takes. The template gains `fileGroups.upstreams` (`[]` for
libs, the `paigasus-proto-rs` + `paigasus-kernel-rs` closure for services).

The service archetype also emits no `tasks.build/test deps: ['^:build']` today, which A3 would red on
the first generated service. Fixed in the same pass.

## 5. Cost

Measured on the full cascade (`moon run paigasus-kernel-rs:build --downstream deep`, macOS, warm
46 GB `rs/target`): **458s wall, 1399s CPU across 42 tasks**, with `paigasus-iam-rs:test` at 382s on
the critical path.

That number does not transfer to CI, and the pre-review draft's "~20m" leaned on it too heavily:

- `ubuntu-latest` is **4 vCPU**. 1399s of CPU is a ~350s floor before any serialization.
- `rs/.config/nextest.toml` sets `retries count = 2` with exponential backoff to 60s for every
  `paigasus-iam` integration target, and warns that three attempts of a genuinely failing run is
  ~18 minutes against the 30-minute budget.
- **Disk, not just time.** `ci.yml` reclaims ~15 GB because this repo has already died with
  `No space left on device` mid-link. This change makes ~9 crates' test binaries *link* in one run
  where previously only clippy metadata was produced.

**Cache key — required, not optional.** The `actions/cache` primary key for `rs/target` hashes only
`rs/rust-toolchain.toml`, `rs/Cargo.lock` and `rs/Cargo.toml`, none of which this change touches, and
`actions/cache` **skips its save on a primary-key hit**. Without a new discriminator the enlarged
`rs/target` is never written back and every subsequent kernel PR rebuilds the newly-scheduled crates
cold, forever — the precise trap `ci.yml` already documents for `-lint-deps-` and SMA-526. A literal
`-upstream-inputs-` segment is added to `key:` and both `restore-keys:` prefixes, keeping the
toolchain hash and `-line-tables-only-` intact.

**Acceptance threshold and rollback.** The implementing PR reports its own CI wall time and `df -h`.
If it exceeds **25 minutes** or shows disk pressure, the job is split or the timeout raised *before*
merge rather than after. The rollback unit is `@group(upstreams)` on `test` alone — removing it
leaves `build`/`lint` coverage, A6 self-consistent (its per-task loop is parameterized by task name),
and the CI-traversal cases re-baselined.

**Staged rollout was proposed by review and rejected.** Landing `build`+`lint` first and `test` a
week later is the option explicitly weighed and declined at design time in favour of full coverage;
re-introducing it would reverse a decision already made. The acceptance threshold and rollback unit
above address the same risk without deferring the issue's headline fix.

**Docker exposure is accepted with eyes open.** The container suites move from "iam PRs" to "kernel,
proto, logging, observability, service-info, iam-core and iam PRs", and `nextest.toml` documents them
as load-sensitive with a `max-threads = 8` cap that is a no-op on 4 vCPU. The SMA-521 retry budget
absorbs the known flake modes; the acceptance threshold is what catches it if the flake rate at the
new frequency turns out worse than assumed.

## 6. Verification

0. **Red before green.** `kernel->consumer-tasks` is written **first**, with a hand-derived expected
   set, and must **fail on unmodified `main`** naming the missing consumer tasks. Only then is the
   fix applied. The two re-baselined cases are likewise derived from intent and diffed against
   observed output, with any difference explained. The failure mode of this whole guard family is
   "snapshot whatever moon printed"; this step is what prevents it.
1. **Gate self-tests** — `ci/affected-graph/run.sh --negative-control` reports red for every new
   synthetic violation, including the new task-case controls.
2. **Query-level** — `echo "rs/crates/libs/paigasus-kernel/src/lib.rs" | moon query tasks --affected`
   lists `paigasus-kernel-parity-rs:test`, `paigasus-iam-rs:test`, `paigasus-iam-core-rs:test` and
   `paigasus-gateway-rs:test`.
3. **Cache correctness** — editing a kernel source changes a consumer's task hash (the §3 primary
   justification). Checked directly, since F5 shows hashes can silently fail to move.
4. **Real run** — `moon ci "${T[@]}" --stdin --include-relations` over a kernel source diff produces
   `RunTask` actions for those four projects in `.moon/cache/ciReport.json`. The issue asks for a
   real run at the real shape, and probes K/L are the pre-fix baseline it is compared against.
5. **Full graph** — the repo-level gates per CLAUDE.md, since this touches `.moon/tasks/rust.yml`
   and therefore schedules the entire Rust graph.

## 7. Documentation

- **`ci/affected-graph/README.md`** — document A6; restate what the project cases prove; document the
  two task-case traversal modes and §4.7's invariant; add `upstreams` to the maintenance section (a
  new crate must declare it; adding an in-tree dep changes the consumer's closure); correct the "the
  edges are inert without it" claim, which appears here as well as in `run.sh`.
- **`CLAUDE.md`** — add the missing half of the existing `^:build` bullet:

  > Task `inputs` are the **only** thing that confers affectedness in Moon 2.3.2. `dependsOn` and a
  > task-level `^:build` schedule an upstream's build but never **select** a downstream — a
  > dependent runs only if independently affected, and `--include-relations`/`--downstream` do not
  > change that for `moon ci` (measured at the full 24-target shape, SMA-528). A new crate declares
  > its transitive upstreams in `fileGroups.upstreams`; omitting it is a hard graph-load error, and
  > mis-declaring it reds `repo:affected-smoke` A6 — nothing else can, since a crate's `moon.yml` is
  > not an input to its own tasks.

  Also note that `^:build` acquires a second purpose here: once a downstream keys on
  `paigasus-proto/src/generated/**`, which `contracts:generate` writes, `^:build` is what orders
  generation before the downstream's hash is computed. Removing it as "vestigial" would introduce
  nondeterministic cache keys on the proto chain.
- **`rs/crates/services/paigasus-iam/moon.yml` and `ci/error-registry/README.md`** — both state that
  a contracts change already schedules the service crates' membership tests via `test:
  deps: ['^:build']`. Per F1 that is **false today** and becomes true only with this change. The
  claim is corrected and marked as now load-bearing.

## 8. Rejected review findings

Recorded so the reasoning is visible rather than silently dropped:

- **Staged rollout (build+lint now, test later)** — reverses an explicit design-time decision for
  full coverage. Mitigated instead by §5's acceptance threshold and rollback unit.
- **Default `upstreams: []` in `.moon/tasks/rust.yml`** — would trade the permanent hard-fail for
  transient landing convenience; worse given F5. See §3.1.

## 9. Out of scope

- **Upstream bug report to moonrepo.** F2 contradicts Moon's documented behaviour at the real CI
  shape and is worth reporting, but it is not a prerequisite: this fix is correct regardless, and
  would remain correct (merely redundant) if Moon later made `--downstream` work. A follow-up Linear
  issue is filed for the report itself.
- **`buf.gen.yaml`-only codegen changes** (§3.2) — a follow-up; the closure covers committed
  generated sources, not the generator's configuration.
- **Cross-stack input coverage.** §8 of the pre-review draft claimed the py/ts wrapper tasks are
  "covered by A5". That is **false**: `FFI_TASK_INPUTS` is `rs/Cargo.lock`, `rs/Cargo.toml`,
  `rs/rust-toolchain.toml`, `.prototools` — it says nothing about the hand-written
  `/rs/crates/libs/paigasus-kernel/src/**/*` globs in `ts/packages/paigasus-kernel/moon.yml` and
  `py/packages/paigasus-kernel/moon.yml`, which are the ADR-0005 cross-binding guarantee and are
  asserted by nothing. Closing it properly means extending A6's closure check across stacks (any
  project with a `dependsOn` on a Rust crate must key on that crate's sources). **Deferred to a
  follow-up issue, not silently assumed away** — this PR is already at the edge of a safe change
  size, and the fix has its own cost profile in the ts/py stacks.
- **Splitting CI into parallel jobs.** Triggered by §5's acceptance threshold if needed.
