# SMA-546 — Workspace-level inputs for the FFI build tasks

**Status:** approved
**Linear:** [SMA-546](https://linear.app/smaschek/issue/SMA-546)
**Related:** SMA-534 (the Rust-`lint` half of this shape, and the spec that recorded this gap as
accepted residual risk), SMA-524 (Cargo↔Moon parity gate — the "a *missing case* is how the bug
survived" lesson), SMA-409/429 (affected-graph guard), SMA-520 (Actions spend; `prebuild.yml`
path filters), SMA-427/420/419 (the wasm/napi/PyO3 bindings these tasks build)

## Problem

SMA-534 made a `rs/Cargo.lock` change schedule all thirteen crate `lint` tasks. `cargo clippy`
emits metadata — it **never links** — and it runs on the **host target only**, so
`wasm32-unknown-unknown` is never compiled at all.

Three crates are `crate-type = ["cdylib"]`, and for them *linking* is the failure mode:

* `rs/crates/bindings/paigasus-py-bindings` (PyO3)
* `rs/crates/bindings/paigasus-node-bindings` (napi-rs)
* `rs/crates/bindings/paigasus-wasm` (wasm-bindgen)

So SMA-534's fix covers "Rust source compiles and lints", not "a dependency bump is safe".

### The scenario that still merges green and reds `main`

Dependabot bumps `wasm-bindgen` 0.2.z. `rs/Cargo.toml:90-97` records an INVARIANT: the
proto-pinned `wasm-pack` must support that exact 0.2.z, because crate↔CLI schema compatibility is
exact per 0.2.z. After SMA-534, all thirteen lints still go green, because:

1. Clippy neither links the cdylib nor targets wasm32.
2. `paigasus-kernel-ts:{build,test}` and `paigasus-kernel-py:test` are the only tasks that run
   `wasm-pack` / `napi build` / maturin. They declare `/rs/crates/**` inputs but **not** the
   lockfile, so Moon replays a cached green.
3. `.github/workflows/prebuild.yml:25-39` deliberately excludes `rs/**` from its `pull_request`
   trigger — the cross-build matrix runs on push-to-`main` only. It also builds **only the napi
   addon**: no wasm, no wheel. It is not a coverage path for this scenario even when it does run.

That is the same merges-green/reds-`main` shape SMA-534 exists to close, surviving for the FFI
third of the workspace.

### It is independently a cache-correctness bug

Those three tasks compile Rust but do not key on the resolution that Rust is compiled against. A
lockfile change therefore leaves their task hash unchanged and Moon replays a **cached artifact
built from a different dependency graph**. That is wrong regardless of what one decides about
affectedness.

## Decision

Add the three workspace-level files that `lint` already keys on to the three tasks that compile
the FFI cdylibs:

| task | what it compiles |
|---|---|
| `ts/packages/paigasus-kernel:build` | `napi build` + `wasm-pack build --release` |
| `ts/packages/paigasus-kernel:test` | same, into its own scratch out-dir |
| `py/packages/paigasus-kernel:test` | `uv sync --reinstall-package` → maturin |

Inputs added to each (workspace-relative, matching `WORKSPACE_LINT_INPUTS`):

```
/rs/Cargo.lock
/rs/Cargo.toml
/rs/rust-toolchain.toml
```

### The scheduling is not a separate choice from the cache fix

Moon has **no hash-only input**: `inputs` feed the task hash *and* task affectedness. The
cache-correctness bug above therefore cannot be fixed without also getting the scheduling. The two
halves of this issue are one edit, and the cost below is not optional overhead attached to a
cheaper fix — it is the price of a correct cache key.

This is also why the issue's cheaper alternative (`/rs/Cargo.lock` on the three binding crates'
`build`) is not a partial version of this decision: it leaves the cache-correctness bug fully
intact *and* misses wasm32, the motivating case. See Rejected alternatives.

### Why `rs/rust-toolchain.toml` too, when the issue names only two files

