# SMA-546 — Workspace-level inputs for the FFI build tasks

**Status:** approved
**Linear:** [SMA-546](https://linear.app/smaschek/issue/SMA-546)
**Related:** SMA-534 (the Rust-`lint` half of this shape, and the spec that recorded this gap as
accepted residual risk), SMA-524 (Cargo↔Moon parity gate — the "a *missing case* is how the bug
survived" lesson), SMA-409/429 (affected-graph guard), SMA-520 (Actions spend; `prebuild.yml`
path filters), SMA-427/420/419 (the wasm/napi/PyO3 bindings these tasks build), SMA-444 (runner
disk exhaustion)

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
   `wasm-pack` / `napi build` / maturin *and rebuild on demand*. They declare `/rs/crates/**`
   inputs but **not** the lockfile, so Moon replays a cached green.
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

Add the workspace-level files that determine what these tasks compile, to the three tasks that
compile the FFI cdylibs.

| task | what it compiles |
|---|---|
| `ts/packages/paigasus-kernel:build` | `napi build` + `wasm-pack build --release` |
| `ts/packages/paigasus-kernel:test` | same, into its own scratch out-dir |
| `py/packages/paigasus-kernel:test` | `uv sync --reinstall-package` → maturin |

Inputs added to each:

```
/rs/Cargo.lock
/rs/Cargo.toml
/rs/rust-toolchain.toml
/.prototools
```

### The scheduling is not a separate choice from the cache fix

Moon has **no hash-only input**: `inputs` feed the task hash *and* task affectedness. The
cache-correctness bug above therefore cannot be fixed without also getting the scheduling. The two
halves of this issue are one edit, and the cost below is not optional overhead attached to a
cheaper fix — it is the price of a correct cache key.

This is also why the issue's cheaper alternative (`/rs/Cargo.lock` on the three binding crates'
`build`) is not a partial version of this decision: it leaves the cache-correctness bug fully
intact *and* misses wasm32, the motivating case. See Rejected alternatives.

### `--locked` was investigated and deliberately deferred

An earlier draft of this spec added `--locked` to all three builds, reasoning from SMA-534: without
it cargo silently re-resolves and rewrites an inconsistent `Cargo.lock`, so nothing proves the
artifact came from the resolution the PR ships — and `rs/Cargo.lock` is now a *declared input* of
these tasks, so a mid-run rewrite would both invalidate the hash Moon recorded before execution and
race `repo:deny` (`moon.yml:23`), `repo:wasm-getrandom-free` (`moon.yml:250`) and
`repo:nats-permissions` (`moon.yml:229`), which read that same file in the same `moon ci` graph.

The reasoning was sound; the mechanism was not. Measured on this worktree, `--locked` on these
tasks **cannot deliver that guarantee**, for two independent reasons:

1. **The tools pre-resolve.** `napi build` and `wasm-pack build` each run an **un-flagged
   `cargo metadata`** before their `cargo build`. That call re-resolves and rewrites the lockfile,
   so the subsequent `cargo build --locked` always finds it fresh and can never fail. Proof: with
   the lock made genuinely stale, the real `wasm-pack build … -- --locked` invocation **exited 0 and
   rewrote `rs/Cargo.lock`**, while a plain `cargo build --locked` control correctly refused with
   `cannot update the lock file … because --locked was passed`. Sending a bogus flag through the
   same channel *does* prove the arguments reach cargo — but reaching cargo is not the same as
   biting, and only the staleness experiment distinguishes them.
2. **The graph pre-resolves.** Even with a task-local `cargo metadata --locked` fail-fast guard
   (which does work in isolation — verified in both directions), the shared `build` task in
   `.moon/tasks/rust.yml` is a plain unflagged `cargo build`, inherited by all thirteen crates and
   scheduled **ahead** of these tasks by `deps: ['^:build']`. It rewrites a stale lock itself, so
   whether any task-local guard fires is order-dependent.

The root cause is therefore repo-wide, not a property of these three task scripts. Closing it means
`--locked` on the shared Rust `build` task — and arguably on `test`, which is `cargo nextest run` —
which is a change to all thirteen crates with its own cost, ergonomics and guard questions.

**Deferred to a follow-up issue**, carrying the measurements above. This spec claims only what it
delivers: the four inputs, which fix the cache-correctness bug and the scheduling gap. Shipping a
half-mechanism under a comment asserting the hazard was closed would recreate exactly the
documented-vs-executed split SMA-521 closed.

