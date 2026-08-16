# SMA-526 — Rust `lint` propagation across Moon edges

**Status:** approved
**Linear:** [SMA-526](https://linear.app/smaschek/issue/SMA-526)
**Related:** SMA-524 (Cargo↔Moon parity gate), SMA-409/429 (affected-graph guard), SMA-401 (py/ts whole-tree lint), SMA-520 (Actions spend)

## Problem

`.moon/tasks/rust.yml` gives `lint` (`cargo clippy --all-targets -- -D warnings`) and `fmt` no
`deps`, and no project overrides them. Only `build` and `test` carry a task-level `^:build`.

`^:build` is what makes a task *affected* when an upstream changes. Without it, `lint` propagates
across **no** edge in the 13-crate workspace. Measured on moon 2.3.2, a `paigasus-proto` edit
schedules:

```
paigasus-proto-rs:build  paigasus-proto-rs:build-release  paigasus-proto-rs:fmt
paigasus-proto-rs:lint   paigasus-proto-rs:test
paigasus-gateway-rs:build       paigasus-gateway-rs:test
paigasus-iam-rs:build           paigasus-iam-rs:test
paigasus-service-info-rs:build  paigasus-service-info-rs:test
repo:machete
```

The upstream gets its own `lint`. Every downstream consumer gets **only** `build` and `test`.

**Consequence.** An in-tree upstream change that trips `-D warnings` in a *consumer* — an added
`#[deprecated]`, a signature change that makes a borrow redundant, a newly-unused import — is not
linted on the PR. It passes CI, merges, and reds `main` on the next run that happens to schedule
that crate's `lint`. Same silent-hole shape as SMA-524, one task over: nothing goes red when the
defect is introduced.

## Decision

Add `deps: ['^:build']` to `lint` in `.moon/tasks/rust.yml` — the **inherited** task file, not each
crate's `moon.yml`.

This placement is the substance of the decision, not an implementation detail. `build` and `test`
declare their `^:build` per-project, and that is precisely how SMA-505 shipped a crate with three
missing edges: a new crate must remember to write them. Declaring lint's dep once in the inherited
file means there is no per-crate declaration for a future crate to forget.

### Rejected alternatives

**`deps: ['^:lint']` instead of `^:build`.** Propagates affectedness identically — a task is
affected when a dep task is affected — and would not chain lint behind builds. Rejected for three
reasons: (a) it breaks A3's uniform shape, which asserts *"every task schedules `{upstream}:build`"*,
forcing a special-cased assertion for the lint row; (b) a failing upstream clippy would block
downstream lint from running at all, so a full-graph run surfaces one crate's findings per CI round
instead of all of them at once; (c) `^:build` guarantees the upstream is already compiled when
clippy starts, which *reduces* `rs/target` lock contention rather than adding it. The ordering
`^:build` imposes costs approximately nothing in CI because those upstream builds are scheduled
anyway.

**Propagate `fmt` too.** Rejected. `cargo fmt --check` reads only the crate's own source files, so
an edit to crate Y cannot change crate X's formatting. Propagation would add CI work with zero
possible signal. (The one case where an out-of-crate action rewrites a crate's sources —
`contracts:generate` writing `paigasus-proto/src/generated/**` — is a *scheduling* concern handled
below, not a propagation one.)

**Whole-workspace clippy** (`cargo clippy --workspace` on a new `rs` root project, mirroring how
py/ts do lint under SMA-401). Rejected on measured evidence. Feature unification differs between
`--workspace` and per-crate resolution, so the two evict each other from the shared `rs/target`:

| step | elapsed |
|---|---|
| cold `cargo clippy --workspace --all-targets` | 36s |
| then 7× per-crate clippy | **69s** (full rebuild) |
| then `--workspace` again | 1s |
| then 7× per-crate again | 1s |

All 13 `build`/`test` tasks are per-crate, so introducing a `--workspace` clippy would thrash the
target dir against them on every CI run. The SMA-401 rationale does not transfer either: ruff and
eslint read a central config and are cheap whole-tree, whereas clippy is a compiler and per-crate
invocations already share `rs/target`.

## Scope

### In scope

1. **`.moon/tasks/rust.yml`** — `deps: ['^:build']` on `lint`. The fix.

2. **`ci/affected-graph/cargo_moon_parity.py`** — three changes, all required together:
   - Extend the A3 assertion loop from `("build", "test")` to `("build", "test", "lint")`.
   - **Update the `self_test()` fixtures.** The clean fixture at `self_test()` declares
     `tasks: {"build": …, "test": …}` with no `lint` key, and A3 reads `tasks.get(task, [])`, so
     widening the loop makes the *clean* fixture report a violation and the negative control exit 1.
     Add `"lint"` to both fixture projects and to the A3 broken fixture. This is load-bearing:
     `moon.yml:119` runs `run.sh` with **no** `--negative-control`, so CI would never catch the
     rotted self-test — it would ship green with the repo's only proof-that-the-gate-bites dead.
   - Emit a distinct violation message when the task key is *absent* rather than present-but-missing
     the dep, so the first crate to drop or rename `lint` gets an accurate diagnosis.
   - Update the remediation text (`"Fix: add '^:build' to the task's deps in the consumer's
     moon.yml"`), which misdirects for `lint` now that its dep lives in the inherited file.

3. **`ci/affected-graph/run.sh`** — the `proto->service-info-tasks` task case filters the observed
   task set to `build`/`test` and must admit `lint`. Its comment justifies the filter as *"those
   are the two tasks that carry `^:build`"*; that rationale expires here, so the comment is updated
   alongside the code. The guard is strict-equality, so leaving it alone reds CI. Expected set
   becomes these twelve targets (`build-release` and `fmt`, still excluded by the filter, are
   deliberately absent):

   ```
   paigasus-proto-rs:build         paigasus-proto-rs:test         paigasus-proto-rs:lint
   paigasus-service-info-rs:build  paigasus-service-info-rs:test  paigasus-service-info-rs:lint
   paigasus-iam-rs:build           paigasus-iam-rs:test           paigasus-iam-rs:lint
   paigasus-gateway-rs:build       paigasus-gateway-rs:test       paigasus-gateway-rs:lint
   ```

   The filter matches task *names* across all projects, and `contracts:lint` exists. It does not
   appear here — `contracts` is upstream of `paigasus-proto-rs` and `--downstream deep` traverses
   dependents — but adding `lint` is the first time the filter shares a name with a non-Rust
   project, so the comment must record that coupling.

4. **`rs/crates/libs/paigasus-proto/moon.yml`** and **`…/paigasus-service-info/moon.yml`** — add
   `contracts:generate` to `lint`'s deps, mirroring their own `build`/`test` and the ts sibling
   (`ts/packages/paigasus-proto/moon.yml` wires it into `build`, `typecheck` *and* `test`).
   `contracts:generate` declares no `outputs:` yet writes into `paigasus-proto/src/generated/**` —
   the files clippy compiles — so today `lint` can run concurrently with the generator. The race is
   pre-existing, but this change raises the number of concurrent Rust tasks, and lint's graph
   should not diverge from build's in the very crate used as the worked example.

5. **`.github/workflows/ci.yml`** — add a literal discriminator to the `rs/target` cache `key` **and**
   `restore-keys`. The key hashes only `rs/rust-toolchain.toml` and `rs/Cargo.lock`, neither of
   which this PR touches, so the primary key still matches an entry written before the change.
   `actions/cache` skips its post-job save on a primary-key hit, so the enlarged `rs/target` — the
   added clippy-driver artifacts for the downstream crates' dependency trees, which `cargo build`
   does not produce — would never be saved: a cold rebuild on every run, indefinitely. This is the
   exact failure mode SMA-520 documented, including that a verification dispatch cannot reveal it
   (feature branches read the base scope, hit the same key, and the cold compile reads as ordinary
   first-run cost). Precedent for the fix is the existing `-line-tables-only-` segment.

6. **`ci/affected-graph/README.md`** — it describes the parity gate as asserting each edge exists
   *and* schedules the upstream's build; that now holds for three tasks. Its strict-equality
   maintenance section governs the expected set being edited here.

### Out of scope, with reasons

- **`fmt` propagation** — crate-local by construction (see Rejected alternatives).
- **`build-release`** — also propagates across no edge, but it is not in `ci.yml`'s `moon ci` target
  list at all, so it never runs in CI. Propagation for it is moot. (An earlier draft of this spec
  claimed it was "covered by `:release-parity`"; that was wrong — `repo:release-parity` is a
  release-plz commit→semver dry run and compiles nothing.)
- **py/ts stacks.** The honest statement is narrower than "no analogous hole":
  - `py:lint` / `py:typecheck` live on the `py` configuration root, which has no `dependsOn` to any
    Rust project and no `rs/**` inputs — so a Rust kernel edit schedules **neither**. What closes
    the gap today is `paigasus-kernel-py:test`'s hand-written `/rs/...` inputs, and only at
    *runtime*: a PyO3 signature change that pytest does not exercise is a basedpyright finding that
    never runs on the introducing PR.
  - `typecheck` in `typescript-project.yml` carries no `deps` at all and structurally cannot
    propagate. It happens not to matter because `paigasus-kernel-ts` overrides `build` with a script
    ending in `tsc`, and no other ts package consumes the kernel bindings today — but
    `typescript-project.yml` itself warns that `build` is the override surface, and
    `paigasus-console-ts` already replaces it with `next build`.

  Both are real holes of the same shape, one stack over, and both are follow-ups rather than part of
  a `rust:`-scoped issue.
- **Dependency-bump-induced clippy breaks.** Measured: a `rs/Cargo.lock`-only touch schedules
  **no crate task at all** — only `repo:deny`, `repo:nats-permissions`, `repo:wasm-getrandom-free`.
  `lint`'s inputs are `@group(sources)`, `@group(tests)` and the project-local `Cargo.toml`; the
  workspace lockfile is not among them. So an external dependency bump that deprecates an API is not
  linted on the bumping PR — not even in the crate that uses it directly. This bounds what SMA-526
  achieves: it closes the **in-tree** upstream→consumer hole, not the external-dependency one.
  Adding `/rs/Cargo.lock` to lint's inputs would fix it but lints all 13 crates on every Dependabot
  PR, which is a spend decision of its own. Follow-up.
- **Two further pre-existing input gaps**, noted so "correct by construction" is not overclaimed:
  `cargo clippy --all-targets` compiles `build.rs`, but `rs/crates/bindings/paigasus-node-bindings/build.rs`
  is in no task's inputs; and `rs/rustfmt.toml` is in no `fmt` task's inputs, so a global
  format-config change re-keys nothing. Follow-ups.

### What A3-for-lint actually guards

Because the dep has exactly one declaration site, the widened assertion can only fire in the
"someone removed the line from `.moon/tasks/rust.yml`" case — where it fires for every crate at
once (17 violations today). It cannot catch a per-crate omission, because there is no per-crate
declaration to omit. That is the intended trade: the single site is what makes new crates correct
without action, and A3 is what stops that single site from being deleted silently.

## Verification

V1–V3 were run against the real workspace before this spec was written; V4–V6 are required before
the PR is opened.

**V1 — the fix schedules downstream lint.**

| touched file | lint tasks before | after |
|---|---|---|
| `rs/crates/libs/paigasus-proto/src/lib.rs` | 1 (the crate itself) | 4 (+ service-info, iam, gateway) |
| `rs/crates/libs/paigasus-kernel/src/lib.rs` | 1 (the crate itself) | 8 (+ iam-core, iam, kernel-parity, node/py/wasm bindings, gateway) |

**V2 — the extended A3 assertion is not vacuous.** With the `rust.yml` fix applied the parity gate
passes (`13 crates: every Cargo dep has a Moon edge that schedules its build`). With the fix
reverted and only the A3 extension in place it fails with 17 named violations — one per real
missing edge, e.g. `paigasus-service-info-rs:lint does not schedule paigasus-proto-rs:build`.

**V3 — measured CI cost.** With a warm `rs/target`, clippy on all seven downstream crates of a
kernel edit adds **~5s**, dominated by `paigasus-iam` at 4s. That figure is *serial*, and it should
not be divided by Moon's parallelism: concurrent cargo invocations serialize on the `rs/target`
lock. ~5s is therefore the realistic added wall-clock, not a number to be amortised. It is also a
warm-cache figure, which is only representative once scope item 5 lands.

**V4 — the negative control still passes.** `ci/affected-graph/run.sh --negative-control` must exit
0 *after* the fixture update in scope item 2. It currently exits 1 with the widened loop alone —
that is the BLOCKER this spec exists to record.

**V5 — blast radius is bounded and budgeted.** `.moon/tasks/rust.yml` appears in
`implicitInputs`, so this PR invalidates and re-runs every task that inherits it. Measured:
**70 tasks across 16 projects** — the 13 crates plus `paigasus-kernel-py`, `paigasus-kernel-ts` and
`repo`. It is *not* a whole-repo run; that is what editing `.moon/tasks.yml` would do (114 tasks /
28 projects). The run does include the Docker-gated `paigasus-iam-rs:test` suite, which is
documented as flaky under parallel load, and the napi + wasm-pack builds, against
`timeout-minutes: 30`. A red on those is to be diagnosed as flake-vs-regression by re-running,
not assumed to be this change.

**V6 — full CI graph.** Run the complete gate list from `CLAUDE.md`
(`moon ci :build :test :lint :fmt :deny :osv :machete :typecheck :breaking :affected-smoke …
--base origin/main --include-relations`) before pushing.

## Rollback

`repo:affected-smoke` is a required check and its expected sets are strict-equality, so a wrong
expected set reds `main` for every contributor until reverted. All six scope items are independent
of each other in `git` terms and the change is config-only — no crate source is touched — so
`git revert` of the single commit is a complete rollback with no data or schema implications. If
only the expected set is wrong, correcting scope item 3 alone is sufficient and preferable.
