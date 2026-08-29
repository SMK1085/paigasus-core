# SMA-601 — make a truncated `rs/Cargo.lock` red a required check

Status: revised after adversarial review (2026-08-29)
Linear: [SMA-601](https://linear.app/smaschek/issue/SMA-601/ci-no-required-check-catches-a-truncated-rscargolock)

## 1. Problem

Dependabot cargo PRs repeatedly ship a truncated `rs/Cargo.lock`. Recorded
occurrences: PR 83 (530 to 158), PR 96 (to 170), PR 140 (543 to 172), PR 149
(543 to 176), PR 181 (543 to 176). The required `moon ci` check passes. Only
`images.yml`'s `build + smoke` job reds, and that workflow is not required, so
branch protection permits the merge.

A merge with a truncated lock un-pins about 370 crates on `main`. Cargo then
resolves them freely on the next build. The damage is silent, because
`cargo build` without `--locked` still succeeds.

## 2. Root cause — measured

SMA-601 attributes the green report to `repo:publish-metadata`'s publish groups
happening to be satisfied by the surviving subset. That is true but it does not
explain the report, because `.moon/tasks/rust.yml`'s `lint` task already runs
`cargo clippy --locked --all-targets` on all thirteen crates and declares
`/rs/Cargo.lock` among its `inputs`.

Measured on PR 181's truncated commit `72c0ddb52` — 176 packages against
`main`'s 543, holding 5 of the 13 workspace members:

| Measurement | Result |
| -- | -- |
| `cargo metadata --locked` from `rs/` | exit 101 |
| Same, from `rs/crates/services/paigasus-gateway/` | exit 101 |
| `paigasus-gateway-rs:lint` on PR 181 (`cargo clippy --locked`) | pass, 23s 958ms, not cached |
| `paigasus-iam-rs:lint` on PR 181 (`cargo clippy --locked`) | pass, 1m 11s 740ms, not cached |

Both `--locked` clippy runs executed for real and passed. They should have
failed. The reason is that an earlier task repaired the lock on disk:

| Command | `--locked`? | Lock before to after | Exit |
| -- | -- | -- | -- |
| `cargo tree -p paigasus-wasm --target wasm32-unknown-unknown -e no-dev` (`repo:wasm-getrandom-free`) | no | 176 to 548 | 0 |
| `cargo deny --manifest-path rs/Cargo.toml check` (`repo:deny`) | no | 176 to 548 | 0 |

**An unlocked cargo invocation re-resolves and rewrites an inconsistent lock in
place, mid-run, before any `--locked` task reaches it.** The repaired lock
exists only in the runner's workspace and is never committed, so `main` keeps
the truncated file.

### 2.1 Ordering — from timestamps, not durations

The first draft of this spec inferred ordering from per-task durations, which
is invalid under a parallel scheduler. Re-derived from the task start
timestamps in job 99064479471:

| Task | Start | Note |
| -- | -- | -- |
| `repo:deny` | 06:37:55 | unlocked `cargo deny` |
| `repo:wasm-getrandom-free` | 06:37:55 | unlocked `cargo tree` |
| `paigasus-kernel-rs:lint` | 06:38:07 | first `--locked` task, 12s later |
| `paigasus-kernel-ts:build` | 06:41:03 | unlocked cargo via `napi` / `wasm-pack` |
| `paigasus-kernel-py:test` | 06:41:24 | unlocked cargo via `uv sync` / maturin |
| `paigasus-gateway-rs:lint` | 06:41:25 | |
| `paigasus-iam-rs:lint` | 06:41:41 | |

`repo:deny` and `repo:wasm-getrandom-free` start together, 12 seconds before
the first `--locked` task and about three minutes before the FFI tasks. So the
two named commands are the actual first repairers, and the FFI tasks are not.
Moon's own toolchain setup is ruled out separately: the run report records
`SetupToolchain(rust:1.95.0)`, `SetupEnvironment(rust, rs)` and
`InstallDependencies(rust, rs)` all as **skipped**.

### 2.2 What this does and does not prove

`repo:deny` audits a re-resolved graph **whenever the lock does not already
satisfy the manifests**. On a normal PR with a consistent lock, cargo changes
nothing and `cargo deny` audits exactly the pinned graph. The hole is therefore
not "live on every PR"; it is live on precisely the PRs this issue is about,
which is also when it matters most. `.moon/tasks/rust.yml:69-71` states the
same distinction correctly.

## 3. Design

Three parts. Part 1 is the detector and stands alone. Parts 2 and 3 are
defence in depth and audit honesty, and Part 1 deliberately does not depend on
either of them holding.

### 3.1 Part 1 — an unconditional `ci.yml` step

Add a step to the `ci` job, **after** the cargo cache restore and **before**
the `moon ci` step:

```yaml
- name: Cargo lockfile integrity (rs/Cargo.lock satisfies every manifest)
  run: |
    set -euo pipefail
    bash ci/cargo-lock-integrity/run.sh --self-test
    bash ci/cargo-lock-integrity/run.sh --negative-control
    bash ci/cargo-lock-integrity/run.sh
```

All three modes run, in the order every self-scheduled gate in this repo uses.
The bare mode alone is not enough: with `--locked` deleted from `run.sh`'s
`cargo metadata` line that command exits `0` **and repairs the lock itself**,
so the gate would print "satisfies every manifest" and become the first
repairer. Only `--negative-control` catches that, so it must execute in CI —
the same reason `release-parity`, `version-lockstep` and `workflow-credentials`
run their controls there.

Nothing has run yet at that point, so the working tree still holds the
committed lock. The check is therefore race-free by **placement** rather than
by construction, and needs no temp directory and no `git archive`.

This follows the codegen-drift precedent at `.github/workflows/ci.yml:249-262`,
which CLAUDE.md describes as "deliberate and load-bearing … the step carries no
`if:`, so it runs on EVERY CI run and cannot be deselected, where a `T`-array
task would run only when affected and a wrong `inputs` list would switch it off
silently". The required check is the whole job (`ci.yml:21-22`, `name: moon
ci`), so a failing step reds it.

The script runs:

```
( cd rs && cargo metadata --locked --format-version 1 >/dev/null )
```

Measured: exit 0 in 0.189s on `main`'s 543-package lock; exit 101 on PR 181's
176-package lock, leaving the lock unmodified at 176.

**Failure classification is mandatory.** `cargo metadata` exits 101 for a
broken lock, a malformed manifest, and a registry outage alike. Without
classification a crates.io outage becomes a false red on a required check. The
repository already solved this: `classify_cargo_failure` at
`ci/publish-metadata/run.sh:589-604` classifies on stderr and returns 2 for
infrastructure and 1 for a real assertion failure. This gate reuses that
convention — **rc 1 = the lock does not satisfy the manifests, rc 2 =
infrastructure, the gate asserted nothing**. rc 2 must be visibly distinct in
the step output, so an outage never reads as a clean pass.

No `--offline` and no `--frozen`: on a cold cargo cache `cargo metadata` needs
the registry index to resolve transitive manifests, so `--offline` would report
a false red.

**Which ref is validated.** `actions/checkout` at `ci.yml:47-53` passes no
`ref:`, so on a `pull_request` event the checkout is `refs/pull/N/merge` and
the step validates the **merge result**. On a `push` to `main` it validates
that commit. Validating the merge result is the intended subject: it is the
tree that will exist on `main`.

**Anti-deletion pin.** The codegen-drift precedent has no pin against its own
deletion, and that residual is not inherited here. `ci/actionlint/run.sh`
already reads `ci.yml` and pins content in it (check 8's `T` assertion, the
`continue-on-error` scan, `T_AFFECTED_SMOKE_REQUIRED_SCRIPT`). A new check
there asserts this step is present and that its `continue-on-error:` value is
absent or the literal `false`. That is one pin in an independently scheduled
gate, in place of the eight registry obligations a `repo:*` task would carry.

### 3.2 Part 2 — the flag fix

Eight declarations gain `--locked`. They expand to 44 of the 57 cargo-resolving
task invocations in the graph (see 3.3 for how that set is derived).

| Declaration | Now | Expands to |
| -- | -- | -- |
| `.moon/tasks/rust.yml` `build` | `cargo build` | 13 tasks |
| `.moon/tasks/rust.yml` `build-release` | `cargo build --release` | 13 tasks |
| `.moon/tasks/rust.yml` `test` | `cargo nextest run --no-tests=pass` | 13 tasks |
| `moon.yml:19` `repo:deny` | `cargo deny --manifest-path …` | 1 |
| `moon.yml:218` `repo:parity-corpus-drift` | `cargo run -p paigasus-kernel-parity …` | 1 |
| `moon.yml:242` `repo:observability-drift` | `cargo nextest run …` | 1 |
| `moon.yml:270` `repo:nats-permissions` | `cargo nextest run …` | 1 |
| `moon.yml:322` `repo:wasm-getrandom-free` | `cargo tree -p paigasus-wasm …` | 1 |

`repo:parity-corpus-drift`'s `cargo run` was missing from the first draft.

All three tools accept the flag. Both directions are measured for the two
proven repairers:

| Command | Truncated lock | Lock after | `main` lock |
| -- | -- | -- | -- |
| `cargo tree --locked -p paigasus-wasm …` | exit 101 | unchanged, 176 | exit 0 |
| `cargo deny --locked --manifest-path …` | exit 1 | unchanged, 176 | exit 0 |

`cargo deny` reports 1 rather than cargo's 101 because it wraps the resolution
failure. Part 3 asserts on the presence of the flag, not on an exit code, so
this does not matter to it.

`cargo nextest run --locked`, `cargo build --locked` and `cargo run --locked`
are not separately measured against a truncated lock, because each requires a
full compile. Implementation verifies them on the real graph.

**Three tasks cannot be fixed by a flag.** `paigasus-kernel-ts:{build,test}`
and `paigasus-kernel-py:test` reach cargo through wrappers, and all three
declare `/rs/Cargo.lock` among their inputs (SMA-546), so a lock-only
Dependabot PR selects them:

* `wasm-pack build` — **not fixable, despite appearances**. Its
  `[EXTRA_OPTIONS]...` positional is documented as "List of extra options to
  pass to `cargo build`", and the passthrough genuinely reaches that forwarded
  build — `wasm-pack build … -- --zzz-not-a-real-cargo-flag` is rejected with
  exit 1, proving the flag arrives. But `wasm-pack build … -- --locked`,
  measured against PR 181's truncated 176-package lock, still exits 0 and
  rewrites the lock 176 -> 548: wasm-pack makes its own **unlocked** cargo call
  BEFORE the build it forwards to, repairs the lock there first, and the
  forwarded `--locked` then sees an already-valid lock. `--locked` is kept on
  both invocations anyway — it does constrain the forwarded `cargo build`
  itself (`cargo build --lib --release --locked --target
  wasm32-unknown-unknown` against the same truncated lock exits 101) — but it
  cannot guarantee a locked resolution for the task as a whole.
* `napi build` — **not fixable**. Measured against the pinned CLI: it exposes
  `--target`, `--target-dir`, `--profile`, `--features`, `--cross-compile` and
  more, but **no `--locked` and no cargo passthrough**. Cargo has no
  environment-variable equivalent of `--locked` either.
* `uv sync --reinstall-package paigasus-py-bindings` — **not fixable**. It
  drives maturin, which drives cargo, with no flag path through either.

These three are a stated residual, not an oversight. They are acceptable
precisely because Part 1 does not depend on Part 2: the detector has already
run and reported before any of them starts.

**Cost.** This changes local ergonomics. Editing a dependency in a `Cargo.toml`
and then running `moon run <crate>:build` now errors instead of resolving
silently. `lint` has behaved this way since SMA-534, so a developer already
meets the error on the same edit. The comment claiming that "among the compile
gates, only `lint` passes `--locked`" lives at `.moon/tasks/rust.yml:71-74`,
not in CLAUDE.md as the first draft stated, and it is rewritten in the same
commit. That comment also carries a stale citation
(`ci/publish-metadata/run.sh:243,258`; the real `--locked` sites are `:742` and
`:812`), corrected at the same time.

### 3.3 Part 3 — assert no unlocked cargo invocation returns

Part 2 is a one-time edit. Without a guard, the next task to be added restores
the hole. The guard derives its invocation set from **moon's resolved task
graph**, not from file text.

The first draft specified a text scan of `moon.yml`, `.moon/tasks/*.yml`,
`rs/Dockerfile` and `ci/**/*.sh`. That was measured and rejected: the resolving
verb set matches **45 times across those files, of which roughly 14 are real
invocations**. The rest are prose and string literals, and comment stripping
does not help — `moon.yml:323` is `echo "cargo tree failed for paigasus-wasm
…"` on an executing line, and `ci/publish-metadata/run.sh:179` is a Python
f-string inside a heredoc inside a `.sh` file. The gate would also collide with
its own self-test fixtures and its own script.

Measured replacement: parsing `moon query projects` and matching each task's
resolved `command` + `args` + `script` yields **57 cargo-resolving invocations,
44 unlocked and 13 locked, with zero prose false positives** on today's graph.
This is the mechanism `derive_ffi_tasks` already uses
(`ci/affected-graph/cargo_moon_parity.py:299-320`, `FFI_MARKERS` at `:118`),
and it covers every project's `moon.yml` including `ts/` and `py/`, which the
file scan's surface did not.

The check therefore lands **inside `cargo_moon_parity.py`**, which
`repo:affected-smoke` already runs. Consequences: no new `repo:*` task, no `T`
entry, no CLAUDE.md marker change, no `SELF_SCHEDULED_GATES` entry, no
`SELF_TASK_EXPECTED_GLOBS` entry, no script-call-site pin, and no
`REQUIRED_REPO_TASKS` entry. It carries an anti-vacuity floor in the shape of
`REQUIRED_FFI_TASKS` (`cargo_moon_parity.py:127`), so an empty derived set
fails rather than passing silently.

`rs/Dockerfile` is checked separately by a single-line text assertion — it is
one `RUN cargo build --release --locked` line (`rs/Dockerfile:27`), already
correct, and moon cannot see it. `repo:affected-smoke` gains `rs/Dockerfile` as
an input so the assertion is not served from cache, and
`T_AFFECTED_SMOKE_REQUIRED_INPUTS` in `ci/actionlint/run.sh` gains the same
entry. That check tests **containment**, not order
(`ci/actionlint/run.sh:2321-2331`, and the comment at `:2087`: "CONTAINMENT,
not equality"), so no ordering work is needed and the arity floor of 20 at
`ci_targets.py:657` stays satisfied at 21.

A task that must stay unlocked needs an `ALLOW_UNLOCKED_CARGO` entry carrying a
reason, following `T_EXEMPT` / `ALLOW_DEAD_INPUT` / `BRANCH_SKIP` /
`ALLOW_NO_CARGO_BACKING`. After Part 2 the expected membership is the three
wrapper tasks of 3.2, each with its measured reason.

## 4. Registry obligations

Four edits, not the eight a new `repo:*` gate would carry:

1. A new check in `ci/actionlint/run.sh` pinning the `ci.yml` step's presence
   and its `continue-on-error` value.
2. `T_AFFECTED_SMOKE_REQUIRED_INPUTS` in `ci/actionlint/run.sh` gains
   `rs/Dockerfile`.
3. `repo:affected-smoke`'s `inputs` in `moon.yml` gain `rs/Dockerfile`.
4. A CLAUDE.md gotcha entry recording the repair mechanism of section 2, which
   is the non-obvious part and which nothing in the code states.

## 5. Testing

1. Fixture tables for Part 3's derived-set check, count-asserted, driven from a
   synthetic `moon query projects` payload rather than the live graph, and
   written to a scratch directory outside `ci/**`.
2. A negative control for Part 1, asserted through the same function the real
   run calls, and proving the rc 1 / rc 2 split: a synthetic stderr resembling
   a registry outage must report rc 2, not "reported red as expected".
3. Replay Part 1 against PR 181's `rs/Cargo.lock` (fetched from `72c0ddb52`):
   must red with rc 1.
4. Replay against `main`'s lock: must pass.
5. Re-run the Part 2 measurements after the edit lands.
6. Run the full graph as CI does, using the marker-delimited command in
   CLAUDE.md.

## 6. Decisions and rejected alternatives

**An unconditional `ci.yml` step, not a `repo:*` Moon task.** This reverses the
first draft. The `repo:*` route required a temp-directory extraction of
`git archive HEAD rs` to escape the mid-run repair race, plus eight registry
obligations, plus Moon's per-task floor, which this repository measures at
roughly 11s (`moon.yml:668-669`). Placing the step before `moon ci` removes the
race outright, so the extraction is unnecessary, and removes every registry
obligation. The codegen-drift step is the standing precedent for exactly this
trade.

**This supersedes the earlier "validate committed HEAD, warn if `rs/` is
dirty" decision.** That choice existed only to escape the race. With the step
running before anything else in the job, the working tree *is* the committed
tree, so the two collapse into one. The local-ergonomics argument that favoured
HEAD disappears with it: run locally, the script now reports on the tree the
developer actually has, which is the more useful answer and needs no dirty-tree
note.

**A derived invocation set, not a text scan.** Measured: 45 matches with ~14
real, against 57 matches with zero false positives. See 3.3.

**No package-count floor.** SMA-601 raises this. Rejected: `cargo metadata
--locked` tests a property, not a proxy, and a count needs re-baselining
whenever a dependency is added or removed.

**`images.yml` stays non-required.** It builds two `--release` images and its
`pull_request` trigger has a narrow path filter, so it does not run on every
PR. Not needed once the required check reds.

## 7. Limitations and residuals

* **`--locked` proves consistency, not correctness.** A lock that is complete
  but wrong — a version swapped for another that still satisfies every
  requirement, a tampered `checksum`, a removed `[patch]` — passes
  `cargo metadata --locked`. This gate detects truncation and any other
  inconsistency with the manifests. It is not a lockfile-tampering detector,
  and nothing here becomes one.
* **`napi build`, `uv sync`/maturin, and `wasm-pack build` cannot be locked**
  (measured, 3.2). `napi build` and `uv sync` expose no flag or passthrough at
  all. `wasm-pack build … -- --locked` forwards the flag to its `cargo build`
  call, but wasm-pack's own pre-build cargo call is unlocked and repairs an
  inconsistent lock first, so the forwarded flag sees an already-valid lock.
  Three tasks therefore still re-resolve. Part 1 has already reported before
  they run, so they cannot mask a truncated lock, but their own cargo work is
  not audited against the shipped resolution.
* **Gate scripts under `ci/**` are outside Part 3's derived set.** A cargo call
  inside a `.sh` invoked by a Moon task is not in moon's resolved command
  string. Today's instances are `ci/version-lockstep/run.sh`'s deliberate
  `cargo update -w` writers, which run only in `--write` mode, and
  `ci/publish-metadata/run.sh`'s `cargo metadata --no-deps`, which performs no
  resolution. A text scan over those files was measured to have an
  intolerable false-positive rate and is deliberately not built.
* **The sibling lockfiles are out of scope.** `ts` is already safe
  (`ci.yml:177`, `pnpm --dir ts install --frozen-lockfile`). `py` is not:
  CLAUDE.md records that `py`'s `moon.yml` runs bare `uv sync`, not `--locked`,
  and that `py/uv.lock` "drifts SILENTLY". That is a separate issue.
* **`wheels.yml` and `prebuild.yml` are out of scope.** Both invoke cargo and
  maturin, neither runs inside `moon ci`. A truncated lock would produce
  silently different published wheels. Separate issue.
* **A task whose script mentions cargo in a string but never runs it would be a
  Part 3 false positive.** There are none today. When one appears it takes an
  `ALLOW_UNLOCKED_CARGO` entry with a reason.
* **Nothing asserts that a future cargo-invoking `repo:*` gate declares
  `rs/.cargo/config.toml` in its `inputs`.** Pre-existing, recorded in
  CLAUDE.md, not closed here.