### Why `rs/rust-toolchain.toml` and `.prototools`, when the issue names only two files

The issue suggests `Cargo.lock` + `Cargo.toml`. Two more files are load-bearing for *exactly these
tasks*, and the INVARIANT this issue is built on is bidirectional — "bump the two together":

* **`/rs/rust-toolchain.toml`** — `ts/packages/paigasus-kernel/moon.yml` runs `wasm-pack` from
  *inside* the crate dir specifically so this file's `1.95.0` override selects the compiler rather
  than rustup's default. It also makes the Rust-side set identical to `WORKSPACE_LINT_INPUTS`, so
  the A5 guard reuses that tuple instead of introducing a second, drifting list.
* **`/.prototools`** — pins `wasm-pack = "0.15.0"` (`.prototools:15`), i.e. the **other half** of
  the `rs/Cargo.toml:90-97` INVARIANT. Adding only the lockfile would guard the wasm-bindgen side
  and leave the wasm-pack side structurally unenforceable. `prebuild.yml:29` already lists
  `.prototools` as a pull-request path filter for precisely this reason.

Both are cheap: they are rarely touched, and a toolchain bump already reschedules these tasks via
the `.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}` implicit input. They catch the bump that drifts the
files apart.

**`rs/.cargo/config.toml` is deliberately excluded.** It supplies the `-undefined dynamic_lookup`
flags the PyO3/napi cdylibs need on macOS only; CI is Linux, where it is inert. It is listed here so
the enumeration above is not mistaken for exhaustive.

### What is deliberately not touched

`paigasus-kernel-py:build` runs `uv build` over the pure-Python wrapper package. It compiles no
Rust.

The **`py` configuration root's** whole-tree tasks (`py:test`, `py:lint`, `py:fmt`, `py:typecheck`,
`.moon/tasks/python.yml`) do cause `uv run` to sync the workspace, which on a *cold* environment
builds the PyO3 cdylib. They are nonetheless **not** a coverage path, and this was measured rather
than assumed:

> With `paigasus_kernel::sum` changed to return `a + b + 1000`, plain `uv run pytest` over
> `packages/paigasus-kernel/tests` reported **124 passed**, having served a cached wheel. The same
> edit made `paigasus-kernel-py:test` — which passes `--reinstall-package` — report **67 failed,
> 57 passed**.

So `uv run` alone cannot observe a Rust change; `--reinstall-package` is what defeats the cache,
which is why that flag exists (SMA-420 spike S4). Adding workspace inputs to the `py:*` root tasks
would re-run pytest against the *same cached wheel* — cost with no coverage. They are out of scope,
and A5 must not match them.

That measurement also answers the converse question: `--reinstall-package` **does** force a rebuild
against a changed Rust tree, so the py third of this change buys a real rebuild and not merely a
cache-key fix.

## Cost

Measured on the development machine (Apple silicon, warm `~/.cargo/registry`, `rs/target` emptied
with `cargo clean` run from `rs/`, Moon state cache cleared), following SMA-534's method: the
**actual scheduled workload**, not a synthetic single-crate build.

The decisive figure is the last row — a real `moon ci` over a **synthetic lockfile-only commit**,
which is exactly the Dependabot Cargo PR shape this issue is about, with the whole graph
interleaved rather than run in sequence:

| | wall | CPU | `rs/target` | Moon actions |
|---|---|---|---|---|
| today — `moon run :lint` (what such a PR schedules after SMA-534) | 1m 40s | 4m 44s user + 41s sys | 3.1 GB | 24 |
| sequential estimate of the addition | +21s | +14s user + 2s sys | +0.4 GB | +7 |
| **after — cold `moon ci :build :test :lint --base <lockfile-only commit> --include-relations`** | **2m 06s** | **5m 10s user + 47s sys** | **3.5 GB** | **27** |

The baseline reproduces SMA-534's recorded figure (1m 47s wall, 3.1 GB) closely enough that the two
are comparable. **The real interleaved cost is +26s wall and +0.4 GB** — close to the sequential
estimate, so the three concerns raised while pricing this all resolved benignly:

1. **Concurrency** did not bite. The interleaved run came in 5s above the sequential estimate, not
   at CPU-time. The two `wasm-pack` invocations do serialize on cargo's target-dir lock
   (`ts/packages/paigasus-kernel/moon.yml:109-110`), and that is already inside the 2m 06s.
