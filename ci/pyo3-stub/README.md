<!-- SPDX-License-Identifier: Apache-2.0 -->

# pyo3-stub — the PyO3 stub-drift gate

`repo:pyo3-stub-drift` runs `check.py --self-test`, then `check.py --negative-control`, then
`check.py --check`, in one `script:` block under `set -euo pipefail`.

## What it gates

For every PyO3 crate under `rs/crates/bindings/*` that ships a `.pyi` next to its `Cargo.toml`,
three sets must match:

* **A** — `#[pyfunction]` declarations in `rs/crates/bindings/*/src/**/*.rs`
* **B** — `wrap_pyfunction!` registrations inside that crate's `#[pymodule]` body
* **C** — `def` names in `rs/crates/bindings/*/*.pyi`

Set membership alone is not enough: **A must match C on full signatures** — name, arity,
parameter names in order, parameter types, and return type, with Rust types mapped through a
small closed table (`RUST_TO_PY`: `&str`/`String` -> `str`, `i64`/`u64` -> `int`, `f64` ->
`float`, `bool` -> `bool`, `()` -> `None`). A type joins that table only when PyO3 exports the
parameter to Python **and** the correspondence is 1:1 — `Python<'_>` and `&Bound<'py, PyAny>` are
injected by PyO3 and never exported, so they are **refused**, not mapped, the moment they appear
in a `#[pyfunction]` signature; `Option<T>` (arity-preserving but shape-changing) and `Vec<u8>`/
`&[u8]` (more than one correct Python spelling) are refused for the same reason. Measured on the
unmutated tree at design time: zero disagreements across all twelve functions in
`paigasus-py-bindings`, which is what makes shipping the strict full-signature comparison on day
one affordable (the design doc's §1.2).

A fourth check, independent of A/B/C, is **module identity** (§5.4): the `#[pymodule]` function's
own name, `[tool.maturin] module-name` in `pyproject.toml`, and the stub's file basename must all
agree. `lib.rs` renaming the pymodule function, or `pyproject.toml` renaming the built module,
orphans the stub silently otherwise — nothing else in this gate's comparison would notice, since
A/B/C never read either of those two identifiers.

This exists because `paigasus_py_bindings.pyi` is hand-written and is basedpyright's **entire**
view of the PyO3 surface (SMA-535). Change a `#[pyfunction]`'s signature and forget the stub, and
the stub stays internally self-consistent — `py:typecheck` passes, because nothing before this
gate ever compared the stub against the Rust it claims to describe.

## What it does NOT gate

**Arm 2's territory.** `py/packages/paigasus-kernel/tests/test_stub_surface.py` asks the
*compiled* module what it actually exports (via `dir()` + `inspect.signature()`), which is ground
truth for the *exported surface* but carries no static types. This file (arm 1) reads source
text, which sees types but cannot see what PyO3's macros actually emit. Each arm is blind to
exactly what the other sees — see "How it reads Rust without a Rust parser" below for the shapes
arm 1 refuses rather than guesses at, and the design doc's §4.4 for why arm 2 exists at all.

Six further things this gate does not do, all recorded in the design doc's §9 as deliberate
non-goals:

* **N1 — the wrapper's re-export set.** `paigasus_kernel/__init__.py` imports and re-exports all
  twelve names. A stale entry in the **import list** is a genuine `ImportError` at collection
  (covered by the package's own tests); a stale entry in `__all__` **alone**, or a **missing**
  re-export of a function that passes both of this gate's arms, is not covered here at all —
  different workspace, different task, and (for `__all__`) a legitimate curation point rather
  than a mirror, since it already carries names that are not `#[pyfunction]`s.
* **N2 — the napi/wasm glue.** That is SMA-434's regenerate-and-diff shape, not this one.
* **N3 — semantics.** Both arms prove the stub *describes* the Rust. Neither proves either is
  *correct*; the parity corpus covers that.
* **N4 — PyO3 macro behaviour.** Arm 1 reads source text and cannot see what the macros emit — a
  PyO3 upgrade can change what `#[pyfunction]` produces with no source diff at all. Arm 2 closes
  most of this by asking the compiled module instead.
* **N5 — `py:typecheck`'s cached wheel.** `uv run basedpyright` is measured, separately, to read
  the *installed* copy of the stub, not the working-tree file. Both of this gate's arms read the
  working tree (arm 2 after a forced reinstall), so neither is affected by that staleness — but it
  remains true of `py:typecheck` itself, and closing it is a separate issue (SMA-535's original
  filing).
* **N6 -> see Limitations L1 below.**

## How it reads Rust without a Rust parser

`check.py` is pure standard library (`ast`, `re`, `glob`, `tomllib` — the Moon task is
`toolchain: 'system'`) and never shells out to cargo.

1. **Lexical pre-pass.** `strip_noise` is copied **verbatim** from `ci/http-extractor/check.py`,
   together with its self-test rows (the lifetime-vs-char-literal trap and the line-offset row).
   Comments and string/char literals are blanked to spaces, preserving length and newlines, so a
   `#[pyfunction]` inside a doc comment or a raw string mints no phantom declaration, `&'static`
   is never mistaken for the start of a char literal, and reported line numbers stay real. Reused
   by copy with attribution, not import — `ci/` gates are deliberately standalone, and a shared
   module would couple two gates' schedules.
2. **Paren-balanced, layout-blind parsing.** `rs/rustfmt.toml` sets `max_width = 200`, and one of
   this crate's real signatures is already 116 characters — a sixth parameter would push rustfmt
   to one parameter per line. The parse walks from `fn` to its matching `)`, then `->` to the
   opening `{`, across newlines, never line-oriented and never a single-line regex.
3. **The attribute window** is every attribute line between `#[pyfunction]` and the `fn` item —
   **and every attribute line above it**, walked backwards over the contiguous run of preceding
   attributes to the end of the previous item. Both halves are needed and the upward one is the
   less obvious: for a `#[cfg]` the upward order is the *only* one that works in Rust, since cfg
   is evaluated before the proc macro runs, and a `#[pyo3(name = "x")]` renames the export from
   above just as effectively as from below. It is **default-deny with an allowlist**: `PERMITTED_INTERVENING_ATTRS` in `check.py`
   names exactly five bracket attributes — `allow`, `expect`, `doc`, `inline`, `must_use` — every
   one a codegen or lint hint that cannot rename or reshape what PyO3 exports. `inline` and
   `must_use` are idiomatic on an ordinary function, so admitting them costs nothing and refusing
   them would reject correct Rust for no safety gain. A bare `///` doc comment is not a member of
   this list and needs none: `strip_noise`'s lexical pre-pass (item 1 above) blanks every `//`
   line, `///` included, before the attribute-window walk ever runs, so a doc comment is simply
   gone by the time the walk looks for `#[…]` — only the explicit `#[doc = "…"]` attribute form
   reaches the allowlist check at all. Anything else — including a bare `#[pyo3(name = "x")]` on
   its own line, which *can* rename the export — is refused rather than skipped past. The same
   window walk, and the same constant, gates both the `#[pyfunction]` window
   (`rust_declarations`) and the `#[pymodule]` window (`_pymodule_body`), and the same constant gates the backward walk too,
   so a change to the allowlist moves all three at once.
4. **The `#[pymodule]` body is default-deny, not a refusal list.** Exactly two statement shapes
   are permitted: `m.add_function(wrap_pyfunction!(<bare-ident>, m)?)?;` and
   `m.add_wrapped(wrap_pyfunction!(<bare-ident>))?;`, plus the trailing `Ok(())`. Anything else is
   refused, naming it. A permission list rather than a refusal list closes three channels a
   refusal list would miss, all of which would otherwise yield **A == B == C** over a module that
   actually exports something else: `m.add("alias", wrap_pyfunction!(f, m)?)?` (exports `alias`,
   not `f`); a submodule built with `PyModule::new` + `child.add_function` +
   `m.add_submodule(&child)?` (exports `child.g`, not a top-level `g`); and `add_class`,
   `add_submodule`, or a path-qualified `wrap_pyfunction!(a::b, …)` registration.

## Fail-closed properties

A gate that fails open is worse than no gate — it converts "unguarded" into "believed guarded".
Every shape below is `Refused` at **rc 1** (a repo problem to fix, not a broken checker) unless
noted; a handful of infrastructure-only failures are `InfraError` at **rc 2**.

**Refused (rc 1) — the parser will not guess:**

* `macro_rules!` anywhere in scope. Its expansions are invisible to a source scanner — real
  exports would be silently absent from sets A *and* B, and a stub that also omits them (the
  exact drift this gate hunts) would pass green. No text scanner can close this; it is one of the
  two reasons arm 2 exists.
* `#[pyfunction(…)]` / `#[pyo3(…)]` carrying arguments, on any line in the attribute window — may
  rename or reshape the exported symbol.
* `#[pyclass]`, `#[pymethods]` — a whole class surface this scanner does not model. Refused
  **file-globally**, like `macro_rules!`, and for the same fail-closed reason: a class-only crate
  declares no `#[pyfunction]`, so without this it was classified not-PyO3-bearing, `analyze`
  short-circuited before the module-body default-deny could see `m.add_class::<Foo>()?`, and a
  crate with a real export and **no stub at all** reported clean (measured).
* `#[pymodule_export]`, a declarative module, a second `#[pymodule]`, or **zero** `#[pymodule]`.
* `#[cfg(…)]` / `#[cfg_attr(…)]` on a `#[pyfunction]` (above it or below it) or on an enclosing
  `mod` — the exported set becomes configuration-dependent, so one static answer would be wrong.
  Closed twice over: the backward attribute walk names the attribute and its line, and a
  **file-global** `#[cfg`/`#[cfg_attr` refusal covers the shapes that have no `#[pyfunction]`
  beneath them to walk back from (a `#[cfg]` on a `mod foo;` declaration, a `#![cfg(...)]` inner
  attribute). Coarse but fail-closed, and free on the real tree: no `#[cfg`, `#[cfg_attr`,
  `#[pyclass]` or `#[pymethods]` appears anywhere under `rs/crates/bindings/*/src/`.

  Two things to know when this one reds, because neither is obvious from the message. The
  **file-global refusal runs first**, so a `#[cfg]` reports the whole-file message and you will
  never see the backward walk's attribute-and-line form for it — that form surfaces only for the
  other refused attributes (`#[pyo3(...)]` and friends). And `SCAN_GLOB` covers **all three**
  bindings crates, so a `#[cfg]` added to `paigasus-node-bindings` or `paigasus-wasm` — neither of
  which is a PyO3 crate or has a `.pyi` — reds this gate too, with a message about an exported set
  that means nothing for those crates. `#[cfg(target_arch = "wasm32")]` is ordinary in a wasm shim,
  so this is the likeliest way to meet this refusal without having touched PyO3 at all. The
  `macro_rules!` and inline-`mod` refusals share the property; `#[cfg]` is just far commoner.
* A `fn` signature the scanner cannot parse, or the same function name declared twice across the
  scanned files (a `{name: Signature}` map would silently overwrite the first).
* `async fn`, `unsafe fn`, `const fn`, `extern`, and a raw identifier `fn r#type` (PyO3 strips the
  `r#` from the exported name; the scanner must not guess at that).
* A `#[pyfunction]` inside an inline `mod { … }` block — nesting has no model.
* Stub-side: `*args`, `**kwargs`, defaults, positional-only or keyword-only parameters,
  decorators (`@overload` included), `async def`, a missing parameter or return annotation, or
  any top-level node that is not a `def`, an import, or the module docstring.

**Infra (rc 2) — the checker itself, or its environment:**

* `SCAN_GLOB` or `STUB_GLOB` matching no file (the scan root moved).
* More than one `.pyi` matching `STUB_GLOB` for a single crate. rc **2**, not rc 1 — two stubs
  make the *scan shape* ambiguous and `discover()` cannot pick one without guessing, which is the
  principle this gate rests on; every other ambiguity that stage hits is rc 2 for the same reason.
  (The design doc said rc 1 until the final review aligned it to the code.) **Zero** stubs beside
  a PyO3-bearing crate is a different verdict — rc 1, reported by `analyze()`.
* Either of the three real-tree sets (A, B, C) being empty, or the sentinel function
  (`sum_as_string` in `paigasus-py-bindings`) missing from any of them — a gate that parses
  nothing reports clean, so "no problems found" must be backed by "the thing I know is there was
  actually found". These are this gate's positive controls (§5.3).

`--self-test` and `--negative-control` run **first**, in the same `script:` block, before
`--check` ever touches the real tree. `set -euo pipefail` is required: Moon does not enable
errexit for `script:` blocks and takes the block's status from its **last** command, so without
it a failing self-test or control would be masked by a passing real run. `--self-test` asserts
`analyze()` against synthetic in-memory fixtures (never touches the real tree); `--negative-control`
takes `discover()`'s real-tree output and mutates the in-memory strings four ways — an unstubbed
`#[pyfunction]` (AC 1), a removed registration (AC 2), a deleted stub `def` (AC 3), and a retyped
stub annotation, `mint_uuid7(unix_ms: float)` -> `int` (AC 4b) — asserting each reds, plus a fifth
row re-asserting the **unmutated** crate stays clean. AC 4b was added in the final review: AC 1-3
are all set-*membership* drift, so neutering `if decls[name] != stub[name]` was measured to leave
the control green, and the full-signature comparison — this gate's headline decision — was the one
thing the real-tree control never exercised. Neither mode proves
the other can report red; both are required.

## There is deliberately no waiver table

Not an omission — a decision, and a correction. `check.py` shipped an `ALLOW_UNPARSED_SHAPE` table
that this README and the design doc both presented as a working escape hatch. It was **inert**:
measured, a live waiver row did not suppress the `macro_rules!` refusal or any other §4 refusal,
because nothing on any refusal path consulted the table — only its own staleness report read it.
The final review deleted the table rather than implementing it.

**A §4 shape is fixed at the source, not waived.** That is the same reasoning that makes every
refusal here rc 1 rather than rc 2: a commit introduced the shape and a commit can remove it. A
waiver would be a way to keep the unreadable shape *and* a green gate — "believed guarded", the
one outcome worse than no gate at all. An inert table was worse still, because it documented an
escape hatch that did not exist. This matches the design doc's §3.2 decision to ship no divergence
table until a genuine entry appears; if an unreadable shape ever genuinely has to stay, add the
table then, wired to the refusal path, with a reason column like every other in the repo.

## Limitations

**L1 — Nothing pins this checker's internals.** `SELF_SCHEDULED_GATES` in
`ci/affected-graph/ci_targets.py` proves the three invocations (`--self-test`,
`--negative-control`, `--check`) actually run under `moon run repo:pyo3-stub-drift`; no analogue
of `WORKFLOW_CREDENTIALS_SH_CALL_SITES` reaches *inside* a Python checker to pin its assertion
logic. This was measured directly: neutering the `failures += 1` accounting inside
`negative_control()` — so the mutated-tree rows are still evaluated but never counted — still
printed `== pyo3-stub negative control passed ==` at the end, with the assertion silently
disabled. This is the same bypass shape CLAUDE.md records for `ci/release-parity/run.sh`'s L5
residual, and it is precedent-consistent rather than novel: `ci/http-extractor` and
`ci/error-registry` carry the identical exposure — an in-process Python checker has no
line-level pin the way a shell script's call sites do. Recorded as a residual, not a defect.

**L2 — `ast.unparse` does not normalize annotation spellings.** The stub's parameter and return
annotations are read with `ast.parse` + `ast.unparse` and string-compared against the mapped
Rust type. A stub written `builtins.int` where the map expects `int`, or `typing.Optional[str]`
where it expects `str | None`, will not string-match and will **red** even though the two spell
the same type. This is the intended, safe direction for a refuse-to-guess gate — it reddens on an
equivalent-but-differently-spelled annotation, it never *passes* one it cannot actually verify —
but it means a stub author must spell annotation types the way the map produces them, not any
semantically equivalent form.

**N1–N6** from the design doc's §9 are covered under "What it does NOT gate" above rather than
repeated here.
