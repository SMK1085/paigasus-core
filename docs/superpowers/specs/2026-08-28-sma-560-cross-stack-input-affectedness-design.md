# SMA-560 — Cross-stack input affectedness: assert the ADR-0005 guarantee, and close two input gaps

**Status:** Draft (brainstorming 2026-08-28).
**Date:** 2026-08-28
**Linear:** [SMA-560](https://linear.app/smaschek/issue/SMA-560) (lead) + [SMA-535](https://linear.app/smaschek/issue/SMA-535) + [SMA-537](https://linear.app/smaschek/issue/SMA-537) + [SMA-536](https://linear.app/smaschek/issue/SMA-536) (folded in — §2).
**Branch:** `feature/sma-560-assert-cross-stack-input-affectedness`
**Targets:** `main` (currently `2f37378`).
**References:** ADR-0005 (cross-language behaviour lives once in the kernel); SMA-528 (inputs are the only thing that confers affectedness; assertion A6); SMA-546 (workspace-level FFI inputs; A5); SMA-534 (`--locked` lint, workspace inputs); SMA-524 ("a MISSING case is how the bug survived"); SMA-429 (default-deny model); SMA-436 (the py typecheck vacuity guard); SMA-553 / SMA-556 (input liveness, dead inputs); SMA-541 (a new `repo:*` gate must be in both `T` and CLAUDE.md); SMA-433 (parity corpus); SMA-434 (ts glue drift — deliberately *not* here, §5); SMA-590 (`ty` swap — deliberately not here, §9).

---

## 1. Problem

One principle, three places it is unapplied or unasserted.

Moon 2.3.2 confers affectedness **only** through a task's own `inputs`. `dependsOn` and a
task-level `^:build` schedule an upstream's build but never *select* a downstream; measured at the
full 24-target shape, neither `--include-relations` nor `--downstream` changes that (SMA-528).
Every guarantee about "a change to X re-runs Y" is therefore a claim about Y's `inputs` and
nothing else.

**SMA-537 — two files are in no task's inputs.** `.moon/tasks/rust.yml` declares inputs as
`@group(sources)` (`src/**/*`), `@group(tests)`, `Cargo.toml` and the workspace-level files. A
crate-root `build.rs` matches none of them, so editing
`rs/crates/bindings/paigasus-node-bindings/build.rs` re-keys nothing and schedules nothing —
though `cargo build` runs it and `cargo clippy --all-targets` compiles it. Separately, `fmt` runs
`cargo fmt --check` with `inputs: ['@group(sources)']`, so the workspace-level `rs/rustfmt.toml`
(which sets `max_width = 200`) invalidates no `fmt` task: changing the global format config leaves
the whole tree free to drift out of compliance until some unrelated edit re-runs `fmt` per crate.

**SMA-535 — `py:typecheck` cannot be selected by a Rust change.** `py:lint`, `py:typecheck` and
`py:test` live on the `py` configuration-root project, which has no `dependsOn` to any Rust
project and no `rs/**` inputs on any task. A PyO3 signature change schedules none of them.

**SMA-560 — the ADR-0005 guarantee is unasserted.** SMA-528 gave every Rust crate a
`fileGroups.upstreams` and added assertion **A6** holding it to strict equality against Moon's own
closure. A6 iterates `language == "rust"` projects only. The py/ts wrappers reach the kernel
through hand-written globs instead, and those globs *are* the cross-binding guarantee — they are
what makes a kernel edit re-run the parity replay in each language. Deleting
`/rs/crates/libs/paigasus-kernel/src/**/*` from `paigasus-kernel-ts:build` leaves every gate green
while the ts parity replay silently stops running on kernel changes: the exact defect shape
SMA-528 fixed for Rust, one stack over.

### Verified against `main` @ `2f37378`

Every claim above was re-measured rather than taken from the issues, which were filed 8–12 days
before this spec and predate SMA-546, SMA-553 and SMA-572/573.

| Claim | Verdict |
| -- | -- |
| `build.rs` in no task's inputs | Holds — `sources` is `src/**/*`; no task lists it |
| `/rs/rustfmt.toml` in no `fmt` inputs | Holds — `fmt: inputs: ['@group(sources)']` |
| A6 iterates Rust only | Holds — `examined = {… if proj.get("language") == "rust"}` |
| py/ts wrapper globs ungated | Holds — A5 asserts only the four workspace-level files |
| Exactly one `build.rs` in the workspace | Holds — `rs/crates/bindings/paigasus-node-bindings/build.rs` |

---

## 2. Scope

The three issues as filed, **plus SMA-536** (the `ts:typecheck` twin of SMA-535). 535 and 536 are
the same defect one stack apart; fixing only py would leave an asymmetry that reads as deliberate
and is not.

### Out of scope, each with a reason

- **SMA-560's second case** — a `buf.gen.yaml` or codegen-plugin-version change regenerates output
  without touching a `.proto`, and selects no consumer task. Same *principle*, genuinely different
  *mechanism* (generate-output → committed code → consumers). It earns its own spec rather than
  riding along. **Needs a new issue.**
- **SMA-434** — the drift check for the committed napi/wasm glue. It is the ts analogue of this
  spec's new stub-drift gate (§4) and is already tracked. §5 records the resulting asymmetry
  explicitly so it does not read as an oversight.
- **SMA-590** — replacing basedpyright with `ty`. Checker-agnostic by construction (§4, D2).

---

## 3. SMA-537 — `build.rs` and `rustfmt.toml`

### D1. `rustfmt.toml` on `fmt`

`.moon/tasks/rust.yml`'s `fmt.inputs` gains `/rs/rustfmt.toml` (leading `/` = workspace-relative,
matching the `lint` task's existing workspace inputs). No ambiguity, no alternatives worth
recording. Note this is a *config*-edit hole, distinct from the `fmt` **propagation** question
SMA-526 considered and correctly rejected — `cargo fmt --check` reads only the crate's own files,
so it cannot be broken by an upstream *crate* edit, but it certainly can by a config edit.

### D2. `build.rs` — per-crate declaration plus a derived assertion

**Chosen:** declare `build.rs` only in
`rs/crates/bindings/paigasus-node-bindings/moon.yml` (`build`/`test`/`lint`), and add a parity-gate
assertion that **every crate with a `build.rs` on disk declares it**.

**Rejected — the shared template.** Adding `'build.rs'` to `.moon/tasks/rust.yml` is one line and
covers a future crate automatically, but it makes 12 of 13 crates declare a file that does not
exist. That is precisely the untracked-input class `repo:input-liveness` reds on, and that SMA-556
must clear before that gate can widen past `repo:*`. Harmless today — input-liveness covers
`repo:*` tasks only — but it is debt pointed the wrong way, and it would enlarge SMA-556's job.

**Rejected — per-crate with no assertion.** Smallest change, no dead inputs, but the next crate to
add a `build.rs` silently repeats the exact bug SMA-537 was filed for. This is SMA-524's "a MISSING
case is how the bug survived" lesson.

The chosen shape is the same derived-plus-floor pattern §6 uses one level up: derive from ground
truth (here, the filesystem) so a new instance is covered on day one, and keep the declaration
itself honest.

### R1. Implementation risk — does Moon append or replace inherited inputs?

The per-crate route assumes a project's `inputs:` on an **inherited** task *appends* to the
inherited list rather than replacing it. There is no `mergeInputs` setting anywhere in `.moon/`, so
Moon's defaults apply, and the default has not been verified on 2.3.2 in this repo.

**This must be measured before anything is built on it**, by comparing a task's resolved
`inputFiles` from `moon query projects` before and after the per-crate declaration. If Moon
*replaces*, the crate's `build`/`test`/`lint` would silently lose `@group(sources)` and every
workspace input — a far worse defect than the one being fixed, and one that would present as a
cache anomaly rather than an error. Fallback in that case: route `build.rs` through a
`fileGroups` entry the shared template already consumes.

---

## 4. SMA-535 — the stub is the interface

### F1. The filed fix would have passed vacuously

SMA-535 proposes adding `/rs/crates/{libs/paigasus-kernel,bindings/paigasus-py-bindings}/src/**/*`
to `py:typecheck`. Measured, that would schedule the gate without giving it anything new to see.

`basedpyright` resolves `paigasus_py_bindings` **from a hand-written `.pyi` stub**, not from the
Rust source and not from the compiled `.so`. maturin installs
`rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi` into site-packages as
`__init__.pyi`, beside a `py.typed` marker:

```
py/.venv/lib/python3.12/site-packages/paigasus_py_bindings/
  __init__.py
  __init__.pyi                     <- what basedpyright reads
  paigasus_py_bindings.abi3.so
  py.typed
```

So a Rust-source input would re-run basedpyright against the **same, possibly stale** stub and
pass. That is scheduling without coverage — the vacuous-pass shape SMA-436 and SMA-489 were both
filed for.

Two real holes sit behind it:

1. The `.pyi` is in **no task's inputs anywhere** — editing the stub alone re-keys nothing.
2. **Nothing asserts the stub matches the Rust.** Twelve `#[pyfunction]`s and twelve stub entries
   agree today, by hand. No drift gate exists in `moon.yml` or `ci/`.

### D3. Stub-as-interface, plus a name-level drift gate

- `.moon/tasks/python.yml`'s `typecheck.inputs` gains
  `/rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi`.
- A new gate asserts the stub's `def` names match the Rust `#[pyfunction]` set exactly.

**Names, not full signatures.** Name-level drift catches add / remove / rename — the failure modes
that actually occur — and is cheap and robust to parse. Comparing arity and types would require
parsing PyO3 attributes and mapping Rust types to Python ones (`f64`→`float`, `String`→`str`,
`PyResult` unwrapping), which is materially more work and brittle in a way that invites waivers.
Recorded as a deliberate limitation in §8 (L1) rather than left implicit.

**Where the coverage actually comes from.** The drift gate, not the scheduling. Keying
`py:typecheck` on the stub does **not** make a Rust-source edit select it, and that is correct —
basedpyright would only re-read the same stub. What makes a Rust signature change red is the drift
gate. The input keying closes the separate, smaller hole that editing the stub re-keys nothing.

This design is deliberately **checker-agnostic**: any conforming checker resolves a distributed
stub via `py.typed`, so SMA-590's `ty` swap neither blocks nor is blocked by it. Doing this first
makes that swap safer, because the drift gate catches a stub regression independently of which
checker runs.

---

## 5. SMA-536 — the ts twin, and where it stops

Same shape, different artifact. TypeScript's equivalent of the `.pyi` is the **committed napi and
wasm glue** (`index.d.ts`, `paigasus_wasm.d.ts`) — generated, committed, and read by `tsc` in place
of the compiled binary. So the ts typecheck path keys on the committed glue.

**The drift half is excluded**, because it is already SMA-434 ("CI drift check for committed FFI
binding glue (napi + wasm)"). The resulting asymmetry — py gets both halves here, ts gets only the
input keying — is deliberate and is stated here so it does not read as an oversight. SMA-434 does
for ts what §4's gate does for py.

Note the glue is *generated*: `paigasus-kernel-ts:build` regenerates it as part of `napi build`.
The plan must confirm that keying a typecheck task on a file another task regenerates does not
create a cyclic or self-invalidating input, and record the answer.

---

## 6. SMA-560 — assertion A7

A new `check_wrapper_upstream_inputs()` in `ci/affected-graph/cargo_moon_parity.py`, **alongside**
A6 rather than inside it: A6's strict equality is correct for Rust and must not be loosened.

### Measured, and why strict equality cannot be reused

| project | Rust `dependsOn` closure | crate dirs observed in inputs |
| -- | -- | -- |
| `paigasus-kernel-py` | kernel, py-bindings | kernel, py-bindings, **kernel-parity** |
| `paigasus-kernel-ts` | kernel, node-bindings, wasm | those three, **+ kernel-parity** |

The extra `kernel-parity` is the SMA-433 parity-vectors input. It is **correct and deliberate**, not
an over-approximation. `paigasus-kernel-py:build` and `paigasus-kernel-ts:typecheck` observe
nothing at all, and should not: they are the inherited wheel-build and `tsc` tasks.

A naive strict-equality extension therefore reports three false violations on an unmutated tree.

### D4. Derived closure + containment + explicit floor

- **Derived.** Reuse `rust_closure`, whose per-*dependency* `language != "rust"` filter stays
  exactly as is; only the *root* language filter changes. A fourth wrapper is covered the day it is
  added.
- **Containment, not strict equality.** Every closure member must be keyed on; extras are allowed.
  Rust's strict equality exists because `fileGroups.upstreams` is a mechanical mirror of the
  closure, so anything extra there is pure waste. The wrapper globs are hand-written per task and
  legitimately mixed with non-crate inputs — parity vectors, `package.json`, `pyproject.toml`,
  `uv.lock`. Strict equality there compares apples to a fruit salad, and would force waivers that
  record *correct* inputs as tolerated defects. That is worse than a weaker assertion: it teaches
  the next reader that a right thing is wrong.
- **Explicit floor.** `REQUIRED_WRAPPER_TASKS = {paigasus-kernel-py:test, paigasus-kernel-ts:build,
  paigasus-kernel-ts:test}`, mirroring `REQUIRED_FFI_TASKS`' role for A5. A derived set that
  shrinks to empty asserts nothing while printing PASS — a moon rename or JSON reshape would do
  exactly that. Every task named here must appear in the derived set or A7 fails.
- **Negative control** in `self_test()` asserting the **specific violation row**, not mere
  non-emptiness. This is the file's own stated convention (see A6's controls) and the
  guard-the-guard lesson from SMA-542: a control that only checks "something failed" passes when
  the wrong thing fails.

### D5. Task selection is explicit, not derived

A7 cannot ask "every task on the project": `paigasus-kernel-py:build` and
`paigasus-kernel-ts:typecheck` legitimately observe nothing, so a whole-project rule would red on
an unmutated tree.

To be unambiguous about what "derived" covers here — the *closure* is derived per wrapper, and the
*set of tasks examined* is not. A7 examines exactly the tasks named in `REQUIRED_WRAPPER_TASKS`.
That is a narrower derivation than A6's (which examines every task in `UPSTREAM_INPUT_TASKS` on
every Rust crate), and it is the reason the floor is load-bearing rather than merely defensive: for
A7 the floor **is** the task set, so an omission from it is not a vacuity risk but a coverage hole.
A new wrapper therefore needs a floor entry — the one hand-maintained part of this design, and the
price of the wrappers having no uniform task shape.