2. **`touch` interference** did not materialise as a measurable rebuild penalty, even though all
   three scripts `touch` the kernel and binding sources while the clippy runs build the same crates
   via `^:build`.
3. **The profile split is a non-issue on disk.** `CARGO_PROFILE_{DEV,TEST}_DEBUG: line-tables-only`
   (`ci.yml:28-29`) does not apply to `wasm-pack build --release`, so that portion does not shrink
   on CI — but measured, the entire `rs/target/wasm32-unknown-unknown` tree is **7.7 MB**. Against
   SMA-444's disk-exhaustion history this is noise; the +0.4 GB is host-target artifacts, which CI's
   debuginfo trim does shrink.

`ci.yml:22` sets `timeout-minutes: 30`, so 2m 06s cold leaves ample headroom. These are no-restore
worst-case figures: a real Dependabot PR misses the exact cache key and restores the most recent
`main` entry via `restore-keys`, so only the bumped dependency and its dependents recompile.

### A cost that lands on precisely the motivating PR

`rs/Cargo.toml:93` records that wasm-pack "fetches the matching wasm-bindgen-cli for whatever 0.2.z
this caret resolves to". This change puts that fetch on the critical path of every Dependabot PR
that moves wasm-bindgen — the one PR shape this issue exists to cover. `ci.yml:82-85` caches
`~/.cargo/registry`, `~/.cargo/git` and `rs/target`; wasm-pack's own cache dir is cached by nothing.

Measured: the fetch is a **prebuilt-binary download**, not a `cargo install` compile. On the cold
`moon ci` run above, each of the two wasm-pack invocations spent about **8.2s** total including the
`⬇️ Installing wasm-bindgen...` step, against sub-second once warm — so roughly 16s of the 2m 06s,
already inside the figure quoted in Cost. The multi-minute `cargo install wasm-bindgen-cli` fallback
exists only when no prebuilt binary matches that 0.2.z, which is the case worth watching on a
genuine version bump. Adding wasm-pack's cache dir to `actions/cache` is out of scope here and would
need its own key discriminator per SMA-520.

### The CI cache key needs no change

This is the SMA-520 failure mode — widening what a cached job builds *without* changing its key
means `actions/cache` skips its post-job save on an exact primary-key hit, and the new output is
rebuilt cold on every run, forever. It does **not** apply here:

`ci.yml:102` keys on `hashFiles('rs/rust-toolchain.toml')` and `hashFiles('rs/Cargo.lock',
'rs/Cargo.toml')`. Three of the four files this change adds are therefore already in the primary
key, and any change that *newly* schedules these tasks is by construction a change to one of them,
so it always rotates the key, always misses, and always saves.

The load-bearing sentence, which a future reader should not have to re-derive: **this change adds no
new artifact *kind* to `rs/target`** — the same napi/wasm32 outputs were already produced there
whenever a Rust source PR scheduled these tasks. The merge of *this* PR touches only `moon.yml` and
`ci/` files, so its primary key hits exactly and its post-job save is skipped; because no new
artifact kind exists, that is a one-time discarded build rather than SMA-520's permanent cold
rebuild.

Two caveats stated precisely rather than glossed:

