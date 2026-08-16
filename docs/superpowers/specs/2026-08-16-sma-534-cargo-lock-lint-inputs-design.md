# SMA-534 — Workspace-level `lint` inputs for the Rust graph

**Status:** approved
**Linear:** [SMA-534](https://linear.app/smaschek/issue/SMA-534)
**Related:** SMA-526 (Rust `lint` propagation — the in-tree half of this shape), SMA-524
(Cargo↔Moon parity gate), SMA-409/429 (affected-graph guard), SMA-520 (Actions spend),
SMA-537 (`build.rs` / `rustfmt.toml` inputs — deliberately *not* this issue)

## Problem

`.moon/tasks/rust.yml` declares `lint`'s inputs as `@group(sources)` (`src/**/*`),
`@group(tests)` and the **project-local** `Cargo.toml`. None of the workspace-level files that
determine what clippy actually sees is an input to any crate task.

Measured on moon 2.3.2, `moon query tasks --affected --downstream deep`:

```
rs/Cargo.lock       -> repo:deny  repo:nats-permissions  repo:publish-metadata  repo:wasm-getrandom-free
rs/Cargo.toml       -> repo:affected-smoke  repo:deny  repo:machete  repo:publish-metadata  repo:wasm-getrandom-free
rs/rust-toolchain.toml -> repo:publish-metadata
```

Not one crate task in any of the three sets — no `build`, no `test`, no `lint`, not even for a
crate that directly consumes the changed dependency.

SMA-526 closed the *in-tree upstream → consumer* clippy hole and deliberately left this one open.
This is strictly wider: there, a consumer went unlinted; here even the directly-using crate does.

### Three distinct silent-green cases, not one

1. **`rs/Cargo.lock`** — the resolved dependency versions. A bump that deprecates an API is the
   single most common way `-D warnings` starts firing. Dependabot Cargo PRs are exactly this
   shape, so they merge green and red `main` later.
2. **`rs/Cargo.toml`** — not only `[workspace.dependencies]` but `[workspace.lints.rust]` and
   `[workspace.lints.clippy]` (lines 216/219). **Editing the clippy lint posture itself currently
   lints nothing.**
3. **`rs/rust-toolchain.toml`** — pins `channel = "1.95.0"`, i.e. *which* clippy-driver runs and
   therefore which lints exist at all. A 1.95 → 1.96 bump is the most reliable way to introduce
   new lints across all 13 crates, and it currently schedules exactly `repo:publish-metadata`.

## Decision

Add all three files to `lint`'s inputs in `.moon/tasks/rust.yml` — the inherited task file, so a
new crate has no per-crate declaration to forget (the SMA-526 placement argument applies
unchanged). `build`, `build-release`, `test` and `fmt` are untouched.

```yaml
  lint:
    command: 'cargo clippy --all-targets -- -D warnings'
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

plus the four unchanged `repo:*` tasks. No `build` and no `test` appear — the leading `/` makes
these *workspace-relative* inputs on a task whose project does not own the file.

### Why `lint` alone is sufficient, not merely cheap

`cargo clippy --all-targets` is a superset of `cargo check --all-targets`: it type-checks the lib,
the bins **and** the test targets. So a dependency bump that breaks *compilation* — including
compilation of test code — is caught by `lint` on its own. What `lint` does not buy is *runtime*
verification, and buying that means scheduling `paigasus-iam:test` on every Dependabot PR, whose
container suites are Docker-gated and flaky under parallel load. `build` adds almost nothing on
top: everything it compiles, `lint --all-targets` already compiles; it proves only codegen and
linking.

### Why the project graph is not involved

`rs/` has no Moon project, so `rs/Cargo.lock` is owned by `repo` (source `.`). Measured: after the
change `moon query projects --affected --downstream deep` on that file still returns `repo` alone,
while `moon query tasks --affected` returns the thirteen lints. Task affectedness flows through
task inputs independently of project membership.

This has a concrete consequence for the guard: **no existing `assert_case` in
`ci/affected-graph/run.sh` needs re-baselining.** Every existing case touches a `.rs` source file,
which does not match the new inputs, and the project-level sets are unchanged by construction. The
issue's own note that re-baselining would be needed is superseded by this measurement.

## Cost

Full-workspace `cargo clippy --all-targets -- -D warnings` after `cargo clean` (so the 1259-crate
dependency tree is rebuilt from scratch): **31s wall / 2m46s CPU** on the development machine. A
second, no-op run is 0.3s.

That is the worst case. On CI the `rs/target` cache key is

```
rust-${{ runner.os }}-${{ hashFiles('rs/rust-toolchain.toml') }}-line-tables-only-lint-deps-${{ hashFiles('rs/Cargo.lock') }}
```

with a prefix `restore-keys` fallback, so a Dependabot PR misses the exact key but restores the most
recent `main` entry: only the bumped dependency and its dependents recompile, not the tree. The
`-lint-deps-` segment already exists because SMA-526 widened what is built, so clippy artifacts are
in the cache today.

### No cache-key discriminator is added

The SMA-520 lesson is that widening what a cached job builds *without* changing the key means the
new output is never saved (`actions/cache` skips its save on an exact primary-key hit). That
failure mode does not apply here:

* `rs/Cargo.lock` and `rs/rust-toolchain.toml` are **both already in the key**, so any change to
  them misses the key and the enlarged `rs/target` is saved.
* A `rs/Cargo.toml`-only edit (e.g. a `[workspace.lints]` change with no resolution change) does
  hit the key exactly while re-running clippy on the workspace members. Those are member-only
  fingerprints — seconds of work — and the alternative is churning the whole 1.5 GB cache once to
  save them. Deliberately rejected as not worth it.

## Guard

A fix without a guard reopens; that is the SMA-409/429/524/526 pattern. Two layers, matching the
existing split — `run.sh` holds hand-written *behavioral* cases, `cargo_moon_parity.py` holds
*generic* assertions.

### Layer 1 — behavioral, in `ci/affected-graph/run.sh`

One new strict-equality, default-deny task case:

```
run_task_case "lockfile->all-lint" "rs/Cargo.lock" \
  "paigasus-gateway-rs:lint,paigasus-iam-core-rs:lint,paigasus-iam-rs:lint,\
paigasus-kernel-parity-rs:lint,paigasus-kernel-rs:lint,paigasus-logging-rs:lint,\
paigasus-node-bindings-rs:lint,paigasus-observability-rs:lint,paigasus-proto-derive-rs:lint,\
paigasus-proto-rs:lint,paigasus-py-bindings-rs:lint,paigasus-service-info-rs:lint,\
paigasus-wasm-rs:lint"
```

This proves the inputs take **effect**, not merely that they are declared. `assert_task_case`
filters observed tasks to the names `build`/`test`/`lint` across all projects; that is safe for
this case because `repo` declares no task with any of those names (verified via `moon query
tasks`), and no py/ts task is scheduled by a `rs/` file.

The case is deliberately *not* triplicated for `rs/Cargo.toml` and `rs/rust-toolchain.toml`: three
hand-maintained thirteen-entry CSVs would have to be updated in lockstep every time a crate is
added. Layer 2 covers those two files generically instead.

### Layer 2 — generic assertion **A4**, in `ci/affected-graph/cargo_moon_parity.py`

For every Rust crate that maps to a Moon project, the crate's **resolved** `lint` input files must
contain all three workspace files:

```
rs/Cargo.lock   rs/Cargo.toml   rs/rust-toolchain.toml
```

`moon query projects` already emits per-task `inputFiles` as resolved, workspace-relative paths
(`{".moon/tasks/rust.yml": {}, "rs/crates/libs/paigasus-kernel/Cargo.toml": {}}`), so A4 reads
Moon's own resolution and the gate keeps its standing rule of never parsing YAML and never shelling
out to cargo.

A4 is self-maintaining: a newly added crate inherits the inputs and is asserted with no CSV to
update, and a crate that overrides `lint`'s inputs with `merge: replace` is caught. Two violation
shapes, reported distinctly, mirroring how A3 separates an absent task from a missing dep:

* the crate has no `lint` task at all;
* `lint` exists but its resolved inputs omit one or more of the three files (name them).

Per SMA-524 D6, A4 gets its own `self_test()` cases so it cannot pass vacuously: one asserting it
fires when a file is dropped from a fixture's inputs, one asserting it fires when the `lint` task
is absent, and one asserting the clean fixture stays green.

## Verification

1. **The gates themselves.** `moon ci … :affected-smoke … --base origin/main --include-relations`
   green, and the full repo-gate list from CLAUDE.md green.
2. **`moon ci`, not just `moon query`.** The guard uses `moon query tasks --affected` as a proxy
   for what `moon ci` schedules. Prove the proxy holds for this new input class: make a throwaway
   commit touching only `rs/Cargo.lock`, run `moon ci :lint --base HEAD~1 --include-relations`, and
   read `.moon/cache/ciReport.json` for thirteen `*-rs:lint` actions.
3. **A4 is non-vacuous against the real tree.** Revert the three input lines in
   `.moon/tasks/rust.yml`, run the parity gate, and confirm thirteen named violations; restore and
   confirm zero. (The `--self-test` fixtures prove the assertion fires on synthetic input; this
   proves it fires on the actual repository.)
4. **`run.sh --negative-control`** still passes, including the new A4 self-test rows. CI runs
   `run.sh` *without* `--negative-control`, so a rotted self-test would ship green — SMA-526 hit
   exactly this.
5. **No existing case regressed.** `run.sh` green with every pre-existing `assert_case` and
   `assert_task_case` expectation unmodified.

## Out of scope

* **`build.rs` and `rs/rustfmt.toml` inputs.** Owned by SMA-537. `fmt` is untouched here.
* **`build`/`test` inputs.** Deliberately excluded; see the cost argument above.
* **`.moon/tasks/rust.yml` as an input.** Already handled by Moon, which injects
  `/.moon/tasks/rust.yml` and a `.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}` glob into every task's
  inputs automatically — visible in the resolved `inputFiles`. Nothing to add.
* **py/ts equivalents.** SMA-535 and SMA-536.

## Documentation

* `ci/affected-graph/README.md` — the new `lockfile->all-lint` case and A4.
* `CLAUDE.md` — the gotcha stating that a new crate depending on `paigasus-kernel-rs` must be added
  to the `kernel->bindings` expected set now also has to name the `lockfile->all-lint` set, since a
  new crate changes that thirteen-entry expectation too.
