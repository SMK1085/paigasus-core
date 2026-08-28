# SMA-560 — Cross-stack input affectedness: assert the ADR-0005 guarantee, and close two Rust input gaps

**Status:** Draft, rev 2 (brainstorming 2026-08-28; **adversarial review incorporated — B1–B4, M1–M8, m1–m4**; **scope reduced 2026-08-28 — §2**).
**Date:** 2026-08-28
**Linear:** [SMA-560](https://linear.app/smaschek/issue/SMA-560) (lead) + [SMA-537](https://linear.app/smaschek/issue/SMA-537).
**Removed from scope in rev 2:** [SMA-535](https://linear.app/smaschek/issue/SMA-535) and [SMA-536](https://linear.app/smaschek/issue/SMA-536) — §2.1. Findings recorded on both issues.
**Branch:** `feature/sma-560-assert-cross-stack-input-affectedness`
**Targets:** `main` (currently `2f37378`).
**References:** ADR-0005; SMA-528 (inputs are the only thing that confers affectedness; A6); SMA-546 (A5); SMA-534 (A4, workspace lint inputs); SMA-524 ("a MISSING case is how the bug survived"); SMA-429 (default-deny); SMA-542 (guard the guard: a check's own call site is what goes unguarded); SMA-553 / SMA-556 (input liveness, dead inputs); SMA-541 (a new `repo:*` gate must be in both `T` and CLAUDE.md); SMA-433 (parity corpus); SMA-434 (ts glue drift).

---

## 1. Problem

Moon 2.3.2 confers affectedness **only** through a task's own `inputs`. `dependsOn` and a
task-level `^:build` schedule an upstream's build but never *select* a downstream; measured at the
full 24-target shape, neither `--include-relations` nor `--downstream` changes that (SMA-528).
Every guarantee of the form "a change to X re-runs Y" is therefore a claim about Y's `inputs` and
nothing else.

Two consequences, both open.

### 1.1 Two files are in no *crate* task's inputs (SMA-537)

`.moon/tasks/rust.yml` declares crate task inputs as `@group(sources)` (`src/**/*`),
`@group(tests)`, `Cargo.toml`, and — on `lint` only — three workspace-level files. So:

- A crate-root `build.rs` matches none of them. Editing
  `rs/crates/bindings/paigasus-node-bindings/build.rs` re-keys no crate task, though `cargo build`
  runs it and `cargo clippy --all-targets` compiles it.
- `fmt` runs `cargo fmt --check` with `inputs: ['@group(sources)']` alone
  (`.moon/tasks/rust.yml:83-85`), so `rs/rustfmt.toml` (`max_width = 200`) invalidates no `fmt`
  task, and neither does `@group(tests)` nor `rs/rust-toolchain.toml` (which selects the rustfmt
  binary).

*Both files do match some broad `repo:*` task input — `repo:machete`'s `rs/**/*.rs`,
`repo:publish-metadata`'s `rs/crates/**/*`, and the two `['**/*']` gates. The claim is specifically
about crate `build`/`test`/`lint`/`fmt`, which is where the invalidation matters (m1).*

### 1.2 The wrappers' Rust globs are only partly asserted (SMA-560)

SMA-528 gave every Rust crate a `fileGroups.upstreams` and added **A6**, holding it to strict
equality against Moon's own closure. A6 iterates `language == "rust"` projects only
(`cargo_moon_parity.py:331`). The py/ts wrappers reach the kernel through hand-written per-task
globs instead, and those globs *are* the ADR-0005 cross-binding guarantee.

**Correction to rev 1 (B3).** Rev 1 claimed "deleting `/rs/crates/libs/paigasus-kernel/src/**/*`
from `paigasus-kernel-ts:build` leaves every gate green." **That is false**, and the adversarial
review caught it. `ci/affected-graph/run.sh:343` — `run_task_case_ci "kernel->consumer-tasks"`,
strict equality — explicitly lists `paigasus-kernel-ts:build`, `paigasus-kernel-ts:test` and
`paigasus-kernel-py:test` in the expected set for a touch of
`rs/crates/libs/paigasus-kernel/src/lib.rs`. Removing that glob makes the task un-affected and the
case reports it `missing`. The comment directly above that line even says those three "key on the
kernel's sources by hand (SMA-420/546)".

So **the kernel→wrapper edge is already covered**, by one hand-written `run.sh` case.

#### What is genuinely uncovered

Three things, and they are what A7 exists for:

1. **Every non-kernel upstream.** No `run.sh` case touches
   `rs/crates/bindings/paigasus-wasm/src/**` *and* asserts `paigasus-kernel-ts:test`. Deleting the
   wasm glob from `paigasus-kernel-ts:test` is green today. The project-level `binding-oneway-wasm`
   case stays green via `:build`'s own wasm glob, so it does not catch it either.
2. **A new wrapper, or a new upstream on an existing one.** `run.sh`'s coverage is a hand-written
   case list; A6 exists because a *missing* case is how SMA-524's bug survived review. The same
   argument applies one stack over.
3. **A live under-declaration, today.** Neither wrapper declares the kernel's `Cargo.toml` —
   verified, zero occurrences in both `moon.yml` files — while A6 demands `src/**/*` **and**
   `Cargo.toml` per upstream for every Rust crate (`cargo_moon_parity.py:357-360`), and
   `repo:parity-corpus-drift` lists it explicitly for exactly this hazard ("a kernel Cargo.toml
   change (a future `[features]` toggle altering `sum`) must re-key the gate", `moon.yml:214-217`).
   A future `[features]` toggle in the kernel would change wrapper behaviour without re-keying
   either wrapper task.

Item 3 is a defect to **fix**, not merely to assert (§4.4).

### Verified against `main` @ `2f37378`

| Claim | Verdict |
| -- | -- |
| `build.rs` in no *crate* task's inputs | Holds — `sources` is `src/**/*`; no crate task lists it |
| `rs/rustfmt.toml` in no `fmt` inputs | Holds — `fmt: inputs: ['@group(sources)']` |
| A6 iterates Rust only | Holds — `examined = {… if proj.get("language") == "rust"}` |
| Exactly one `build.rs` in the workspace | Holds — `rs/crates/bindings/paigasus-node-bindings/build.rs` |
| Neither wrapper declares the kernel's `Cargo.toml` | Holds — 0 occurrences in both wrapper `moon.yml`s |
| Kernel→wrapper edge already covered by `run.sh` | Holds — `run.sh:343`, strict equality |
| `repo:affected-smoke` has no `build.rs` glob | Holds — `moon.yml:165-202`; nearest is `rs/**/Cargo.toml` |

---

## 2. Scope

**SMA-560 and SMA-537 only.**

### 2.1 Why SMA-535 and SMA-536 left (B1, B2)

Rev 1 folded both in and designed a "key the typecheck task on the interface artifact" fix for
each. The adversarial review showed both are **vacuous by the same mechanism rev 1 used to reject
SMA-535's originally filed fix** — the reasoning was applied one level up and not one level down.

- **py:** `py/pyproject.toml:12` sets `include = ["packages/*/src/**", "packages/*/tests/**"]`, so
  basedpyright never opens the source stub; it reads the maturin-**installed** copy.
  `.moon/tasks/python.yml:38` runs plain `uv run basedpyright` with no `--reinstall-package`, and
  this repo has already measured that plain `uv run` serves a cached wheel
  (`ci/affected-graph/run.sh:325-329`). An input on the source stub is a pure cache key over a file
  the task never reads.
- **ts:** `tsc` resolves `@paigasus/node-bindings` through `node_modules` with no `paths` mapping,
  and pnpm copies a `file:` dep into its store and never re-copies
  (`ts/packages/paigasus-kernel/moon.yml:99-102`, measured). Same shape.

Both issues' real problem is *make the checker read a fresh artifact*, which is a different and
larger fix (uv `cache-keys` or `--reinstall-package`; a tsconfig `paths` mapping or `link:` dep).
Full analysis is recorded on each issue, including a third finding — a correct PyO3 stub-drift gate
must compare **three** sets (`#[pyfunction]` idents, `wrap_pyfunction!` registrations, stub `def`
names), because an unregistered `#[pyfunction]` does not exist at runtime.

### 2.2 Also out of scope

- **SMA-560's codegen case** — a `buf.gen.yaml` or plugin-version change regenerates output without
  touching a `.proto`. Same principle, different mechanism. **Needs a new issue.**
- **SMA-434** — ts glue drift, already tracked.
- **SMA-590** — the `ty` swap.

### 2.3 What still justifies one PR

Rev 1 claimed these belong together because each re-baselines `run.sh`'s strict-equality expected
sets. **That was probably false** and the adversarial review independently agreed: no existing case
edits `rs/rustfmt.toml`, `build.rs`, or `@group(tests)` in a way that moves a recorded set, and
`fmt` is outside `run.sh`'s task-case name filter entirely (`run.sh:94-97`, m4).

The surviving grounds are narrower and honest: **one principle, one review cycle, and one file** —
`cargo_moon_parity.py` gains both new assertions, and `repo:affected-smoke`'s `inputs` need one
edit that serves both. §6 requires the plan to report what the affected sets actually do rather
than inherit the assumption.

---

## 3. SMA-537 — the two Rust input gaps

### D1. `fmt` inputs, widened past what SMA-537 asked for (M3)

`.moon/tasks/rust.yml`'s `fmt.inputs` becomes:

```yaml
inputs: ['@group(sources)', '@group(tests)', '/rs/rustfmt.toml', '/rs/rust-toolchain.toml']
```

SMA-537 asks only for `rustfmt.toml`. That stops three-quarters short: `cargo fmt --check` formats
**every** target in the package — `src/**`, `tests/**`, `benches/**`, `build.rs` — so a misformatted
integration test can merge green today and red `main` on an unrelated later `src` edit.
`rust-toolchain.toml` selects the rustfmt binary, which is the same argument
`.moon/tasks/rust.yml:63-67` already makes for putting it on `lint`. Leaving those out while
touching this exact line would read to the next engineer as a deliberate decision.

`build.rs` is handled by D2 rather than listed here.

### D2. `build.rs` — per-crate declaration plus a derived assertion

**Chosen:** declare `build.rs` in `rs/crates/bindings/paigasus-node-bindings/moon.yml`
(`build`/`test`/`lint`/`fmt`), and add a parity assertion that **every crate with a `build.rs` on
disk declares it in those tasks**.

**Rejected — the shared template.** One line, and covers a future crate automatically, but it makes
12 of 13 crates declare a file that does not exist: the untracked-input class `repo:input-liveness`
reds on and that SMA-556 must clear before that gate widens past `repo:*`. Harmless today,
debt pointed the wrong way.

**Rejected — per-crate with no assertion.** The next crate to add a `build.rs` silently repeats the
exact bug SMA-537 was filed for (SMA-524's lesson).

### D3. The assertion must be reachable (M5)

`repo:affected-smoke`'s `inputs` (`moon.yml:165-202`) contain **no `build.rs` glob**; the nearest is
`rs/**/Cargo.toml`. So adding a `build.rs` to an *existing* crate without touching its `Cargo.toml`
or `moon.yml` does not schedule `repo:affected-smoke`, and D2's assertion serves a cached PASS on
exactly the PR it exists for. `moon.yml:181-192` already documents this precise trap for the
actionlint and release-parity pins: *"the pin is real but unreachable: the PR that deletes those
lines does not schedule this task."*

**Add `rs/crates/*/*/build.rs` to `repo:affected-smoke`'s `inputs`.** This acquires a
`repo:input-liveness` obligation — the glob must keep matching a tracked file, which it does today
(one crate) and would stop doing if that crate's `build.rs` were deleted. That is the correct
behaviour: the assertion becomes dead at exactly that moment.

### D4. `fmt`'s new inputs need an assertion too (M4)

A spec whose thesis is *unasserted inputs rot* must not add four unasserted inputs. A4
(`check_lint_inputs`, `cargo_moon_parity.py:198-230`) already asserts the three workspace files on
every crate's `lint` for exactly this reason; there is no equivalent for `fmt`.

Generalize A4 into `check_task_inputs(projects, crates, task, required)` and call it twice — for
`lint` (the existing three files) and for `fmt` (`rustfmt.toml`, `rust-toolchain.toml`) — with a
self-test row per task. This is a refactor of an existing check, not a new gate, so it carries no
SMA-541 wiring cost.

### R1. Implementation risk — does Moon append or replace inherited inputs?

D2 assumes a project's `inputs:` on an **inherited** task *appends* rather than replaces. There is
no `mergeInputs` setting in `.moon/`, so defaults apply, and this has not been verified on 2.3.2
here. **Measure first**, comparing resolved `inputFiles` from `moon query projects` before and
after.

**Corrected from rev 1 (m2):** rev 1 said a replace would be "silent" and "present as a cache
anomaly". It would not. A4 reds immediately for that crate's `lint`, and A6 reds for all three
tasks, since `paigasus-node-bindings-rs` has a non-empty `upstreams` group. The measurement is
still the right first step, but the existing gates catch replace-semantics loudly.

**Corrected fallback (M7).** Rev 1 offered "route it through a `fileGroups` entry the shared
template already consumes" — but no candidate group exists, and adding `@group(buildscript)` makes
its absence a hard graph-load error (`project::unknown_file_group`) for every moon command, forcing
all 13 crates to declare it, 12 of them empty. That is *larger* than the shared-template option D2
rejected. The actually-cheap fallback: declare `build.rs` inside the crate's existing
`fileGroups.upstreams`. A6's `observed` filter excludes `own/`-prefixed entries
(`cargo_moon_parity.py:400-401`, verified), so it passes A6 untouched. Verify that before relying
on it.

---

## 4. SMA-560 — assertion A7

A new `check_wrapper_upstream_inputs()` in `ci/affected-graph/cargo_moon_parity.py`, **alongside**
A6. A6's strict equality is correct for Rust and is not loosened.

### 4.1 Measured shape

| project | Rust closure | crate dirs observed |
| -- | -- | -- |
| `paigasus-kernel-py` | kernel, py-bindings | kernel, py-bindings, **kernel-parity** |
| `paigasus-kernel-ts` | kernel, node-bindings, wasm | those three, **+ kernel-parity** |

`kernel-parity` is the SMA-433 parity-vectors input — correct and deliberate.
`paigasus-kernel-py:build` and `paigasus-kernel-ts:typecheck` observe nothing, and should not.

### D5. Containment, file-granular, with two floors

**Containment, not strict equality.** Every required file must be keyed on; extras are allowed.

The adversarial review challenged this (Q3), arguing A6's `startswith("rs/crates/")` filter already
removes the non-crate inputs, leaving only `kernel-parity` — so strict equality plus one waiver
would be strictly stronger at the same cost. **That holds at crate-dir granularity and fails at
file granularity**, which is where D5 operates: both wrappers declare
`rs/crates/bindings/paigasus-node-bindings/package.json` and
`rs/crates/bindings/paigasus-py-bindings/pyproject.toml`, which *are* under `rs/crates/` and would
enter `observed`. Strict equality would flag correct, deliberate inputs as over-approximation —
recording a right thing as wrong, which is worse than a weaker assertion.

**File-granular, not crate-dir (M6).** A7 demands `<upstream>/src/**/*` **and**
`<upstream>/Cargo.toml` per closure member, matching what A6 demands of every Rust crate. Crate-dir
granularity would certify §1.2's item 3 — the missing kernel `Cargo.toml` — as green forever.

**Two floors, not one (B4).** Rev 1 specified only a task-name floor, and the adversarial review
showed it cannot catch A7's actual vacuity mode: if `rust_closure` degrades to empty (a moon
rename, a `dependencies` reshape, a `language` field change), `want` becomes `{}` and containment
`want ⊆ observed` is *vacuously satisfied* while A7 prints PASS. A task-name floor is silent on
that. A6 gets this right with a per-consumer **edge** floor plus a membership half
(`cargo_moon_parity.py:129-132`, `:339-343`).

```python
REQUIRED_WRAPPER_TASKS = {
    "paigasus-kernel-py:test", "paigasus-kernel-ts:build", "paigasus-kernel-ts:test",
}
REQUIRED_WRAPPER_CLOSURE = {
    "paigasus-kernel-py": {"paigasus-kernel-rs", "paigasus-py-bindings-rs"},
    "paigasus-kernel-ts": {"paigasus-kernel-rs", "paigasus-node-bindings-rs", "paigasus-wasm-rs"},
}
```

Floor rows are `FLOOR:`-prefixed so the negative control can distinguish a floor failure from a
per-wrapper one, as A6's already are. Note A6's own warning applies: emptying `deps` also empties
`want`, producing confusable rows — the control must match the prefix.

### D6. The task set is the floor, and that is a coverage decision

A7 cannot ask "every task on the project": `paigasus-kernel-py:build` and
`paigasus-kernel-ts:typecheck` legitimately observe nothing, so a whole-project rule reds on an
unmutated tree.

To be unambiguous — the *closure* is derived per wrapper; the *set of tasks examined* is not. A7
examines exactly `REQUIRED_WRAPPER_TASKS`. For A7 the task floor **is** the task set, so an
omission from it is not a vacuity risk but a **coverage hole**: a new wrapper needs a floor entry
or it is simply unchecked. This is the one hand-maintained part of the design, and the price of the
wrappers having no uniform task shape. `REQUIRED_WRAPPER_CLOSURE` is what carries the anti-vacuity
role.

### 4.2 Wording correction (m3)

Rev 1 said "only the root language filter changes" in `rust_closure`. `rust_closure`
(`cargo_moon_parity.py:279-305`) has **no** root language filter — the root filter lives in
`check_upstream_inputs`'s `examined` set (`:331`). A7 modifies nothing in `rust_closure`; it
defines its own root set.

### 4.3 Fix the wrappers first (M6)

Add `/rs/crates/libs/paigasus-kernel/Cargo.toml` to `paigasus-kernel-py:test`,
`paigasus-kernel-ts:build` and `paigasus-kernel-ts:test`, plus each binding crate's `Cargo.toml`
where absent. This must land **before or with** A7; otherwise A7's first run reds on a pre-existing
defect and the temptation is to weaken A7 rather than fix the wrappers.

### 4.4 A7's own call site (SMA-542)

A7's production invocation in `main()` must itself be pinned, not merely its verdict function
exercised by fixtures. SMA-542's lesson: deleting the production block passes green when only the
verdict function is tested.

---

## 5. Testing

Every assertion must demonstrate it bites.

1. **R1 probe, first.** Resolved `inputFiles` before/after the per-crate `build.rs` declaration.
2. **A7 positive** on the unmutated tree, after §4.3 lands.
3. **A7 negative, per row — using mutations no other assertion can see (B3).** Delete the wasm glob
   from `paigasus-kernel-ts:test`; confirm A7 reds naming that consumer and upstream; restore.
   *Do not* use the kernel glob on `:build` or `:test` — `run.sh:343` already reds on those, so a
   red proves nothing about A7. Then delete the kernel's `Cargo.toml` from
   `paigasus-kernel-py:test`.
4. **A7 closure floor.** Simulate an emptied derivation; confirm `FLOOR:` rows fire and are
   distinguishable from per-wrapper rows.
5. **A7 task floor.** Remove a task from `REQUIRED_WRAPPER_TASKS`; confirm the pairing/registry
   check notices, or record explicitly that it cannot (see L2).
6. **`build.rs` assertion.** Remove the per-crate declaration; confirm red naming the crate.
7. **`build.rs` reachability (D3).** Add a `build.rs` to a crate whose `Cargo.toml` and `moon.yml`
   are untouched; confirm `repo:affected-smoke` is now scheduled by
   `moon query tasks --affected`. Without the new glob this is the case that silently passes.
8. **`fmt` inputs.** Confirm editing `rs/rustfmt.toml` selects the `fmt` tasks via
   `moon query tasks --affected`. **This is an ad-hoc probe, not a `run.sh` case** —
   `_assert_task_case_impl` filters to `build`/`test`/`lint` names (`run.sh:94-97`), so promoting it
   into `run.sh` would yield a vacuous green (m4).
9. **A4/`check_task_inputs` refactor.** Existing `lint` rows still red; new `fmt` rows red on
   removal.
10. **Full graph.** `moon ci` with all 27 targets, `--base origin/main --include-relations`. No new
    `repo:*` task is created (§3 D4, §4 are edits to an existing gate), so the count stays 27 and no
    `T` / CLAUDE.md / `SELF_SCHEDULED_GATES` wiring is needed (M8).

### The re-baseline

Measure, do not assume — see §2.3. Report what the affected sets actually do.

---

## 6. Limitations

- **L1 — A7 is containment, so a wrapper may over-declare permanently.** A glob for a crate no
  longer in the closure stays green. Accepted: the wrapper globs are hand-written with stated
  reasons, and a stale glob costs a spurious re-run, not a missed one. The dangerous direction —
  a *missing* glob — is what A7 catches.
- **L2 — a new wrapper is unchecked until someone adds a `REQUIRED_WRAPPER_TASKS` entry** (D6).
  Unlike A6, whose task set is uniform across crates, A7's must be hand-maintained. Nothing detects
  the omission.
- **L3 — A7 asserts file presence, not glob correctness.** `<upstream>/src/**/*` satisfies it;
  so would a hypothetical `<upstream>/src/lib.rs`. Tightening would require A7 to know which shape
  each consumer needs.
- **L4 — §1.2's item 1 hole is closed for the three floor tasks only.** A wrapper task outside
  `REQUIRED_WRAPPER_TASKS` is not examined at all (L2).

---

## 7. Open questions for the plan

1. **R1's answer** — append or replace. Everything in D2 depends on it.
2. **Does `check_task_inputs`'s `fmt` call need a per-crate exemption?** Every crate has `src/`, but
   `@group(tests)` is empty for crates without a `tests/` dir — confirm an empty group does not red
   the new `fmt` rows.
3. **Ordering of §4.3 against A7** — same commit, or a preparatory one? A separate commit makes the
   "A7 reds on a pre-existing defect" state briefly real in history.