* `.prototools` is **not** in the cache key. A wasm-pack bump therefore schedules these tasks
  without rotating the key. It also produces no `rs/target` artifact of its own (the fetched CLI
  lives in wasm-pack's own cache dir), so the SMA-520 trap still does not bite — but this is the one
  input where the reasoning differs, and it is why the sentence above is about artifact kinds rather
  than about key rotation alone.
* "Steady state is unchanged" is true only for Rust-*source* PRs. A PR editing solely `rs/Cargo.toml`
  — a feature flip, or adding a workspace dep consumed by a later commit — now newly runs all three
  FFI tasks. It rotates the key, so no trap, but the widening is real.

## Guard

A fix without a guard reopens; that is the SMA-409/429/524/526/534 pattern. Two layers, matching the
existing split — `run.sh` holds hand-written *behavioural* cases, `cargo_moon_parity.py` holds
*generic* assertions.

### Layer 1 — behavioural: re-baseline `lockfile->all-lint`

The existing case in `ci/affected-graph/run.sh:257` is strict-equality, default-deny over tasks named
`build`/`test`/`lint`. These three tasks are named `build` and `test`, so they enter its observed set
the moment the inputs land. Its expected set gains exactly three rows:

```
paigasus-kernel-py:test,paigasus-kernel-ts:build,paigasus-kernel-ts:test
```

The case's existing comment already names these three tasks and instructs the reader to add them here
when this happens. Re-baselining turns a prediction into a record; the comment is rewritten to
describe the new state.

**The case is NOT renamed.** `lockfile->all-lint` is now a slight misnomer, but the name is
referenced by `CLAUDE.md:69`, `ci/affected-graph/README.md:44,55,85` and by historical plan
documents. A rename buys nothing functional and breaks greps, including the operational procedure a
contributor follows when adding a crate.

"Exactly three rows" is a prediction to be **verified empirically** before the CSV is written
(V3) — Moon's JS toolchain can synthesise implicit project edges from `package.json`, so the
observed set is measured, not reasoned about.

This layer proves the inputs take **effect**. What it cannot see: it only ever touches
`rs/Cargo.lock`, so dropping any of the other three files from any of the three tasks leaves it
green. That is Layer 2's job.

### Layer 2 — generic assertion **A5**, in `ci/affected-graph/cargo_moon_parity.py`

A4 asserts every *Rust crate's* `lint` keys on `WORKSPACE_LINT_INPUTS`. A5 is its cross-stack twin:
**every task that compiles the FFI cdylibs must key on the workspace files.**

A5 combines a **derived** set with a **required floor**:

```python
FFI_MARKERS = ("napi build", "wasm-pack", "maturin", "--reinstall-package")
REQUIRED_FFI_TASKS = ("paigasus-kernel-ts:build", "paigasus-kernel-ts:test", "paigasus-kernel-py:test")
FFI_TASK_INPUTS = WORKSPACE_LINT_INPUTS + (".prototools",)
```

* **Derived** — scan each task's resolved invocation for the markers. This covers a future fourth
  FFI task (a new binding language) on the day it is added, which is SMA-524's "a *missing case* is
  how a graph bug survives a full review cycle" lesson.
* **Floor** — every entry in `REQUIRED_FFI_TASKS` must appear in the derived set. Without it A5 can
  degrade to a **vacuous PASS**: moving an invocation behind a `package.json` script, changing
  `--reinstall-package` to `--refresh-package`, or a Moon upgrade renaming the `script` key all
  yield `matched = ∅` and a green gate. A4's inherited "absent `inputFiles` is a violation, never a
  skip" rule does *not* protect against this — when nothing matches, `inputFiles` is never consulted.
  This repo treats can-pass-vacuously gates as must-fix (SMA-524 D6, SMA-489).

**What exactly is scanned.** The concatenation of `command`, `args` and `script`. This is not
cosmetic: measured on moon 2.3.2, a command-form task reports `command='cargo'` with the verb in
`args` (`paigasus-kernel-rs:lint` → `args=['clippy', '--locked', …]`, `script=None`), while a
script-form task reports `command='touch'` with the real invocation in `script`. Scanning
`command`/`script` alone would miss a `command: 'napi'` + `args: ['build', …]` task entirely.

A task exposing **neither** a `command` nor a `script` key is an infrastructure error (rc 2, aborting
the guard), not an assertion failure — the same distinction A4 draws for an absent `inputFiles`, and
for the same reason: "Moon told us nothing" must not be reported as "the graph regressed".

`maturin` is a **forward-looking** marker. It matches nothing today — the string appears only in
`py/packages/paigasus-kernel/moon.yml` comments, and the resolved script is `uv sync
--reinstall-package paigasus-py-bindings`. It is retained so a future direct maturin invocation is
covered on day one; it is listed here so nobody mistakes it for measured coverage.

A5 reuses A4's rules verbatim: it reads Moon's **resolved** output, never `moon.yml` (the gate's
"never parse YAML" invariant).

**No exemption allowlist is added.** A2's `ALLOW_NO_CARGO_BACKING` is the pattern to copy if a
legitimate matched-but-exempt task ever appears, but the table would be empty today and this spec
declines to add dead machinery. `repo:parity-corpus-drift` — identified below as a cousin of this
gap — does not match the markers, so it needs no entry.

A5 gains `--self-test` rows like A1–A4, covering **three** directions: a matched task missing a
required input, the floor-violation case (a required task absent from the derived set), and the
absent-invocation infra case.

### Why no new `assert_case` (project-level) row is needed

`rs/` has no Moon project, so `rs/Cargo.lock` is owned by `repo` (source `.`). Task affectedness
flows through task **inputs** independently of project membership: adding a workspace-relative input
makes the *task* affected while `moon query projects --affected` still reports `repo` alone. SMA-534
measured this for the `lint` case, and the input form here is identical. Verified empirically in V2,
not assumed.

## Rejected alternatives

**`/rs/Cargo.lock` on the three binding crates' `build` only** (the issue's cheaper option). It
catches host link breakage for the PyO3/napi cdylibs, but it does not compile wasm32 and never runs
`wasm-bindgen-cli`, so it misses the motivating scenario entirely. It also leaves the
cache-correctness bug on the ts/py tasks untouched — and fixing *that* re-adds this decision's full
cost anyway, so the saving is illusory.

