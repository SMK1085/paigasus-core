# SMA-592 / SMA-594 — Codegen-config freshness, and two more "read but keyed on by nothing" files

**Status:** design
**Issues:** SMA-592 (codegen-config selection), SMA-594 (`rs/.cargo/config.toml`), plus one gap
found during this design and folded into SMA-594.
**Branch:** `feature/sma-592-codegen-and-input-affectedness-residue`
**Verified against `main` @ `82fe78e`.**

One principle, three files: *task `inputs` are the only thing that confers affectedness in Moon
2.3.2, and the only thing that makes a cache key honest.* SMA-528, SMA-546 and SMA-560 closed this
class for crate sources, workspace files and the py/ts wrapper closures. These are the residue.

---

## 1. Measured baseline

All five rows come from `moon query tasks --affected` on the worktree at `82fe78e`. They are the
evidence for every claim in this spec.

| Edited file | Tasks selected today |
| -- | -- |
| `rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi` | `repo:actionlint`, `repo:input-liveness`, `repo:publish-metadata` |
| `rs/.cargo/config.toml` | `repo:actionlint`, `repo:input-liveness`, `repo:publish-metadata` |
| `.prototools` | 8 crate/wrapper tasks + 8 `repo:*` gates — but **not** `contracts:generate` |
| `py/uv.lock` | `paigasus-kernel-py:test`, `paigasus-py-bindings-rs:build`, the four `py:*` tasks, 5 `repo:*` gates — but **not** `contracts:generate` |
| `contracts/buf.gen.yaml` | `contracts:generate`, `repo:actionlint`, `repo:input-liveness` |

The first two rows select only broad or packaging gates. `repo:actionlint` and
`repo:input-liveness` both declare `inputs: ['**/*']`, so they select on *every* file and prove
nothing about a specific one. `repo:publish-metadata` declares `rs/crates/**/*` and
`rs/.cargo/config.toml`, so it reaches both files — but it checks packaging metadata. It compiles
nothing that consumes them.

**This corrects SMA-594 as filed.** That issue says `rs/.cargo/config.toml` is "in no task's
inputs". That is false: `moon.yml:529` declares it on `repo:publish-metadata`, and
`ci/affected-graph/ci_targets.py:238` pins it there. The accurate claim is narrower and is what
this spec fixes: **no task that compiles Rust with those flags keys on them.**

---

## 2. SMA-592 — the codegen-drift gate can pass vacuously

### 2.1 The gate is not where SMA-592 says it is

SMA-592 states "`repo:*` already has a codegen-drift gate". There is no such Moon task. The gate is
an inline step in `.github/workflows/ci.yml:249-262`:

```
- name: Codegen drift gate (committed generated code matches protos)
  run: |
    moon run contracts:generate
    git add --intent-to-add -- <three generated dirs>
    if ! git diff --exit-code -- <three generated dirs>; then ...
```

The difference matters in the gate's favour. The step carries no `if:`, so it runs on **every** CI
run and cannot be deselected. A `repo:*` task in `ci.yml`'s `T=(…)` array would run only when
affected, so a wrong `inputs` list would silently switch it off. That is why §6 keeps it where it
is.

### 2.2 Where the generator versions actually live

`contracts:generate`'s inputs are `proto/**/*`, `buf.yaml`, `buf.gen.yaml`, `buf.lock`
(`contracts/moon.yml:9-13`). The things that determine its output live in three places, not one:

| Generator | Pinned in | An input of `contracts:generate`? |
| -- | -- | -- |
| `neoeinstein-prost:v0.5.0`, `neoeinstein-tonic:v0.5.0`, `bufbuild/es:v2.13.0` | `contracts/buf.gen.yaml` | **yes** |
| `buf` itself (1.70.0) | `.prototools` | **no** |
| `protoc-gen-python_betterproto2` (local plugin, run via `uv run --project ../py`) | `py/uv.lock` | **no** |

SMA-592 says `.prototools` "pins the plugins". That is half right. `.prototools` pins **buf**; the
Python plugin is pinned by `py/uv.lock`, which the issue never names.

### 2.3 The failure