The issue suggests `Cargo.lock` + `Cargo.toml`. This spec adds the toolchain file as well, for
three reasons:

1. **It is load-bearing for exactly these tasks.** `ts/packages/paigasus-kernel/moon.yml` runs
   `wasm-pack` from *inside* the crate dir specifically so `rs/rust-toolchain.toml`'s `1.95.0`
   override selects the compiler rather than rustup's default. A toolchain change changes the
   emitted artifact.
2. **It makes the set identical to `WORKSPACE_LINT_INPUTS`**, so the A5 guard below reuses that
   tuple verbatim instead of introducing a second, drifting list.
3. **It costs nothing.** `.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}` is an implicit input of every
   task, and a correct toolchain bump also touches `.moon/toolchains.yml` — which already
   reschedules these tasks. This input catches the bump that drifts the two files apart, the same
   defence-in-depth argument SMA-534 made for `lint`.

### What is deliberately not touched

`paigasus-kernel-py:build` runs `uv build` over the pure-Python wrapper package. It compiles no
Rust and gets nothing.

## Cost

Measured on the development machine (Apple silicon, warm `~/.cargo/registry`, `rs/target` emptied
with `cargo clean` run from `rs/`, Moon state cache cleared), following SMA-534's method: the
**actual scheduled workload**, not a synthetic single-crate build.

| | wall | CPU | `rs/target` | Moon actions |
|---|---|---|---|---|
| today — `moon run :lint` (what a Dependabot Cargo PR schedules after SMA-534) | 1m 40s | 4m 44s user + 41s sys | 3.1 GB | 24 |
| **added by this change** | **+21s** | **+14s user + 2s sys** | **+0.4 GB** | +7 |

The baseline reproduces SMA-534's recorded figure (1m 47s wall, 3.1 GB) closely enough that the
two are comparable — the delta is measured against the same workload that spec priced.

Two properties of this measurement matter:

* The delta was measured **on the warm `rs/target` that `:lint` leaves behind**, which is the real
  CI situation (lint and these tasks share one target dir in one `moon ci` run). It is a marginal
  cost, not a second cold build.
* It is a **+21% wall / +5% CPU** increase on a workload that already exists — not a new workload.

The issue's framing ("a `wasm-pack` **release** build plus a `napi build` on every Dependabot Cargo
PR") sounds more expensive than it measures. Both binding crates are tiny and their dependency
trees are small beside `cedar-policy`, which dominates the existing `:lint` figure. Against
SMA-520's spend cut this is affordable.

`ci.yml:22` sets `timeout-minutes: 30`; the figures above are the no-restore worst case, and CI
additionally sets `CARGO_PROFILE_{DEV,TEST}_DEBUG: line-tables-only` (SMA-444), so its `rs/target`
is materially smaller. Both numbers should be re-confirmed on CI during verification rather than
extrapolated from a dev machine.

### The CI cache key needs no change

This is the SMA-520 failure mode — widening what a cached job builds *without* changing its key
means `actions/cache` skips its post-job save on an exact primary-key hit, and the new output is
rebuilt cold on every run, forever. It does **not** apply here, and the reasoning is recorded so
nobody has to re-derive it:

`ci.yml:102` keys on `hashFiles('rs/rust-toolchain.toml')` and `hashFiles('rs/Cargo.lock',
'rs/Cargo.toml')`. All three files this change adds as inputs are therefore already in the primary
key. Any change that *newly* schedules these tasks is, by construction, a change to one of those
three files — so it always rotates the key, always misses, and always saves.

Steady state is unchanged: on an ordinary Rust-source PR exactly the same tasks run as before, and
this change adds no new artifact kind to `rs/target` that was not already produced there.

## Guard

A fix without a guard reopens; that is the SMA-409/429/524/526/534 pattern. Two layers, matching
the existing split — `run.sh` holds hand-written *behavioural* cases, `cargo_moon_parity.py` holds
*generic* assertions.

### Layer 1 — behavioural: re-baseline `lockfile->all-lint`