**A dedicated minimal FFI gate** — a new repo-scoped task running host `cargo build -p` for the two
cdylibs plus `wasm-pack build --dev`. Cheaper in isolation and it does cover all three failure modes.
Rejected because it does not fix the cache-correctness bug either, so it would have to be *added to*
this change rather than replace it; and it would introduce a second wasm profile into the shared
`rs/target`, which SMA-526 measured as mutually evicting.

**Adding `rs/Cargo.lock` + `rs/Cargo.toml` to `prebuild.yml`'s `pull_request` paths.** Strictly worse
on both axes: six cross-build legs including macOS and Windows on every Dependabot Cargo PR — far
more than 21 seconds — while still covering neither wasm nor the wheel, because that workflow builds
only the napi addon. SMA-520 removed `rs/**` from this trigger deliberately.

**A hand-written table for A5 with no derivation.** Rejected in favour of derived-∪-floor, which
keeps day-one coverage of a future FFI task while removing the silent-degradation mode a bare
derivation would have.

**Renaming the behavioural case.** See Layer 1.

## Verification

**V1 — the guard suite is green and correctly re-baselined.** `bash ci/affected-graph/run.sh` passes
with the new expected set, and `bash ci/affected-graph/run.sh --negative-control` passes, including
the three new A5 self-test rows. Baseline before the change: all twelve assertions green (captured
during design).

**V2 — the project graph is measurably unaffected.** `moon query projects --affected --downstream
deep` on `rs/Cargo.lock` still returns `repo` alone. Observed directly, because "task inputs do not
create project edges" is the load-bearing premise of the whole design.

**V3 — the scheduling actually changes, measured not predicted.** `moon query tasks --affected
--downstream deep` on `rs/Cargo.lock` returns the thirteen lints **plus** exactly the three FFI rows;
before the change it returns thirteen. The CSV is written from this output.

**V4 — prove the guard bites, end to end through `moon ci`.** The decisive test. Commit a
lockfile-only change, then run
`moon ci :build :test :lint --base HEAD~1 --include-relations` and read `.moon/cache/ciReport.json`
to distinguish **ran** from **replayed** — the form SMA-534's V2 used. Required: the three FFI
actions appear as *ran*; `paigasus-node-bindings-rs:build` / `paigasus-wasm-rs:build` appear as
replays. A bare `moon run paigasus-kernel-ts:build` satisfies nothing here — it proves the task
works, not that a lockfile change *schedules* it, and that proxy has never been established for a
`script:`-form task on a non-Rust project.

**V5 — prove the failure direction.** Make the FFI build genuinely fail on a lockfile-only change and
confirm `paigasus-kernel-ts:build` reds where it previously replayed a cached green. Preferred:
pin `wasm-bindgen` to a 0.2.z the proto-pinned wasm-pack 0.15.0 cannot process. If no such published
version exists, fall back to any workspace dependency pinned to a version that fails to build — the
two things that must be demonstrated are that the task **ran** (per `ciReport.json`) and that it
**failed**, not the specific error text.

Note the mtime hazard when reverting an experimental edit: restoring a file via `mv file.bak file`
rolls its mtime *backwards*, and cargo then reuses the artifact built from the temporary edit. Revert
with an editor write followed by `touch`, never a `.bak` move.

