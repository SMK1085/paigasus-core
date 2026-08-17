# SMA-534 — Workspace-level `lint` inputs for the Rust graph

**Status:** approved
**Linear:** [SMA-534](https://linear.app/smaschek/issue/SMA-534)
**Related:** SMA-526 (Rust `lint` propagation — the in-tree half of this shape), SMA-524
(Cargo↔Moon parity gate), SMA-409/429 (affected-graph guard), SMA-520 (Actions spend),
SMA-444 (runner disk exhaustion), SMA-537 (`build.rs` / `rustfmt.toml` inputs — deliberately
*not* this issue)

## Problem

`.moon/tasks/rust.yml` declares `lint`'s inputs as `@group(sources)` (`src/**/*`),
`@group(tests)` and the **project-local** `Cargo.toml`. None of the workspace-level files that
determine what clippy actually sees is an input to any crate task.

Measured on moon 2.3.2, `moon query tasks --affected --downstream deep`:

```
rs/Cargo.lock          -> repo:deny  repo:nats-permissions  repo:publish-metadata  repo:wasm-getrandom-free
rs/Cargo.toml          -> repo:affected-smoke  repo:deny  repo:machete  repo:publish-metadata  repo:wasm-getrandom-free
rs/rust-toolchain.toml -> repo:publish-metadata
```

Not one crate task in any of the three sets — no `build`, no `test`, no `lint`, not even for a
crate that directly consumes the changed dependency.

SMA-526 closed the *in-tree upstream → consumer* clippy hole and deliberately left this one open.
This is strictly wider: there, a consumer went unlinted; here even the directly-using crate does.

### The three cases, ranked by what they actually buy

1. **`rs/Cargo.lock`** — the resolved dependency versions, and the whole point of the issue. A bump
   that deprecates an API is the most common way `-D warnings` starts firing. Dependabot Cargo PRs
   are exactly this shape, so they merge green and red `main` later.
2. **`rs/Cargo.toml`** — not only `[workspace.dependencies]` but `[workspace.lints.rust]` and
   `[workspace.lints.clippy]` (lines 216/219). **Editing the clippy lint posture itself currently
   lints nothing.** It also covers *feature flips*, which never appear in `Cargo.lock` at all.
3. **`rs/rust-toolchain.toml`** — pins `channel = "1.95.0"`, i.e. which clippy-driver runs and
   therefore which lints exist. This one is **defence-in-depth, not the fix for a live hole**:
   `rust-toolchain.toml:8` requires `channel` to be kept in lockstep with `.moon/toolchains.yml`
   `rust.version`, and `.moon/toolchains.yml` is in `implicitInputs` (`.moon/tasks.yml:17`).
   Measured: a `.moon/toolchains.yml` touch already schedules 115 tasks including all thirteen
   crate lints. So a *correct* toolchain bump is already covered; this input catches the bump that
   drifts the two files apart, which is precisely the failure the lockstep comment warns about.

## Decision

Add all three files to `lint`'s inputs in `.moon/tasks/rust.yml` — the inherited task file, so a
new crate has no per-crate declaration to forget (SMA-526's placement argument applies unchanged).
`build`, `build-release`, `test` and `fmt` are untouched. The command also gains `--locked`.

```yaml
  # illustrative — SMA-526's seven-line rationale comment above `deps` is retained verbatim
  lint:
    command: 'cargo clippy --locked --all-targets -- -D warnings'
    deps: ['^:build']
    inputs:
      - '@group(sources)'
      - '@group(tests)'
      - 'Cargo.toml'
      - '/rs/Cargo.toml'
      - '/rs/Cargo.lock'
      - '/rs/rust-toolchain.toml'
```

Measured after the change — a `rs/Cargo.lock` touch schedules exactly the thirteen crate lints:

```
paigasus-gateway-rs:lint         paigasus-iam-core-rs:lint      paigasus-iam-rs:lint
paigasus-kernel-parity-rs:lint   paigasus-kernel-rs:lint        paigasus-logging-rs:lint
paigasus-node-bindings-rs:lint   paigasus-observability-rs:lint paigasus-proto-derive-rs:lint
paigasus-proto-rs:lint           paigasus-py-bindings-rs:lint   paigasus-service-info-rs:lint
paigasus-wasm-rs:lint
```

plus the four unchanged `repo:*` tasks. The leading `/` makes these *workspace-relative* inputs on
a task whose project does not own the file.

### `--locked` is part of the fix, not a drive-by

Without it the guarantee the whole issue rests on — *"lint what the lockfile resolves"* — is
unproven. If `rs/Cargo.lock` is inconsistent with the manifests, cargo silently re-resolves and
rewrites it, so the thirteen lints would compile against newest-compatible versions rather than the
ones the PR ships. This repo has a concrete incident of a Dependabot Cargo PR shipping a lockfile
resolved from only 3 of 11 workspace members (530 → 158 packages, ~370 crates silently unpinned),
and **no CI gate anywhere runs `--locked`**. Verified adoptable today: `cargo metadata --locked`
succeeds on the current tree.

### Why `lint` and not `build`/`test`

`cargo clippy --all-targets` is a superset of `cargo check --all-targets` — it type-checks the lib,
the bins **and** the test targets — so a dependency bump that breaks *compilation of Rust source*
is caught. What it does not buy is runtime verification, and buying that means scheduling
`paigasus-iam:test` on every Dependabot PR, whose container suites are Docker-gated and flaky under
parallel load.

This is a cost trade, **not** a claim of sufficiency. See the residual risk below.

### Accepted residual risk: clippy neither links nor cross-compiles

`cargo clippy` emits metadata; it never links. `paigasus-py-bindings`, `paigasus-node-bindings` and
`paigasus-wasm` are all `crate-type = ["cdylib"]`, and for them **linking is the failure mode**.
Clippy also runs on the host target only, so `wasm32-unknown-unknown` is never compiled.

Concrete scenario this change does **not** close: Dependabot bumps `wasm-bindgen` 0.2.z.
`rs/Cargo.toml:90-96` records an INVARIANT that the proto-pinned wasm-pack must support that exact
0.2.z — "bump the two together, or this re-introduces the schema mismatch". After this change all
thirteen lints go green. `paigasus-kernel-ts:{build,test}` and `paigasus-kernel-py:test` are the
only tasks that run `wasm-pack`/`napi build`, and they list `/rs/crates/**` inputs but **not** the
lockfile (`ts/packages/paigasus-kernel/moon.yml:55-125`, `py/packages/paigasus-kernel/moon.yml:46-55`),
so they replay a cached green. `prebuild.yml:25-39` deliberately excludes `rs/**` from its
pull-request trigger, running the cross-build matrix on push-to-`main` only.

That is the same merges-green/reds-`main` shape this issue exists to close, surviving for the FFI
third of the workspace. It is **pre-existing and not worsened** by this change — today those crates
get nothing at all — but the honest scope of the fix is "Rust source compiles and lints", not "a
dependency bump is safe". Closing it means adding `/rs/Cargo.lock` and `/rs/Cargo.toml` to the ts/py
kernel task inputs (which are cache-input-incomplete today regardless) and paying a `wasm-pack`
release build plus a `napi build` on every Dependabot Cargo PR. **Out of scope here; file as a
follow-up.** Note it would also re-baseline the new `lockfile->all-lint` expected set, since those
tasks are named `build` and `test`.

### Why the project graph is not involved

`rs/` has no Moon project, so `rs/Cargo.lock` is owned by `repo` (source `.`). Measured directly on
that file: after the change `moon query projects --affected --downstream deep` still returns `repo`
alone, while `moon query tasks --affected` returns the thirteen lints. Task affectedness flows
through task inputs independently of project membership.

Consequence: **no existing `assert_case` needs re-baselining.** Every existing case touches a `.rs`
source file, which matches none of the new inputs. The issue's own note that re-baselining would be
needed is superseded by this measurement.

## Rejected alternatives

**A single repo-scoped `cargo clippy --workspace` gate.** One cargo invocation instead of thirteen,
no thirteen-entry CSV, no A4 — and it would follow the pattern `moon.yml` already uses for eight
`rs/`-scoped gates. Rejected on SMA-526's measured evidence: feature unification differs between
`--workspace` and per-crate resolution, so the two evict each other from the shared `rs/target`
(cold `--workspace` 36s → then 7× per-crate **69s, a full rebuild**). All thirteen `build`/`test`
tasks are per-crate, so a `--workspace` clippy would thrash the target dir against them on every
other CI run.

**Adding the files to `build`/`test` as well.** Everything `build` compiles, `lint --all-targets`
already compiles; `build` adds only codegen and linking. `test` adds real runtime coverage but
costs the Docker-gated IAM suites on every Dependabot PR. See the residual-risk section for the
narrow slice of `build` that would actually pay for itself.

**Triplicating the behavioural case** for `rs/Cargo.toml` and `rs/rust-toolchain.toml`: three
hand-maintained thirteen-entry CSVs updated in lockstep on every new crate. A4 covers those two
files generically instead.

## Cost

Measured on the development machine (Apple silicon, warm `~/.cargo/registry`, `rs/target` emptied
with `cargo clean` run from `rs/`, moon state cache cleared) — the **actual** scheduled workload,
`moon run :lint`, which is thirteen separate per-crate `cargo clippy` invocations sharing
`rs/target`, not one `--workspace` pass:

| metric | value |
|---|---|
| wall | **1m 47s** |
| CPU | **5m 04s** user + 46s sys |
| moon actions | **24 completed** — 13 × `lint` + 11 upstream `build` pulled in by `^:build` |
| `rs/target` after | **3.1 GB** |

For contrast, a single `cargo clippy --workspace --all-targets` on the same cold tree is 31s wall /
2m46s CPU / 1.5 GB — i.e. **the per-crate workload costs ~3.5× the wall time and ~2× the disk**, the
same divergence SMA-526's eviction table records. Quoting the `--workspace` figure would have
understated this by a factor of three.

**Headroom.** `ci.yml:21-22` sets `timeout-minutes: 30`. `rs/Cargo.lock` is in the `rs/target` cache
key, so a Dependabot PR misses the exact key and restores the most recent `main` entry via
`restore-keys`: only the bumped dependency and its dependents recompile. The figures above are the
no-restore worst case. CI also sets `CARGO_PROFILE_{DEV,TEST}_DEBUG: line-tables-only` (SMA-444), so
its `rs/target` is materially smaller than 3.1 GB. Both numbers must be re-confirmed on CI during
verification rather than extrapolated from a dev machine.

### The cache key gains `rs/Cargo.toml`

`ci.yml:96` currently keys on `hashFiles('rs/rust-toolchain.toml')` and `hashFiles('rs/Cargo.lock')`.
Those two files therefore always miss the key when they change, and `actions/cache` saves the
enlarged `rs/target`. `rs/Cargo.toml` does **not** have that property: enabling a feature on an
existing workspace dependency changes no resolution and so leaves `Cargo.lock` byte-identical — the
primary key **hits exactly**, `actions/cache` skips its save, and cargo recompiles that dependency
and everything above it on every subsequent run, permanently. That is the SMA-520 failure mode, and
per SMA-520 a verification run cannot reveal it.

Fix: extend the primary key to `hashFiles('rs/Cargo.lock', 'rs/Cargo.toml')`. Cheaper and more
precise than a literal discriminator segment — the existing `restore-keys` prefixes (`ci.yml:105-107`)
are unchanged, so no one-time 1.5 GB cold churn.

## Guard

A fix without a guard reopens; that is the SMA-409/429/524/526 pattern. Two layers, matching the
existing split — `run.sh` holds hand-written *behavioural* cases, `cargo_moon_parity.py` holds
*generic* assertions.

### Layer 1 — behavioural, in `ci/affected-graph/run.sh`

One new strict-equality, default-deny task case:

```
run_task_case "lockfile->all-lint" "rs/Cargo.lock" \
  "paigasus-gateway-rs:lint,paigasus-iam-core-rs:lint,paigasus-iam-rs:lint,\
paigasus-kernel-parity-rs:lint,paigasus-kernel-rs:lint,paigasus-logging-rs:lint,\
paigasus-node-bindings-rs:lint,paigasus-observability-rs:lint,paigasus-proto-derive-rs:lint,\
paigasus-proto-rs:lint,paigasus-py-bindings-rs:lint,paigasus-service-info-rs:lint,\
paigasus-wasm-rs:lint"
```

This proves the inputs take **effect**, not merely that they are declared.

`assert_task_case` filters observed tasks to the names `build`/`test`/`lint` across **all**
projects. The durable premise is narrow and must be stated as such in the case's comment: `repo`
declares no task named `build`/`test`/`lint` (verified via `moon query tasks`), and **no py/ts task
lists `rs/Cargo.lock` today**. It is *not* true that no py/ts task is reachable from an `rs/` path —
`ts/packages/paigasus-kernel:{build,test}` and `py/packages/paigasus-kernel:test` all declare
`/rs/crates/**` inputs and are one input line away from entering this case's observed set. The
comment names those three tasks so the next person to touch them sees the coupling.

### Layer 2 — generic assertion **A4**, in `ci/affected-graph/cargo_moon_parity.py`

For every Rust crate that maps to a Moon project, the crate's **resolved** `lint` input files must
contain all three workspace files (`rs/Cargo.lock`, `rs/Cargo.toml`, `rs/rust-toolchain.toml`).

Verified on moon 2.3.2: `moon query projects` emits per-task `inputFiles` as a **path-keyed object**
of resolved, workspace-relative paths — for `paigasus-kernel-rs:lint` today,
`{".moon/tasks/rust.yml": {}, "rs/crates/libs/paigasus-kernel/Cargo.toml": {}}`. So A4 reads Moon's
own resolution and the gate keeps its standing rule of never parsing YAML and never shelling out to
cargo. (Note the three new entries are plain file paths and land in `inputFiles`; workspace-level
*globs* such as Moon's automatic `.moon/*.{yml,…}` land in `inputGlobs` instead, which is why
`.moon/toolchains.yml` is absent from the object above despite being an implicit input.)

**A4 iterates every crate unconditionally.** It must *not* reuse A3's `if want:` guard: A3 only
reaches its assertions for crates that have in-tree dependencies, so `paigasus-kernel`,
`paigasus-logging`, `paigasus-observability` and `paigasus-proto-derive` — four of thirteen — are
outside it. Copying that shape would leave them unguarded while the negative control stayed green.

Three violation shapes, reported distinctly (mirroring how A3 separates an absent task from a
missing dep):

* the crate has no `lint` task at all;
* `lint` exists but the `inputFiles` key is absent from moon's output — **fire loudly**, never skip,
  since a silent skip would turn a moon-version change into a vacuous pass;
* `lint` exists and its resolved inputs omit one or more of the three files (name which).

A4 gets its **own** function and violation list rather than being bolted into `check()`'s
Cargo-versus-Moon loop: the module contract in `cargo_moon_parity.py:1-14` is dependency-graph
parity, and A4 uses none of the `crates` dependency data. The module docstring and
`ci/affected-graph/README.md` are updated to match.

Per SMA-524 D6, A4 gets `self_test()` rows so it cannot pass vacuously: it fires when a file is
dropped from a fixture's inputs; it fires for a **dep-free** fixture crate (pinning the divergence
from A3 above); it fires when the `lint` task is absent; it fires when `inputFiles` is absent; and
the clean fixture stays green.

### Layer 3 — make the negative control actually run in CI

`moon.yml:120` runs `ci/affected-graph/run.sh` bare, so the self-test rows above — the only proof
A4 can bite — are never executed by CI. SMA-526 hit exactly this and the README still lists
`--negative-control` as a manual step. There is in-repo precedent for the fix in the same file:
`repo:publish-metadata` (`moon.yml:336-339`) runs its `--negative-control` pass first, commented
"a gate that cannot report red is worse than no gate, and it is sub-second, so paying for it before
the real checks is free".

Change `repo:affected-smoke`'s script to run `--negative-control` first, then the real suite. Cost:
a few extra `moon query` invocations plus a sub-second Python self-test.

## Scope

1. **`.moon/tasks/rust.yml`** — three inputs on `lint`, plus `--locked` on the command. SMA-526's
   rationale comment is retained.
2. **`ci/affected-graph/cargo_moon_parity.py`** — four changes, required together:
   - `moon_projects()` must carry per-task resolved `inputFiles`, not only dep targets.
   - A new A4 function and violation list; `main()` reports it with its own remediation text.
   - **The `self_test()` clean fixture must gain the new data**, or the clean case reports a
     violation and the negative control exits 1. SMA-526 recorded this exact trap as load-bearing.
   - New self-test rows per the list above.
3. **`ci/affected-graph/run.sh`** — the `lockfile->all-lint` case, with the comment naming the three
   py/ts tasks that are one input line from entering its observed set.
4. **`moon.yml`** — `repo:affected-smoke` runs `--negative-control` first.
5. **`.github/workflows/ci.yml`** — `rs/Cargo.toml` added to the `rs/target` primary cache key.
6. **Docs** — `ci/affected-graph/README.md` (the new case, A4, the negative-control change);
   `CLAUDE.md` (a new crate now changes the `lockfile->all-lint` thirteen-entry set as well as the
   `kernel->bindings` set).

## Verification

1. **The gates.** The full repo-gate `moon ci … --base origin/main --include-relations` list from
   CLAUDE.md, green.
2. **`moon ci`, not just `moon query`.** The guard uses `moon query tasks --affected` as a proxy for
   what `moon ci` schedules. Prove the proxy holds for this new input class: commit a throwaway
   `rs/Cargo.lock` edit, run `moon ci :lint --base HEAD~1 --include-relations`, and read
   `.moon/cache/ciReport.json`. Expect **thirteen `*-rs:lint` actions that run**, plus upstream
   `build` actions pulled in by `^:build` — those must appear as cache **replays**, since their
   inputs exclude the lockfile. A build that *runs* means an input is wider than intended.
3. **A4 is non-vacuous against the real tree.** Revert the three input lines, run the parity gate,
   confirm thirteen named violations; restore, confirm zero. (`--self-test` proves it fires on
   synthetic fixtures; this proves it fires on the actual repository.)
4. **`run.sh --negative-control`** passes, including the new A4 rows — and is now reached by
   `repo:affected-smoke` itself.
5. **No existing case regressed.** Every pre-existing `assert_case` and `assert_task_case`
   expectation unmodified and green.
6. **CI-side cost is confirmed, not extrapolated.** On the PR run, record the `moon ci` wall time
   and the post-run `rs/target` size, and state the headroom against `timeout-minutes: 30` and the
   ~14 GB runner disk. The dev-machine figures above do not settle this.

## Rollback

`git revert` of the implementation commit is complete and safe: the inputs, the guard case, A4 and
the cache-key change are independent of any runtime code path.

If only the *expected set* is wrong — a crate added concurrently, or a moon behaviour difference on
the runner — correcting the CSV in `run.sh` alone is sufficient and does not require reverting the
fix. This matters because `repo:affected-smoke` is part of the required `moon ci` check, so a wrong
thirteen-entry set blocks every contributor's merge until corrected.

## Out of scope

* **`build.rs` and `rs/rustfmt.toml` inputs** — SMA-537. `fmt` is untouched here. Note
  `rs/crates/bindings/paigasus-node-bindings/build.rs` is in no task's inputs, so editing it alone
  still schedules nothing; that is SMA-537's to close, and it is orthogonal to the workspace-level
  files this issue covers.
* **`rs/.cargo/config.toml`** — sets `rustflags` for the two `*-apple-darwin` targets and is in no
  crate task's inputs. Excluded deliberately: it is dev-machine-only today (CI is Linux), and
  `prebuild.yml:37` already lists it as a pull-request trigger path for the darwin matrix. Named
  here so the enumeration above is not mistaken for exhaustive.
* **Closing the cdylib/wasm32 link hole** — see the residual-risk section; needs its own issue and
  its own cost decision.
* **py/ts equivalents of the propagation defect** — SMA-535, SMA-536.

## Open question for implementation

When a Dependabot or toolchain-bump PR now reds on a clippy finding *unrelated* to the bump — a
lint that has been latent since the last time that crate was linted — the bump PR would otherwise
have to grow source changes across up to thirteen crates. The intended policy is: fix it in a
separate PR and rebase the bump onto it, rather than widening the bump. Recorded here so the first
occurrence is not resolved ad hoc.