---

## 7. Testing

Every gate must demonstrate it bites. Assertions that cannot be shown to fail are not covering
anything — the standard SMA-560 itself sets.

1. **A7 positive.** Passes on the unmutated tree, with all three floor tasks in the derived set.
2. **A7 negative, per row.** Delete `/rs/crates/libs/paigasus-kernel/src/**/*` from
   `paigasus-kernel-ts:build`; confirm A7 reds **naming that specific consumer and upstream**;
   restore; confirm green. Repeat for `paigasus-kernel-py:test`.
3. **A7 floor.** Simulate an emptied derivation and confirm the `FLOOR:` rows fire — distinguishable
   from a per-wrapper failure, as A6's controls already are.
4. **`build.rs` assertion.** Remove the per-crate declaration; confirm red naming the crate; restore.
5. **Stub drift gate.** Add a `#[pyfunction]` without touching the stub; confirm red naming the
   missing symbol. Then the reverse: add a stub entry with no Rust function.
6. **`rustfmt.toml` input.** Confirm editing `rs/rustfmt.toml` now selects the `fmt` tasks, via
   `moon query tasks --affected`. Note this is a **new ad-hoc probe**, not an existing `run.sh`
   case — no `run.sh` case edits that file today, which is exactly why the re-baseline below may
   turn out empty. The two statements are consistent: the input genuinely changes behaviour, and
   no *recorded* case exercises it.