**V6 — price the real interleaving.** The Cost figures above are sequential and are a lower bound.
Measure a **synthetic lockfile-only commit** end to end (`moon ci … --base <that commit>
--include-relations`), not this PR's own CI run — this PR edits the two `moon.yml` files and
therefore schedules a different task set. Record wall time under real concurrency and split the
`rs/target` growth into dev/host vs release/wasm32. Also record the wasm-bindgen-cli fetch time on a
version-changing run.

**V7 — the full repo gate graph.** Per CLAUDE.md, run the whole `moon ci` gate list with `--base
origin/main --include-relations` before pushing, not just the per-project tasks. `repo:affected-smoke`
is the gate that carries Layers 1 and 2.

## Scope

1. `ts/packages/paigasus-kernel/moon.yml` — four inputs on `build` and on `test`.
2. `py/packages/paigasus-kernel/moon.yml` — four inputs on `test`.
3. `ci/affected-graph/run.sh` — the `lockfile->all-lint` expected set (+3 rows) and its comment.
4. `ci/affected-graph/cargo_moon_parity.py` — A5. This is more than one function, and SMA-534
   recorded the trap: `moon_projects()` must carry per-task `command`/`args`/`script` (a third
   parallel dict alongside `tasks`/`task_inputs`); `self_test()`'s clean fixture must gain those
   keys **or the negative control exits 1** and `repo:affected-smoke` is an immediate hard red for
   every contributor, since `moon.yml:125-128` runs `--negative-control` first; `main()`'s PASS
   string and the `--self-test` "all four assertions fire" line must change; and a fifth remediation
   block is needed in `main()`'s reporting loop.
5. `ci/affected-graph/README.md` — an A5 bullet beside A4's (`:55-59`), corrections to `:44` and
   `:85` (the case no longer schedules only lints), and the moon-upgrade re-grounding paragraph
   (`:90-95`) must record that A5 adds a **second** moon-version dependency: the per-task
   `command`/`args`/`script` shape, alongside `inputFiles`.
6. `CLAUDE.md:69-70` — note that the `lockfile->all-lint` expected set now also carries three FFI
   rows, so a contributor re-baselining it is not surprised. The existing claim (every new Rust crate
   changes the set) remains true.
7. This spec.

Out of scope, and deliberately so:

* **Committed-glue drift.** `paigasus_wasm*.{js,d.ts}` and `index.{js,d.ts}` are tracked; only
  `*.wasm`/`*.node` are gitignored. A wasm-bindgen bump regenerates that glue in CI, leaves it
  uncommitted, and merges green — no gate diffs those paths. This change catches the *loud* failure
  (the CLI cannot process the crate) but not the *quiet* one (the CLI emits different glue). A
  `git diff --exit-code` gate in the shape of `repo:parity-corpus-drift` (`moon.yml:152`) is the
  obvious fix, but it needs its own design: the glue must first be shown to be byte-identical across
  macOS and Linux, or the gate reds on every contributor's platform. **File as a follow-up.**
* **`repo:parity-corpus-drift`** runs `cargo run -p paigasus-kernel-parity` but keys on no lockfile —
  a cousin of this gap. A5's markers do not match it, and widening them to `cargo run`/`cargo tree`
  would demand `rust-toolchain.toml` on grep-only `repo:` gates that never invoke a compiler.
* **Caching wasm-pack's own cache dir** — see Cost; needs an SMA-520 key discriminator.
* **`prebuild.yml` triggers** stay as SMA-520 set them.
* **The duplicated `napi build` + `wasm-pack` work between `paigasus-kernel-ts:build` and `:test`**
  is pre-existing and load-bearing: SMA-427 gave them separate out-dirs to fix a CI race where both
  copied into the shared crate dir. Both tasks get the inputs — `:test`'s cached *vitest result* is
  as resolution-dependent as `:build`'s artifact, so exempting it would leave the same
  cache-correctness bug this spec exists to fix. Consolidating them would roughly halve the wasm
  portion of the delta and is worth its own issue.

## Open question, inherited

SMA-534 recorded no policy for a Dependabot Cargo PR that reds on a latent finding unrelated to the
bump. This change adds a second, noisier trigger class (napi / wasm-pack / maturin failures). The
answer is the same and is still unwritten; this spec does not invent one.

## Rollback

Revert the two `moon.yml` input edits, the `run.sh` expected set, A5 and the three
documentation touch-ups. Nothing depends on them beyond those files; there is no data migration and
no published artifact. The pre-change state is the SMA-534 state, whose residual risk is documented.
