# SMA-560 — Cross-stack input affectedness: assert the ADR-0005 guarantee, and close two Rust input gaps

**Status:** Draft, rev 3 (brainstorming 2026-08-28; **two adversarial passes incorporated**; scope reduced in rev 2 — §2.1; `build.rs` route reversed in rev 3 — §3 D2).
**Date:** 2026-08-28
**Linear:** [SMA-560](https://linear.app/smaschek/issue/SMA-560) (lead) + [SMA-537](https://linear.app/smaschek/issue/SMA-537).
**Removed from scope:** [SMA-535](https://linear.app/smaschek/issue/SMA-535), [SMA-536](https://linear.app/smaschek/issue/SMA-536) (§2.1); codegen case split to [SMA-592](https://linear.app/smaschek/issue/SMA-592).
**Branch:** `feature/sma-560-assert-cross-stack-input-affectedness`
**Targets:** `main` (currently `2f37378`).
**References:** ADR-0005; SMA-528 (A6; inputs are the only thing that confers affectedness); SMA-546 (A5); SMA-534 (A4); SMA-524 ("a MISSING case is how the bug survived"); SMA-429 (default-deny); SMA-542 (a check's own call site goes unguarded); SMA-553 / SMA-556 (input liveness, dead inputs); SMA-541 (`T` / CLAUDE.md wiring); SMA-433 (parity corpus); SMA-434, SMA-590, SMA-592 (adjacent, out of scope).

---

## 1. Problem

Moon 2.3.2 confers affectedness **only** through a task's own `inputs`. `dependsOn` and a
task-level `^:build` schedule an upstream's build but never *select* a downstream (SMA-528,
measured at the full 24-target shape). Every "a change to X re-runs Y" guarantee is a claim about
Y's `inputs` and nothing else.

### 1.1 Two files are in no *crate* task's inputs (SMA-537)

`.moon/tasks/rust.yml` declares crate task inputs as `@group(sources)` (`src/**/*`),
`@group(tests)`, `Cargo.toml`, and — on `lint` only — three workspace-level files. So:

- A crate-root `build.rs` matches none of them. Editing
  `rs/crates/bindings/paigasus-node-bindings/build.rs` re-keys no crate task, though `cargo build`
  runs it and `cargo clippy --all-targets` compiles it.
- `fmt` runs `cargo fmt --check` with `inputs: ['@group(sources)']` alone (`:83-85`), so
  `rs/rustfmt.toml` invalidates no `fmt` task, and neither does `@group(tests)` nor
  `rs/rust-toolchain.toml` (which selects the rustfmt binary).

*Both files match some broad `repo:*` input — `repo:machete`'s `rs/**/*.rs`,
`repo:publish-metadata`'s `rs/crates/**/*`, the two `['**/*']` gates. The claim is about crate
`build`/`test`/`lint`/`fmt`, which is where invalidation matters.*

### 1.2 The wrappers' Rust globs are only partly asserted (SMA-560)

SMA-528 gave every Rust crate a `fileGroups.upstreams` and added **A6**, strict-equality against
Moon's own closure. A6 iterates `language == "rust"` only (`cargo_moon_parity.py:331`). The py/ts
wrappers reach the kernel through hand-written per-task globs, and those globs *are* the ADR-0005
cross-binding guarantee.

**Correction carried from rev 1.** Rev 1 claimed deleting `/rs/crates/libs/paigasus-kernel/src/**/*`
from `paigasus-kernel-ts:build` "leaves every gate green." **False.**
`ci/affected-graph/run.sh:343` — `run_task_case_ci "kernel->consumer-tasks"`, strict equality —
lists `paigasus-kernel-ts:build`, `:test` and `paigasus-kernel-py:test` in the expected set for a
touch of `rs/crates/libs/paigasus-kernel/src/lib.rs`. The kernel→wrapper edge **is** already
covered, by one hand-written case.

#### What is genuinely uncovered — all three re-verified in the second pass

1. **Every non-kernel upstream.** No `run.sh` case touches
   `rs/crates/bindings/paigasus-wasm/src/**` *and* asserts `paigasus-kernel-ts:test`. Deleting the
   wasm glob from that task is **green today** (confirmed: `binding-oneway-wasm` survives via
   `:build`'s own glob; A5 still matches the markers; A6 skips the project on `language != "rust"`).
2. **A new wrapper, or a new upstream on an existing one.** `run.sh`'s coverage is a hand-written
   case list, and a *missing* case is how SMA-524's bug survived review.
3. **Two live under-declarations, today.**
   - Neither wrapper declares the kernel's `Cargo.toml` (0 occurrences in both), while A6 demands
     `src/**/*` **and** `Cargo.toml` per upstream for every Rust crate (`:357-360`), and
     `repo:parity-corpus-drift` lists it for exactly this hazard — "a kernel Cargo.toml change (a
     future `[features]` toggle altering `sum`) must re-key the gate" (`moon.yml:214-217`).
   - Neither ts wrapper task declares `rs/crates/bindings/paigasus-node-bindings/build.rs`, though
     both run `napi build` against that crate and its `build.rs` is `napi_build::setup()` — it
     emits the addon's link args. **This is where SMA-537 and SMA-560 intersect.**

Item 3 is a defect to **fix**, not merely assert (§4.3).

### Verified against `main` @ `2f37378`

| Claim | Verdict |
| -- | -- |
| `build.rs` in no *crate* task's inputs | Holds |
| `rs/rustfmt.toml` in no `fmt` inputs | Holds — `fmt: inputs: ['@group(sources)']` |
| A6 iterates Rust only | Holds — `examined = {… == "rust"}` (`:331`) |
| Exactly one `build.rs` in the workspace | Holds — `paigasus-node-bindings` |
| Neither wrapper declares the kernel's `Cargo.toml` | Holds — 0 occurrences in both |
| Neither ts wrapper task declares the node-bindings `build.rs` | Holds — 0 occurrences |
| Kernel→wrapper edge already covered | Holds — `run.sh:343`, strict equality |
| Deleting the wasm glob from `kernel-ts:test` is green | Holds — verified in the second pass |

---

## 2. Scope

**SMA-560 and SMA-537 only.**

### 2.1 Why SMA-535 and SMA-536 left

Rev 1 designed a "key the typecheck task on the interface artifact" fix for each. Both were
**vacuous by the same mechanism rev 1 used to reject SMA-535's originally filed fix** — the
reasoning applied one level up but not one level down.

- **py:** `py/pyproject.toml:12` sets `include = ["packages/*/src/**", "packages/*/tests/**"]`, so
  basedpyright never opens the source stub; it reads the maturin-**installed** copy.
  `.moon/tasks/python.yml:38` runs plain `uv run basedpyright` with no `--reinstall-package`, and
  plain `uv run` is measured in-repo to serve a cached wheel (`run.sh:325-329`).
- **ts:** `tsc` resolves `@paigasus/node-bindings` through `node_modules` with no `paths` mapping,
  and pnpm copies a `file:` dep into its store and never re-copies
  (`ts/packages/paigasus-kernel/moon.yml:99-102`, measured).

Both need *make the checker read a fresh artifact*, a different and larger fix. Full analysis is
recorded on each issue, including that a correct PyO3 stub-drift gate must compare **three** sets
(`#[pyfunction]` idents, `wrap_pyfunction!` registrations, stub `def` names).

### 2.2 Also out of scope

SMA-592 (codegen-config selection), SMA-434 (ts glue drift), SMA-590 (`ty`).

### 2.3 What justifies one PR

Rev 1 claimed each half re-baselines `run.sh`'s expected sets. **That is false**, and the second
pass determined it definitively rather than leaving it to measurement: no `run.sh` case touches
`rs/rustfmt.toml`, `rs/rust-toolchain.toml`, the kernel's `Cargo.toml`, or any `build.rs`, and
`fmt` is filtered out of `_assert_task_case_impl` entirely (`run.sh:94-97`). **No expected set
moves.**

The surviving grounds: one principle, one review cycle, and one file — `cargo_moon_parity.py` gains
one new assertion and one generalized one.

---

## 3. SMA-537 — the two Rust input gaps

### D1. `fmt` inputs, widened past what SMA-537 asked for

```yaml
fmt:
  command: 'cargo fmt --check'
  inputs: ['@group(sources)', '@group(tests)', '/rs/rustfmt.toml', '/rs/rust-toolchain.toml']
```

SMA-537 asks only for `rustfmt.toml`. That stops three-quarters short: `cargo fmt --check` formats
**every** target — `src/**`, `tests/**`, `benches/**`, `build.rs` — so a misformatted integration
test can merge green today and red `main` on an unrelated later `src` edit. `rust-toolchain.toml`
selects the rustfmt binary, the same argument `.moon/tasks/rust.yml:63-67` makes for `lint`.

### D2. `build.rs` — the shared `sources` fileGroup (**reversed in rev 3**)

```yaml
fileGroups:
  sources:
    - 'src/**/*'
    - 'build.rs'
```

One line. It reaches `build`, `test`, `lint`, `build-release` **and** `fmt` (all consume
`@group(sources)`), and covers every future crate automatically —
`.moon/tasks/rust.yml:50-56` makes exactly this argument for declaring `deps: ['^:build']` centrally:
*"Declared here rather than per-crate so a new crate has no per-crate `lint` declaration to forget."*

**Verified not to disturb A6:** a crate's own `build.rs` resolves under its own source dir, and A6's
`observed` excludes `own/`-prefixed entries (`cargo_moon_parity.py:400-401`).

**Why rev 2's per-crate route was reversed.** Rev 2 chose a per-crate declaration plus a derived
assertion, to avoid 12 of 13 crates declaring a file that does not exist. The second adversarial
pass costed that route properly and it is far more expensive than rev 2 assumed:

- a new derived assertion, its own anti-vacuity floor, and handling for `[package] build =
  "custom.rs"` (which would otherwise empty the derived set silently);
- a 20th `repo:affected-smoke` input for reachability — and that list is pinned by check 8e's
  `T_AFFECTED_SMOKE_REQUIRED_INPUTS` (`ci/actionlint/run.sh:2097-2117`, deliberately *the whole
  list* — "a floor, not a judgement call") with an arity floor pinned verbatim at **two** sites in
  `ci_targets.py` (`:572`, `:1515`). A four-site edit.

Against that, the dead-input cost is: nothing reds today (`repo:input-liveness` is `repo:`-scoped —
confirmed in `task_inputs.py`), and SMA-556's list grows only *if* that gate is ever widened. Rev 2
rejected the one-liner on a future cost that has not landed and may never, while accepting a
concrete cost now. Reversed.

Consequence: rev 2's D3 (the reachability input) and R1 (append-vs-replace) **no longer exist** —
there is no per-crate declaration, so nothing depends on Moon's input merge semantics.

### D3. `fmt`'s new inputs need an assertion

A spec whose thesis is *unasserted inputs rot* must not add unasserted inputs. A4
(`check_lint_inputs`, `:198-230`) asserts the three workspace files on every crate's `lint`; there
is no equivalent for `fmt`.

Generalize A4 into `check_task_inputs(projects, crates, task, required)` and call it for `lint` and
`fmt`, with a self-test row per task.

**It must union both input buckets.** A4 reads `task_inputs` only — moon's `inputFiles`. Literals
like `rustfmt.toml` land there, but `@group(sources)` and `@group(tests)` resolve to **globs** and
land in `inputGlobs` (`moon_projects()`, `:441-448`). A one-bucket check cannot see `@group(tests)`
at all, so removing it later would restore today's bug with nothing red — A6 exists partly because
of this exact split (`:744-762`). Union both, as A6 does (`:381`), and require
`<crate>/src/**/*` and `<crate>/tests/**/*` alongside the two literals.

---

## 4. SMA-560 — assertion A7

A new `check_wrapper_upstream_inputs()` in `cargo_moon_parity.py`, **alongside** A6. A6's strict
equality is correct for Rust and is not loosened.

### 4.1 Measured shape

| project | Rust closure | crate dirs observed |
| -- | -- | -- |
| `paigasus-kernel-py` | kernel, py-bindings | kernel, py-bindings, **kernel-parity** |
| `paigasus-kernel-ts` | kernel, node-bindings, wasm | those three, **+ kernel-parity** |

`kernel-parity` is the SMA-433 parity-vectors input — correct and deliberate.
`paigasus-kernel-py:build` and `paigasus-kernel-ts:typecheck` observe nothing, and should not.

### D4. The task set is **derived**, not hand-maintained (**changed in rev 3**)

Rev 2 specified a hand-written `REQUIRED_WRAPPER_TASKS`, and admitted as L2 that a new wrapper
would be silently unchecked. The second pass showed that limitation is unnecessary: **A5 already
derives exactly this set.** `check_ffi_inputs` (`:233-276`) matches each task's resolved
`command + args + script` blob against `FFI_MARKERS` (`:94`) — `napi build`, `wasm-pack`,
`maturin`, `--reinstall-package` — and `REQUIRED_FFI_TASKS` (`:103-107`) is byte-identical to what
rev 2 was about to hand-write.

So: split `derive_ffi_tasks(projects) -> set[str]` out of `check_ffi_inputs`, and have A7 examine

```python
{t for t in derive_ffi_tasks(projects) if projects[t.split(":")[0]]["language"] != "rust"}
```

keeping `REQUIRED_FFI_TASKS` as the **shared** floor. A new wrapper's `napi build` task is then
matched on day one *even if it declares zero inputs* — which is precisely the bug rev 2's L2 said
nothing detects. **L2 and L4 are deleted.**

### D5. Containment, file-granular, with a closure floor

**Containment, not strict equality.** Every required file must be keyed on; extras are allowed.

Both adversarial passes challenged this, the second one arguing the headline over-declaration is
`kernel-parity/vectors/**` — a whole crate dir outside the closure — which would strengthen the
case for strict-equality-plus-one-waiver. The answer is unchanged and now has a second leg: at
**file** granularity both wrappers also declare
`rs/crates/bindings/paigasus-node-bindings/package.json` and
`rs/crates/bindings/paigasus-py-bindings/pyproject.toml`, which are under `rs/crates/` and would
enter `observed`. Strict equality would flag correct, deliberate inputs as over-approximation —
recording a right thing as wrong, which teaches the next reader the wrong lesson.

**File-granular.** A7 demands, per closure member: `<upstream>/src/**/*`, `<upstream>/Cargo.toml`,
and `<upstream>/build.rs` **when one exists on disk**. Crate-dir granularity would certify both of
§1.2's item-3 under-declarations as green forever.

**Read shape, stated explicitly.** A7 unions `inputFiles` and `inputGlobs` **per `(project, task)`**,
never across a wrapper's tasks, and treats an absent bucket as a violation (`:362-381`). All three
wrong readings fail silently rather than loudly: an `inputGlobs`-only A7 makes §4.3's `Cargo.toml`
fix invisible; an `inputFiles`-only A7 makes §5's headline mutation a vacuous pass; a
union-across-tasks A7 also passes it, because `:build` keeps the glob. A6's `own/`-prefix exclusion
is **not** carried over — a wrapper's `source_dir` is `ts/packages/…`, so the `rs/crates/` filter
suffices and an `own` variable would be dead code.

**Closure floor.** A containment check with a derived `want` is *vacuously satisfied* when `want`
empties — a moon rename, a `dependencies` reshape, a `language` field change. A task-name floor is
silent on that; A6 encodes the countermeasure as a per-consumer **edge** floor (`:129-132`, `:339-343`).

```python
REQUIRED_WRAPPER_CLOSURE = {
    "paigasus-kernel-py": {"paigasus-kernel-rs", "paigasus-py-bindings-rs"},
    "paigasus-kernel-ts": {"paigasus-kernel-rs", "paigasus-node-bindings-rs", "paigasus-wasm-rs"},
}
```

Rows are `FLOOR:`-prefixed so a control can distinguish a floor failure from a per-wrapper one.
A6's warning applies: emptying `deps` also empties `want`, producing confusable rows.

**The floor constant is itself floored.** `self_test()` gains a non-emptiness row for
`REQUIRED_WRAPPER_CLOSURE`, mirroring `:555-560`'s rows for `REQUIRED_FFI_TASKS` and
`UPSTREAM_INPUT_TASKS` — which exist because A6/A5's self-tests pass explicit `floor=` arguments,
so the real constant is otherwise never exercised. A floor entry naming an absent project or task
is a `FLOOR:` violation, **never a skip** (A6's `_ABSENT` handling, `:362-381`).

### 4.2 Wording correction

`rust_closure` (`:279-305`) has **no** root language filter — the root filter lives in
`check_upstream_inputs`'s `examined` set (`:331`). A7 modifies nothing in `rust_closure`; it derives
its own root set (D4).

### 4.3 Fix the wrappers first

Add to `paigasus-kernel-py:test`, `paigasus-kernel-ts:build`, `paigasus-kernel-ts:test`:

- `/rs/crates/libs/paigasus-kernel/Cargo.toml` — all three;
- `/rs/crates/bindings/paigasus-node-bindings/build.rs` — the two ts tasks.

The binding crates' own `Cargo.toml`s are **already** declared on all three (verified), so no
change there — rev 2's "plus each binding crate's `Cargo.toml` where absent" was a no-op and is
removed.

This lands **in the same commit** as A7. §2.3 established no `run.sh` expected set moves, so there
is no reason to split it, and a split would briefly make "A7 reds on a pre-existing defect" real in
history.

### 4.4 Guarding A7's own invocation (SMA-542)

A call-site pin alone is insufficient. `main()` has a second, equally unguarded site: the aggregate
guard `if not (a1 or a2 or a3 or a4 or a5 or a6)` (`:897`) and the report tuple (`:907-941`).
Calling `check_wrapper_upstream_inputs()` but forgetting to fold `a7` into **both** is a green
no-op.

**Chosen mechanism:** restructure `main()` to iterate a list of `(rows, title)` pairs, so a
forgotten fold is structurally impossible rather than merely detected. Fallback if that proves
invasive: a self-test row over `inspect.getsource(main)` requiring both
`check_wrapper_upstream_inputs(` and `a7` inside the guard expression — the in-repo precedent for
source-pinning in Python is `ci_targets.py:1732,1881`.

---

## 5. Testing

1. **A7 positive** on the unmutated tree, after §4.3.
2. **A7 negative, per row — mutations no other assertion can see.** Delete the wasm glob from
   `paigasus-kernel-ts:test`; confirm A7 reds naming that consumer and upstream; restore. *Do not*
   use the kernel glob on `:build`/`:test` — `run.sh:343` already reds on those, so a red proves
   nothing about A7. Then delete the kernel's `Cargo.toml` from `paigasus-kernel-py:test`, and the
   node-bindings `build.rs` from `paigasus-kernel-ts:build`.
3. **A7 closure floor.** Simulate an emptied derivation; confirm `FLOOR:` rows fire and are
   distinguishable from per-wrapper rows.
4. **Floor-constant non-emptiness.** Empty `REQUIRED_WRAPPER_CLOSURE`; confirm the self-test row
   fires.
5. **Floor entry naming an absent project/task** → a `FLOOR:` row, not a skip.
6. **A7 aggregate-guard guard (§4.4).** Remove `a7` from the guard; confirm the structure makes it
   impossible, or the self-test row fires.
7. **`build.rs` in `sources`.** Confirm editing `paigasus-node-bindings/build.rs` selects that
   crate's `build`/`test`/`lint`/`fmt` via `moon query tasks --affected`.
8. **`fmt` inputs.** Confirm editing `rs/rustfmt.toml` selects the `fmt` tasks. **Ad-hoc probe, not
   a `run.sh` case** — `_assert_task_case_impl` filters to `build`/`test`/`lint` names
   (`run.sh:94-97`), so promoting it into `run.sh` yields a vacuous green.
9. **`check_task_inputs` refactor.** Existing `lint` rows still red; new `fmt` rows red on removal
   of each of the four, **including the two glob-bucket ones**.
10. **Full graph.** `moon ci` with all 27 targets (verified: `ci.yml:214` has 27, and CLAUDE.md's
    marker block matches), `--base origin/main --include-relations`. **No new `repo:*` task**, so no
    `T`, CLAUDE.md, or `SELF_SCHEDULED_GATES` change — and, with D2 reversed, no
    `repo:affected-smoke` input change and therefore no check-8e / `ci_targets.py` pin churn either.

### The re-baseline

**No expected set moves** — determined, not assumed (§2.3). The plan should confirm rather than
discover this.

---

## 6. Limitations

- **L1 — A7 is containment, so a wrapper may over-declare permanently.** A glob for a crate no
  longer in the closure stays green. Accepted: the wrapper globs are hand-written with stated
  reasons; a stale glob costs a spurious re-run, not a missed one. The dangerous direction — a
  *missing* glob — is what A7 catches.
- **L2 — A7 asserts file presence, not glob correctness.** `<upstream>/src/**/*` satisfies it; so
  would `<upstream>/src/lib.rs`. Tightening needs A7 to know which shape each consumer requires.
- **L3 — twelve crates declare a `build.rs` that does not exist** (D2). Reds nothing today; adds to
  SMA-556's list if `repo:input-liveness` is ever widened past `repo:*`.

*Rev 2's L2 and L4 are deleted — D4's derivation closes them.*

---

## 7. Open questions for the plan

1. **Is restructuring `main()` into a `(rows, title)` list invasive enough to prefer the
   `inspect.getsource` fallback?** (§4.4) Decide against the actual diff.

*Rev 2's other open questions are closed: `@group(tests)` on a crate with no `tests/` dir is a glob
matching nothing, not a violation — already true for the inherited `test` and `lint` tasks on
several crates today; the re-baseline is determined empty (§2.3); §4.3 lands in one commit; R1 no
longer exists (D2).*