7. **Merge-strategy probe (R1).** Before anything else — resolved `inputFiles` before and after the
   per-crate declaration.
8. **Full graph.** `moon ci` with all 27 targets, `--base origin/main --include-relations`. A new
   `repo:*` gate reds `:affected-smoke` until it is in **both** `ci.yml`'s `T=(…)` and CLAUDE.md's
   marker-delimited command (SMA-541).

### The re-baseline — measure, do not assume

The stated rationale for combining these three was that each re-baselines
`ci/affected-graph/run.sh`'s strict-equality expected sets, so the re-baseline should happen once.
**On reflection that rationale is weaker than claimed and must be measured, not assumed.** The new
inputs are `/rs/rustfmt.toml`, a `.pyi`, and a `build.rs` — and no existing case in `run.sh` edits
any of those files, so the expected sets may not move at all.

The combination remains right on other grounds: one coherent principle, one review cycle, one PR,
and one gate file (`cargo_moon_parity.py`) touched by two of the three changes. But the plan must
report what the affected sets actually do rather than inheriting the assumption.

---

## 8. Limitations, stated

- **L1 — stub drift is name-level only.** A changed parameter *type* that keeps the name passes
  (§4, D3). Closing it needs a Rust→Python type mapping; deferred deliberately, not overlooked.