The drift step delegates its freshness to `moon run contracts:generate`. On a `.prototools` or
`py/uv.lock` bump that task's hash is unchanged, so Moon reports a cache hit and does not run `buf
generate`. `contracts:generate` declares no `outputs:`, so a cache hit restores nothing and the
working tree keeps the committed files. The diff then compares the committed output against
itself and passes.

**The `.moon/cache` is warm in CI.** `ci.yml:115-121` restores it with
`restore-keys: moon-${{ runner.os }}-`. So this is a real CI hole, not a local-only one.

For a `buf.gen.yaml` edit the gate already works today: that file is an input, the hash changes,
`buf generate` re-runs and the diff is real. So SMA-592's motivating example is the one case that
is already safe. The hole is the two files it does not name precisely.

### 2.4 The fix

Add two inputs to `contracts:generate` in `contracts/moon.yml`:

```yaml
- '/.prototools'    # pins buf itself; its output is version-dependent
- '/py/uv.lock'     # pins protoc-gen-python_betterproto2, the local plugin
```

This closes both halves of SMA-592 with no new gate:

* **Freshness** closes directly. The drift step's `moon run` re-runs `buf generate` on a generator
  bump, so the diff becomes real.
* **Selection** closes as a consequence. A real diff reds the gate. The developer commits the
  regenerated output. The committed diff then selects the consumers through the same mechanism
  that already works for an ordinary `.proto` edit. No parallel selection path is introduced.

### 2.5 The fix needs its own guard

Nothing asserts `contracts:generate`'s inputs. `repo:input-liveness` reds when a declared glob
matches zero tracked files; it does not notice a **deleted** input, which is this defect's exact
shape. Landing an unasserted fix would let the next person silently reopen the hole.

Add a literal pin in `ci/affected-graph/ci_targets.py`, run by `repo:affected-smoke`:

```python
CONTRACTS_GENERATE_INPUTS = (
    "buf.gen.yaml", "buf.lock", "buf.yaml", "proto/**/*",
    ".prototools", "py/uv.lock",
)
```

Exact-equality, matching how `SELF_TASK_EXPECTED_GLOBS` already pins `repo:*` gates. The
trade-off is stated and accepted: a legitimate edit to those inputs reds the gate until the
constant is updated. SMA-554 makes the same argument for `ci.yml`'s invocation lines, from
measured evidence that pattern-matching has a long tail and exact literals do not.

**It needs a new comparison function, not `check_gate_inputs`.** That function hardcodes
`repo = projects.get("repo")` (`ci_targets.py:1041`), so it can only reach `repo:*` tasks;
`contracts:generate` lives in the `contracts` project. Generalising it was rejected — it carries a
default-table assertion and a self-test that both name `SELF_TASK_EXPECTED_GLOBS` explicitly
(`ci_targets.py:1831-1841`, `:1895`), so widening its signature risks the guard-the-guard
machinery for no gain. Add `check_contracts_generate_inputs(projects)` instead, reusing the same
two-bucket `inputGlobs`-then-`inputFiles` comparison and the same injected-glob filter, with its
own self-test. Keeping it outside the three registries also leaves `check_registry_pairing`'s
invariant untouched.

**Reachability is already satisfied.** `repo:affected-smoke` declares `'*/moon.yml'` among its
inputs (`moon.yml:157`), which matches `contracts/moon.yml`. So the PR that edits
`contracts:generate`'s inputs does select the task that pins them. This is the trap CLAUDE.md
records twice — a pin that is "real but unreachable behind a cached PASS" — and it does not apply
here. Verify it still holds rather than assuming, since the assertion depends on it.

---

## 3. SMA-594 — `rs/.cargo/config.toml`

### 3.1 What the file does, and who needs it

It sets `-C link-arg=-undefined -C link-arg=dynamic_lookup` for the two `*-apple-darwin` targets,
so the PyO3 and napi cdylibs link on macOS. Cargo finds it by walking up from the **working
directory**, so any cargo invocation with cwd inside `rs/` reads it.

Two facts, both already measured in-repo, decide the scope:

* **napi needs it.** `ts/packages/paigasus-kernel/moon.yml:24` documents the
  `--cwd ../../../rs/...` that exists precisely to bring it into scope.
* **maturin does not.** SMA-578 measured (2026-08-28) that maturin injects the same link arguments
  itself; an sdist builds on macOS with no `.cargo/config.toml` anywhere on cargo's upward walk.
  The control was a plain `cargo build` in that directory, which failed with undefined `_Py*`
  symbols. The file's own comment says it exists so a build "WITHOUT maturin" links.

### 3.2 D1. One rule, not a needs-it/does-not carve-out

**Decision: every task that runs cargo from `rs/` keys on the file.**

The alternative was to declare it only on the two ts tasks, where the flags are load-bearing. That
was rejected. `REQUIRED_FFI_TASKS` names all three FFI tasks, so a narrow fix needs a **second**
constant plus a recorded exemption for `paigasus-kernel-py:test`. The exemption would rest on
"maturin currently supplies these arguments itself" — a property of maturin 1.9.6, not of this
repo, and one that no gate here observes.

The broader rule is also the *correct* criterion. The right question for a cache input is "does
this file influence the output?", not "is it strictly required?". `rustflags` under
`[target.*-apple-darwin]` affect **every** cargo build on darwin from within `rs/`, so a future
entry there would change maturin's output too.

So `WORKSPACE_LINT_INPUTS` gains one entry and flows into `FFI_TASK_INPUTS` by the existing splat:

```python
WORKSPACE_LINT_INPUTS = (
    "rs/Cargo.lock", "rs/Cargo.toml", "rs/rust-toolchain.toml",
    "rs/.cargo/config.toml",   # SMA-594
)
FFI_TASK_INPUTS = (*WORKSPACE_LINT_INPUTS, ".prototools")
```

A4 then asserts it on all thirteen crates' `lint`, and A5 on all three FFI tasks. No new
assertion, no new constant, no exemption.

### 3.3 D2. Declared on build / build-release / test / lint, **not** `fmt`

`cargo fmt --check` neither compiles nor links, and `rustflags` cannot change formatting. The
exclusion matches how `fmt` already omits `@group(upstreams)` and the workspace lock — it is
crate-local and config-driven by `rustfmt.toml` alone.

### 3.4 Scope note — this is a cache-correctness fix, not a CI-correctness fix

The flags are `[target.*-apple-darwin]`-scoped and CI is Linux, so this cannot change a CI verdict
today. It stops a developer's Mac replaying a cached cdylib against changed linker configuration.
SMA-546 excluded the file **deliberately** for that reason. This spec reverses that call on the
grounds in D1, and records the reversal rather than leaving the two decisions to contradict each
other silently.

---

## 4. SMA-594′ — the `.pyi` stub is keyed on by nothing that validates it

Found while verifying SMA-535's premise (§5). Same defect class as §3, one crate over, so it lands
here rather than as its own issue.

`rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi` is the hand-written interface
contract between the Rust cdylib and every Python consumer. It is tracked, and it is listed in the
crate's Cargo `include` (`Cargo.toml:20`), so it ships in the wheel and the sdist.

It sits at the **crate root**, so `paigasus-kernel-py:test`'s
`/rs/crates/bindings/paigasus-py-bindings/src/**/*` does not match it. Row 1 of §1 confirms the
consequence: editing the stub selects only the two `**/*` gates and the packaging gate. **The FFI
smoke test that exercises the very symbols the stub declares does not run.**

**Fix, two halves.** Add the file to `paigasus-kernel-py:test`'s `inputs` in
`py/packages/paigasus-kernel/moon.yml`, beside the binding's `Cargo.toml` and `pyproject.toml`
that are already there for the same cache-input-completeness reason.

Then make A7 demand it, so the input cannot rot. A7's `want` set is **derived**, not listed
(`cargo_moon_parity.py:540-548`): for each upstream it adds `{src}/src/**/*`, `{src}/Cargo.toml`,
and `{src}/build.rs` *only when that file exists on disk*. Extend it with a matching conditional
clause for `{src}/*.pyi`, demanding each stub found. Mirroring the `build.rs` precedent is
deliberate:

* it keeps the requirement off the twelve crates that have no stub, so no dead demand is created;
* it needs no new hand-maintained list, so a second stub is covered on the day it appears; and
* it reuses the `root`-is-required discipline SMA-560 I3 established, since the clause is
  disk-conditional exactly as `build.rs` is.

It creates no false demand on the ts wrapper: `paigasus-kernel-ts`'s closure is the kernel plus
the node and wasm bindings, none of which carries a `.pyi`.

---

## 5. Scope — why SMA-535 left

SMA-535 was in this combination when the work started. It came out during design, and the reason
is recorded here so it is not re-derived.

Its filed fix — give `py:typecheck` Rust inputs — **cannot work**, for a reason stronger than the
one SMA-560's spec §2.1 gives. That spec argues from staleness: `uv run basedpyright` carries no
`--reinstall-package`, and plain `uv run` is measured in-repo to serve a cached wheel. True, and
sufficient on its own.

The stronger reason is that basedpyright never reads the Rust at all. `paigasus_py_bindings.pyi`
is **hand-written**, not generated from the crate. It is basedpyright's only view of the FFI
surface. A `#[pyfunction]` signature change with no stub update leaves the stub self-consistent,
so the checker passes **whatever inputs the task declares**. Adding Rust sources buys scheduling
with no coverage — the vacuous shape this repo keeps filing issues about.

A correct fix is a stub-drift gate comparing three sets: `#[pyfunction]` idents,
`wrap_pyfunction!` registrations, and stub `def` names. That is its own design. SMA-535 pairs
naturally with SMA-536, the identical `ts:typecheck` defect, currently in the Frontend milestone.

Dropping SMA-535 also drops the A8 assertion this design first proposed. There would be nothing
sound to guard.

**Also out of scope:** SMA-552 (`--locked` across the graph) and SMA-527 (path-form Cargo deps).
Both move `ci/affected-graph/run.sh`'s expected sets, which would make it impossible to attribute
any movement this branch causes.

---

## 6. D3. The drift gate stays in `ci.yml`

**Decision: do not promote the codegen-drift step to a `repo:*` Moon task.**

Promotion would buy `repo:input-liveness` coverage, a `SELF_TASK_EXPECTED_GLOBS` pin, and the
removal of ~14 lines of unlinted inline bash (SMA-539's territory). It would cost the one property
that makes the gate trustworthy: it runs **unconditionally**. In `T` it would run only when
affected, so a wrong `inputs` list would silently switch off the gate — the exact failure class
this branch exists to close.

§2.5's pin recovers most of the assertion benefit without the loss. Promotion stays available as a
separate decision; it is not blocked by anything here.

---

## 7. Testing

Every change is an `inputs` addition, so the test for each is the same shape: **the affected set
must grow, and the guard must be proven to red.**

1. **Re-run §1's five measurements.** Expected movement, and nothing else:
   * `rs/.cargo/config.toml` gains all thirteen crates' `lint` (A4) and the three FFI tasks (A5).
   * the `.pyi` gains `paigasus-kernel-py:test`.
   * `.prototools` and `py/uv.lock` each gain `contracts:generate`.
2. **Prove A4/A5 red.** Remove `rs/.cargo/config.toml` from `WORKSPACE_LINT_INPUTS`, confirm the
   named `a4-lint` and `a5` failures, restore.
3. **Prove A7 red, both ways.** Remove the `.pyi` from `paigasus-kernel-py:test`, confirm the
   named A7 failure, restore. Then confirm the clause is not vacuous: A7 must still pass for
   `paigasus-kernel-ts`, whose closure carries no stub, so the new demand cannot be satisfied by
   accident.
4. **Prove the new pin red.** Delete `/py/uv.lock` from `contracts:generate`, confirm
   `repo:affected-smoke` names the expected literal and the file to update, restore.
5. **Prove the drift hole was real, and is closed.** On an unmutated tree with a warm
   `.moon/cache`: bump a generator version, run the `ci.yml:249-262` step's commands by hand, and
   confirm it passes **before** the input fix and reds **after**. This is the one measurement this
   spec derives by reading rather than by running, so the plan must execute it.
6. **`ci/affected-graph/run.sh` expected sets.** Expected to be unchanged: no case edits any of
   the four files, and adding an input changes what *that file* selects, not what a source edit
   selects. Any movement is reported, never silently re-baselined.
7. **Full graph**, as CI runs it — the marker-delimited command in `CLAUDE.md`.

Step 5 is the acceptance evidence for SMA-592. Steps 2-4 are the acceptance evidence for the
guards; a guard that cannot report red is worse than no guard.

---

## 8. Limitations

* **L1.** The `.pyi` fix (§4) makes a stub edit re-run the FFI smoke test. It does **not** make a
  stub that disagrees with the Rust fail. That is SMA-535's stub-drift gate, deliberately out of
  scope. Do not read §4 as closing SMA-535 in part.
* **L2.** §3 cannot change a CI verdict, because CI is Linux and the flags are darwin-scoped. Its
  value is local cache correctness. Stated so the next reader does not over-claim it.
* **L3.** §2.5's pin is exact-equality on `contracts:generate`'s inputs. It cannot see the drift
  step itself being deleted from `ci.yml`, nor its `moon run` line being neutered. `repo:actionlint`
  check 8 pins `moon ci` lines, not `moon run` lines. That residual is SMA-554's territory.
* **L4.** `contracts:generate` still declares no `outputs:`. This branch makes its cache **key**
  honest; it does not make Moon restore its output. A cache hit still leaves whatever is on disk.
  The unconditional drift step is what covers that, which is a second reason for D3.

---

## 9. Open questions for the plan

1. Does adding `/py/uv.lock` to `contracts:generate` cause unwanted churn? Every Python dependency
   bump would re-run `buf generate`. That is correct but broad. Measure the cost; `buf generate` is
   fast, and the drift step already runs it unconditionally on every CI run, so the marginal cost
   may be zero.
2. Should `.prototools` be narrowed? It pins twelve tools, only one of which (`buf`) affects
   codegen. Moon has no sub-file input granularity, so the answer is probably "accept it", but
   record the reasoning rather than leaving it implicit.