The existing case in `ci/affected-graph/run.sh` is strict-equality, default-deny over tasks named
`build`/`test`/`lint`. These three tasks are named `build` and `test`, so they enter its observed
set the moment the inputs land. Its expected set gains exactly three rows:

```
paigasus-kernel-py:test,paigasus-kernel-ts:build,paigasus-kernel-ts:test
```

The case's existing comment already names these three tasks and instructs the reader to add them
here when this happens. Re-baselining it turns a prediction into a record; the comment is rewritten
so it describes the new state rather than the anticipated one.

The case's name is no longer accurate (`all-lint` now includes three non-lint rows) and is renamed
to `lockfile->lint+ffi`.

This layer proves the inputs take **effect**. What it cannot see: it only ever touches
`rs/Cargo.lock`, so dropping `/rs/Cargo.toml` or `/rs/rust-toolchain.toml` from any of the three
tasks leaves it green. That is Layer 2's job.

### Layer 2 — generic assertion **A5**, in `ci/affected-graph/cargo_moon_parity.py`

A4 asserts every *Rust crate's* `lint` keys on `WORKSPACE_LINT_INPUTS`. A5 is its cross-stack twin:
**every task that compiles the FFI cdylibs must key on the same three files.**

A5 **derives its own target list** rather than hand-listing the three tasks. `moon query projects`
exposes each task's resolved `command` and `script` (verified on moon 2.3.2 — the task object's
keys include `command`, `script`, `args`, `inputFiles`, `inputGlobs`). A5 scans that text for the
markers that mean "this task shells out to a Rust build from a non-Rust project":

```
napi build      wasm-pack      maturin      --reinstall-package
```

Measured against the live graph, those markers match **exactly**
`paigasus-kernel-ts:build`, `paigasus-kernel-ts:test` and `paigasus-kernel-py:test`, and nothing
else among the repo's 118 tasks across 28 projects. Every other cargo/uv-invoking task is either a
Rust-project task
(covered by A4, and deliberately excluded from `build`/`test` by SMA-534) or a `repo:` gate.

Deriving is the point: SMA-524's lesson is that a *missing case* is how a graph bug survives a full
review cycle. A hand-written three-entry table would leave a future fourth FFI task — a new binding
language — silently unguarded. A5 covers it the day it is added.

A5 reuses A4's two hard-won rules verbatim:

* It reads Moon's **resolved** `inputFiles`, never `moon.yml` — the gate's "never parse YAML"
  invariant.
* An **absent** `inputFiles` key is a violation, never a skip. "Moon told us nothing" and "Moon told
  us there are none" are different defects, and the first must fire loudly rather than pass
  vacuously.

It also gains a `--self-test` row, like A1–A4: a gate whose whole value is catching a silent hole
must not be able to pass vacuously (SMA-524 D6). The self-test must cover **both** A5 failure
directions — a matched task missing a required input, and the absent-`inputFiles` case.

### Why no new `assert_case` (project-level) row is needed

`rs/` has no Moon project, so `rs/Cargo.lock` is owned by `repo` (source `.`). Task affectedness
flows through task **inputs** independently of project membership: adding a workspace-relative input
makes the *task* affected while `moon query projects --affected` still reports `repo` alone. SMA-534
measured this directly for the `lint` case, and the same mechanism applies here because the input
form is identical.

Consequence: no existing `assert_case` changes. This must be **verified empirically** during
implementation, not assumed — see Verification V2.

## Rejected alternatives

**`/rs/Cargo.lock` on the three binding crates' `build` only** (the issue's cheaper option). It
catches host link breakage for the PyO3/napi cdylibs, but it does not compile wasm32 and never runs
`wasm-bindgen-cli`, so it misses the motivating scenario entirely. It also leaves the
cache-correctness bug on the ts/py tasks untouched — and fixing *that* re-adds this decision's full
cost anyway, so the saving is illusory rather than a genuine trade.