- **L2 — A7 is containment, so a wrapper may over-declare permanently.** A glob for a crate no
  longer in the closure stays green. Accepted: for wrappers the extra inputs are hand-written with
  stated reasons, and the cost of a stale glob is a spurious re-run, not a missed one. The
  dangerous direction — a *missing* glob — is what A7 catches.
- **L3 — A7 asserts crate-dir granularity, not glob shape.** Keying on
  `<crate>/Cargo.toml` alone satisfies A7 even though `src/**/*` is what matters, because the
  upstream half is recovered structurally from the first four path segments (the same recovery A6
  uses). Tightening it would require A7 to know which suffix each consumer needs.

---

## 9. Open questions for the plan

1. **R1's answer** — does Moon append or replace? Everything in §3 D2 depends on it.
2. **Where the stub-drift gate lives.** A new `repo:pyo3-stub-drift` task, or folded into an
   existing gate's script. A new `repo:*` task carries the full SMA-541 wiring cost (`T`,
   CLAUDE.md's marker block) plus a `repo:input-liveness` obligation that its declared inputs stay
   live.
3. **§5's regenerated-glue question** — is keying a typecheck task on a file another task
   regenerates sound, or self-invalidating?
4. **Does `ts:typecheck` exist as a distinct task worth keying**, or is the committed-glue input
   better placed on the inherited `tsc` task that SMA-536 says a build override currently hides?