**A dedicated minimal FFI gate** — a new repo-scoped task running host `cargo build -p` for the two
cdylibs plus `wasm-pack build --dev` for the schema check. Cheaper in isolation and it does cover
all three failure modes. Rejected because it does not fix the cache-correctness bug either (the
ts/py tasks would still replay stale artifacts), so it would have to be *added to* this change
rather than replace it. It would also introduce a second wasm profile into the shared `rs/target`,
and SMA-526 measured what divergent profiles do to that directory: they evict each other.

**Adding `rs/Cargo.lock` + `rs/Cargo.toml` to `prebuild.yml`'s `pull_request` paths.** Strictly
worse on both axes: it puts six cross-build legs, including macOS and Windows runners, on every
Dependabot Cargo PR — far more expensive than 21 seconds — while still not covering wasm or the
wheel, because that workflow builds only the napi addon. SMA-520 removed `rs/**` from this trigger
deliberately.

**A hand-written three-entry table for A5.** See Layer 2 — it reproduces the SMA-524 failure mode
for the next FFI task.

## Verification

**V1 — the guard suite is green and correctly re-baselined.** `bash ci/affected-graph/run.sh` passes
with the new expected set, and `bash ci/affected-graph/run.sh --negative-control` passes, including
the new A5 self-test rows. Baseline before the change: all twelve assertions green (captured during
design).

**V2 — the project graph is measurably unaffected.** `moon query projects --affected --downstream
deep` on `rs/Cargo.lock` still returns `repo` alone, so no `assert_case` needs re-baselining. This
is asserted by the suite's strict equality, but must also be observed directly, because the claim
"task inputs do not create project edges" is the load-bearing premise of the whole design.

**V3 — the scheduling actually changes.** `moon query tasks --affected` on `rs/Cargo.lock` returns
the thirteen lints **plus** the three FFI rows; before the change it returns thirteen.

**V4 — prove the guard bites.** The decisive test, and the one this issue exists for: pin
`wasm-bindgen` to a 0.2.z the proto-pinned `wasm-pack` cannot process, touch only the lockfile, and
confirm `paigasus-kernel-ts:build` **reds** where it previously replayed a cached green. Then
revert. A guard asserted only in the passing direction reproduces the exact bug being closed
(SMA-489, SMA-524 D6).

Note the mtime hazard when reverting an experimental edit: restoring a file via `mv file.bak file`
rolls its mtime *backwards*, and cargo then reuses the artifact built from the temporary edit. Revert
with an editor write (or `touch`), never a `.bak` move.

**V5 — the full repo gate graph.** Per CLAUDE.md, run the whole `moon ci` gate list with `--base
origin/main --include-relations` before pushing, not just the per-project tasks. `repo:affected-smoke`
is the gate that carries Layers 1 and 2.

**V6 — confirm the cost on CI.** Read the actual `moon ci` wall time on the PR rather than
extrapolating the dev-machine figures above.

## Scope

In scope: the three `moon.yml` input lists, the `run.sh` case re-baseline and rename, A5 plus its
self-test, and the spec.

Out of scope, and deliberately so:

* **`repo:parity-corpus-drift`** runs `cargo run -p paigasus-kernel-parity` but keys on no lockfile
  — a cousin of this gap. A5's markers do not match it, and widening them to `cargo run` / `cargo
  tree` would demand `rust-toolchain.toml` on grep-only `repo:` gates that never invoke a compiler.
  Noted rather than silently swept in; file separately if it matters.
* **`prebuild.yml` triggers** stay as SMA-520 set them.
* **The duplicated `napi build` + `wasm-pack` work between `paigasus-kernel-ts:build` and
  `:test`** is pre-existing and load-bearing (SMA-427 gave them separate out-dirs to fix a CI race).
  Not consolidated here.

## Rollback

Revert the three `moon.yml` input additions, the `run.sh` expected set and name, and A5. Nothing
else depends on them; there is no data migration and no published artifact. The pre-change state is
the SMA-534 state, whose residual risk is documented.
