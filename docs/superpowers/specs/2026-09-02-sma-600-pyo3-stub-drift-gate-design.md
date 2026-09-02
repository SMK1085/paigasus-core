# SMA-600 — A PyO3 stub-drift gate

**Status:** design (rev 2 — reworked after adversarial challenge; see §11 for the changelog)
**Issue:** SMA-600 (owns the real fix for SMA-535)
**Branch:** `feature/sma-600-py-a-pyo3-stub-drift-gate-the-hand-written-pyi-can-disagree`
**Verified against `main` @ `925fdd4` (moon 2.5.3, proto 0.61.1).**

`rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi` is hand-written, and it is
basedpyright's **entire** view of the PyO3 surface. Change a `#[pyfunction]` and forget the stub
and the stub stays self-consistent, the type checker passes, and nothing in the repo compares the
two. This spec adds the thing that compares them.

**It does so in two arms, and the split is the central design decision.** A source-text scanner
can read *types* but cannot see what PyO3's macros actually emit; a runtime introspection of the
compiled module is ground truth on the *exported surface* but carries no types. Each is blind to
exactly what the other sees. §5 is arm 1, §6 is arm 2, and §4.4 records the measured bypasses that
force the split.

---

## 1. Measured baseline

Every number below was measured on this branch at `925fdd4`, not reasoned about.

### 1.1 The three sets agree today

| Set | Source | Count |
| -- | -- | -- |
| **A** `#[pyfunction]` idents | `paigasus-py-bindings/src/lib.rs` | 12 |
| **B** `wrap_pyfunction!` registrations | the `#[pymodule]` body in the same file | 12 |
| **C** `def` names | `paigasus_py_bindings.pyi` | 12 |

### 1.2 Full signatures agree too — the load-bearing measurement

A prototype extracting both sides and mapping Rust types through a seven-entry table found
**zero** disagreements across all 12 functions: name, arity, parameter names *in order*,
parameter types and return type all match. Total runtime **8.4 ms**, standard library only.

This is what makes §3's decision affordable. Had the strictest comparison red on the unmutated
tree, the gate would have had to ship weaker or ship with an exemption table on day one. It does
not.

### 1.3 The runtime surface — three measurements that shaped §6

Measured by importing the installed module from `py/`:

* **`inspect.signature()` resolves on PyO3 `builtin_function_or_method` objects.** `prn_build`
  reports `(service, region, org, resource_type, resource_id)` — parameter names **in order**,
  from `__text_signature__`. It carries **no** type information, which is why arm 2 cannot
  replace arm 1.
* **The module exports 13 names, not 12.** maturin wraps the extension in a package, so the inner
  module object is itself visible as an attribute: `dir()` yields the 12 functions plus
  `paigasus_py_bindings`, and `prn_build.__module__` is `paigasus_py_bindings.paigasus_py_bindings`.
  Arm 2 **must** filter to callables and exclude module objects, or it compares 13 against 12 and
  reds on a correct tree. This is the kind of detail that turns a plausible design into a broken
  one, and it is recorded here so the implementation does not rediscover it.
* **The stub ships in the wheel, renamed.** It installs as
  `py/.venv/lib/python3.12/site-packages/paigasus_py_bindings/__init__.pyi`, not under its source
  basename. maturin relocates it into the package directory. So the spec's opening premise holds —
  there is an installed stub for basedpyright to read — but §5.4's identity assertion is on the
  **source** basename, which is what maturin keys on.

### 1.4 The current tree is the simplest possible shape

Measured across `rs/crates/`:

* the only PyO3 attributes present anywhere are `#[pyfunction]` (12) and `#[pymodule]` (1)
* **zero** `#[pyo3(name = …)]` renames, `#[pyclass]`, `#[pymethods]`, `text_signature`
* the stub carries **zero** top-level nodes that are not `def`s, and **zero** `*args`,
  `**kwargs`, defaults, positional-only or keyword-only parameters

Read this as a warning, not as comfort. Every shape absent today is a shape the scanner would
have to guess at the day it appears, and a wrong guess is a silent bypass. §4 is the answer.

### 1.5 What each edit selects today

Measured with `moon query tasks --affected`, parsed from the JSON `tasks` map — **not** grepped
for `"target"`, which counts scheduled upstreams as selections and would inflate both rows
(CLAUDE.md records this trap).

| Edited file | Tasks selected today | Count |
| -- | -- | -- |
| `paigasus_py_bindings.pyi` | `paigasus-kernel-py:test`, `repo:actionlint`, `repo:input-liveness`, `repo:publish-metadata` | 4 |
| `paigasus-py-bindings/src/lib.rs` | the five `paigasus-py-bindings-rs:*` tasks, `paigasus-kernel-py:test`, `repo:actionlint`, `repo:error-code-single-site`, `repo:input-liveness`, `repo:machete`, `repo:publish-metadata` | 11 |

Both rows are non-empty, and that is exactly the trap SMA-600 exists to name. **Not one of these
15 selections compares the stub to the Rust.** `paigasus-kernel-py:test` — the only one that even
loads both — imports the compiled module and never reads the `.pyi` at all; the three broad
`repo:*` gates are packaging and liveness checks. The pre-existing scheduling looks like coverage
and is not, which is why SMA-535's input-list fix reads plausible and buys nothing.

After this change both rows gain `repo:pyo3-stub-drift`, and arm 2 gives
`paigasus-kernel-py:test` a reason to read the `.pyi` it already keys on.

### 1.6 Reachability premises

* `repo:affected-smoke` lists `ci/**/*` among its own `inputs` (`moon.yml:204`), floored by
  `T_AFFECTED_SMOKE_REQUIRED_INPUTS` in `ci/actionlint/run.sh:2123`. A change under a **new**
  `ci/` directory therefore already schedules `repo:affected-smoke`, which is what makes §8's
  two `ci_targets.py` pins reachable. **No new `T_AFFECTED_SMOKE_REQUIRED_INPUTS` entry is
  required**. Check 8e's array is a containment test with an `-ge 20` floor (`run.sh:5012`) over
  its 23 entries, so adding nothing to it cannot break it. Two comments are stale about that
  array's size — `moon.yml:202-203` says 21 and `ci/actionlint/run.sh:2093` says twenty; this
  change corrects both, since it is a two-word fix in files it already touches and a wrong count
  is exactly what makes a floor look re-baselinable.
* **No `REQUIRED_REPO_TASKS` entry is needed**, and this was checked rather than skipped:
  `check_gate_inputs` emits "repo:{task} is absent from the graph" (`ci_targets.py:1359`) and
  `check_self_invocation` reports every pinned line missing for an absent task (`:1288`, via
  `scripts.get(task, "")`). The two registry entries in §8 therefore already floor the gate's
  existence, which is the guarantee `REQUIRED_REPO_TASKS` was added to provide for tasks that
  lacked it.
* All of the gate's proposed `inputs` match at least one tracked file, so `repo:input-liveness`
  passes without an `ALLOW_DEAD_INPUT` entry.

---

## 2. Why the neighbouring work does not close this

Recorded so the next reader does not re-litigate it.

**SMA-535's filed fix cannot work.** Adding `/rs/…` inputs to `py:typecheck` buys scheduling with
no coverage, for two independent reasons either of which is sufficient: basedpyright reads the
stub and never the Rust, so no input list changes what it reads; and `.moon/tasks/python.yml` runs
plain `uv run basedpyright` with no `--reinstall-package`, which is measured in-repo to serve a
cached wheel — so even the stub it reads is the installed copy, not the working tree's.

**SMA-594's A7 is a different gap.** It made the `.pyi` an input of `paigasus-kernel-py:test`, so
a stub edit now re-runs the FFI smoke test. That is scheduling. It does not make a stub that
disagrees with the Rust fail, and A7's `{upstream}/*.pyi` clause must not be read as partial
coverage here. Arm 2 (§6) is what finally spends that scheduling on something.

**SMA-434 is a different shape.** `paigasus-node-bindings/index.d.ts` opens with
`/* auto-generated by NAPI-RS */`. Generated glue is checked by regenerating and diffing, not by
comparing sets. Do not merge the two.

---

## 3. Decision: compare full signatures (AC 6)

**Decided: names, arity, parameter names in order, parameter types, and return type.** Not names
only.

The stub's entire reason to exist is to carry *types* into basedpyright. A gate that compared only
identifiers would leave unverified exactly the content the type checker acts on — it would pass a
Rust `f64 → i64` change against a stub still claiming `float`, and pass a parameter reorder in
`prn_build(service, region, org, resource_type, resource_id)` that silently breaks every
keyword-argument call site while the stub keeps describing the old order. Both are silent, both
are the drift class this gate is for.

The cost is a `RUST_TO_PY` map that must grow when a genuinely new type shape appears. That cost
is accepted deliberately, on the same reasoning `CONTRACTS_GENERATE_INPUTS` records: an edit that
changes the FFI type vocabulary **should** stop a human.

The map is closed:

| Rust | Python |
| -- | -- |
| `&str`, `String` | `str` |
| `i64`, `u64` | `int` |
| `f64` | `float` |
| `bool` | `bool` |
| `()` / absent | `None` |

`PyResult<T>` is unwrapped to `T` before the lookup — it is an error channel, not a value type,
and PyO3 raises rather than returning it to Python. `PyResult<()>`, a bare `()` and an absent
return type therefore all normalize to `None`, consistently.

### 3.1 The map's admission criterion — a type may NOT join it just because it appeared

This is the rule that stops the map's growth instruction from steering into a bug. A Rust type may
join `RUST_TO_PY` **only if** PyO3 exports the parameter to Python **and** the correspondence is
1:1. Anything else is a **refusal row in §4, never a map row**:

* **`Python<'_>`, `&Bound<'py, PyAny>`, `&PyAny` and friends** are injected by PyO3 and **not**
  exported. Mapping one would make the gate demand a stub parameter Python callers must never
  pass — the gate would enforce a *wrong* stub. Refuse.
* **`Option<T>`** is arity-preserving but shape-changing (`T | None`), and **`Vec<u8>` / `&[u8]`**
  admit more than one correct Python spelling (`bytes`, `bytearray`, `Sequence[int]`). A map row
  would pick one and mandate it. Refuse until a human decides, in a PR that says why.

Today the map's closure is silently doing double duty as an arity guard — an unmapped
`Python<'_>` reds because it is unmapped, not because anything models injection. §4 makes that
explicit rather than leaving it load-bearing by accident, and §7.1 carries a fixture asserting
`Python<'_>` reds.

### 3.2 The stub is a mechanical projection — no divergence waiver

**Decided: no `ALLOW_SIGNATURE_DIVERGENCE` table.** The stub is a mechanical projection of the
Rust signature, and an author who wants a different annotation changes the Rust or files an issue.

The known cost, recorded rather than waved away: PyO3's `f64` extractor accepts a Python `int`, so
`mint_uuid7(unix_ms: float, …)` **under-specifies** what the function actually accepts — and the
only in-repo caller passes an int (`paigasus_kernel/__init__.py:23`, `time.time_ns() // 1_000_000`,
which type-checks only because `int` is promotable to `float` under PEP 484's numeric tower). An
author wanting `int | float`, a `Literal`, or a `NewType` is red with no escape.

That is accepted for now on YAGNI grounds: the case is hypothetical today, a second waiver table
is real surface, and the repo's idiom is to add an exemption table when the first genuine entry
appears rather than in anticipation. If it bites, add the table then — with a reason column, like
every other. This is a decision recorded so it is not re-litigated as an oversight.

---

## 4. Fail-closed: the gate refuses to guess

This is arm 1's central property, and the one most likely to be eroded later by someone who finds
it noisy. Each refused shape below is a **bypass**, not a curiosity: if the scanner guessed wrong
on any of them it would report a green comparison over a set it had mis-extracted.

### 4.1 The module body is default-deny

Not a refusal *list* but a permission list, because the ways to register something under a name
other than its own are open-ended. **Exactly two statement forms are permitted** in the
`#[pymodule]` body:

```rust
m.add_function(wrap_pyfunction!(<bare-ident>, m)?)?;
m.add_wrapped(wrap_pyfunction!(<bare-ident>))?;
```

plus the trailing `Ok(())`. **Any other statement is rc 1, naming it.** This closes three channels
that a refusal list would have missed, all of which yield **A == B == C** over a module that
exports something else:

| Channel | What happens |
| -- | -- |
| `m.add("alias", wrap_pyfunction!(f, m)?)?` | `PyModule::add` takes an arbitrary name string, so the export is `alias` while set **B** records `f` and a `def f` in the stub satisfies **C** |
| a submodule — `PyModule::new` + `child.add_function(...)` + `m.add_submodule(&child)?` | `g` is exported as `child.g`, but **B** records a top-level `g` and a top-level `def g` satisfies **C** |
| `PyModule::add_class`, `add_submodule`, or a path-qualified `wrap_pyfunction!(a::b, …)` | the registration is not the bare ident **B** records |

### 4.2 `macro_rules!` anywhere in scope is rc 1

A `macro_rules!` that emits `#[pyfunction]` items is invisible to a source scanner: it reads the
macro *definition*, never its expansions, so N real exports are absent from **A** *and* **B**, and
a stub that also omits them — the exact drift being hunted — passes green. No text scanner can
close this, which is one of the two reasons arm 2 exists (§4.4). Arm 1 refuses outright.

### 4.3 The remaining refusals

**Rust side — rc 1:**

| Shape | Why refusing matters |
| -- | -- |
| `#[pyfunction(…)]` / `#[pyo3(…)]` carrying arguments, **on any attribute line in the window** | may carry `name = "…"` or `signature = (…)`, renaming or reshaping the exported symbol |
| `#[pyclass]`, `#[pymethods]` | a whole class surface the stub would describe and the scanner does not model |
| `#[pymodule_export]`, a declarative module, a second `#[pymodule]`, **zero** `#[pymodule]` | changes where registrations come from; set **B** would be extracted from the wrong place or not at all |
| `#[cfg(…)]` on a `#[pyfunction]` **or on an enclosing `mod`** | the exported set becomes configuration-dependent, so one static answer is wrong |
| a `fn` signature the scanner cannot parse | never silently dropped from set **A** |
| the same function name declared twice across the scanned files | a `{name: Signature}` map would silently overwrite |

**Stub side — rc 1:** `*args`, `**kwargs`, parameter defaults, positional-only or keyword-only
parameters, decorators (`@overload` included), `async def`, a missing parameter or return
annotation, and any top-level node that is not a `def`, an `import`/`from … import`, or the module
docstring.

**The parse contract, stated so two implementers build the same scanner:**

1. **The attribute window** is every attribute line between `#[pyfunction]` and the `fn` item.
   Intervening attributes are **default-deny with an allowlist**: `#[allow(…)]`, `#[expect(…)]`,
   `#[doc = …]`, `#[inline]`, `#[must_use]` and `///` doc comments are permitted and ignored;
   anything else is rc 1. This is what makes a `#[pyo3(name = "x")]` on its own line refused
   rather than skipped past. The allowlist's criterion is what matters more than its membership:
   every member is a codegen or lint hint that cannot affect the exported surface, while anything
   that could rename or reshape the export is refused. `#[inline]` and `#[must_use]` were added
   during implementation on that criterion — this rev-2 text named only four, and refusing two
   idiomatic no-effect attributes would have rejected ordinary Rust for no safety gain.
2. **Item modifiers** `pub`, `pub(crate)`, `pub(super)` are accepted and ignored. `async fn`,
   `unsafe fn`, `const fn` and `extern` are rc 1. A raw identifier `fn r#type` is rc 1 — PyO3
   strips the `r#` from the exported name and the scanner must not guess at that.
3. **Layout-blind parsing.** `rs/rustfmt.toml` sets `max_width = 200` and `prn_build`'s signature
   is already 116 characters, so a sixth parameter makes rustfmt emit one parameter per line. The
   parse is **paren-balanced across newlines** from `fn` to the matching `)`, then `->` to the
   opening `{` — never line-oriented, never a single regex over one line.
4. **`mod` has no model, so nesting is refused.** `SCAN_GLOB`'s `**` covers a future
   `src/prn/mod.rs` at the *file* level, but a `#[pyfunction]` inside an inline `mod { … }` block
   is rc 1, as is any `#[cfg]` on such a block.
5. **Duplicate names across files** are rc 1, per the table above.
6. **`strip_noise` is copied verbatim from `ci/http-extractor/check.py`**, together with its
   self-test rows — the lifetime-vs-char-literal row and the line-offset row. That file's own
   comment calls the stripper "exactly the kind of thing that silently does the wrong thing", and
   it already handles raw strings (`r#"…"#`), Rust's *nesting* block comments, and the lifetime
   trap ("the classic way a naive stripper swallows the rest of a file and takes the gate with
   it"). Re-deriving it would be re-earning a bug the repo has already paid for once. Reuse is by
   **copy with attribution**, not import: `ci/` gates are deliberately standalone, and a shared
   module would couple two gates' schedules.

### 4.4 What refusing cannot close — and why arm 2 exists

Two of the above are refusals *because a source scanner is the wrong instrument*, not because the
shape is exotic:

* **`macro_rules!`-generated pyfunctions** (§4.2) — the exports exist and the scanner cannot see
  them.
* **PyO3 macro behaviour itself** — a PyO3 upgrade can change what `#[pyfunction]` emits without
  any source diff. Arm 1 reads source text and is structurally blind to this.

Arm 1 handles both by refusing to run, which is safe but means the repo simply cannot use those
constructs. Arm 2 (§6) makes them *checkable* instead, because it asks the compiled module what it
actually exports.

### 4.5 One waiver table

`ALLOW_UNPARSED_SHAPE` **ships empty** and requires a non-empty reason string per entry — the
repo's universal idiom (`T_EXEMPT`, `ALLOW_DEAD_INPUT`, `REQUIRED_INPUT_SKIP`, `BRANCH_SKIP`,
`COE_SKIP`, `ALLOW_NO_CARGO_BACKING` all ship empty or reasoned). A waiver naming a shape that is
no longer present is itself rc 1, so the table cannot silently rot. It waives *shapes the scanner
cannot read* and nothing else — per §3.2 there is deliberately no waiver for a comparison the
author disagrees with.

---

## 5. Arm 1 — `repo:pyo3-stub-drift`

### 5.1 `ci/pyo3-stub/check.py`

Standard library only — `ast`, `re`, `glob`, `sys`, `pathlib`, `tomllib`. No `uv`, no lockfile, no
third-party dependency. `ci/http-extractor/check.py` is the template.

```
usage: check.py [--self-test | --negative-control | --check]
  rc 0 clean · rc 1 the repo is wrong · rc 2 the checker itself is broken
```

**On rc 1 vs rc 2 for an unparseable signature**, the two in-repo precedents disagree and this
spec follows the one it does not template from, deliberately: `ci/http-extractor/README.md:80-83`
treats "the parser cannot read this shape" as rc 2, while `ci_targets.py:28-36` treats "a file
edited into a shape this gate cannot read" as "a red with a fix, not a broken tool". **This gate
uses rc 1**, because every refusal in §4 is something a *commit* introduced and a *commit* can
remove — the tool is fine. rc 2 is reserved for the scan root having moved or the checker
throwing. Stated here because the divergence from the cited template would otherwise look like an
oversight.

**Multi-crate by construction.** Hard-coding one crate would repeat a finding this repo has
already paid for twice — `moon.yml:706-709` on `repo:error-code-single-site` ("the one case it
exists for would be the one case it never runs on") and `ci_targets.py:228-231` on
`publish-metadata` ("a literal list could not select the task for a py package that does not exist
yet, which was itself a review finding"). So:

* `SCAN_GLOB = "rs/crates/bindings/*/src/**/*.rs"`
* `STUB_GLOB = "rs/crates/bindings/*/*.pyi"`

Both **byte-identical** to the Moon task's `inputs` entries, as `repo:http-extractor-envelope` and
`repo:workflow-credentials` each require of themselves — scheduling and scanning must not be able
to drift apart. A bindings crate is **PyO3-bearing** if it declares any `#[pyfunction]`; such a
crate with **no** `.pyi` is rc 1, and a `.pyi` with no PyO3-bearing crate is rc 1. Exactly one
`.pyi` per crate: zero or two is rc 1 naming what was found. `paigasus-node-bindings` and
`paigasus-wasm` declare no `#[pyfunction]` and are correctly out of scope, disk-conditionally —
the same idiom as A7's `{upstream}/*.pyi` clause.

Constants: `RUST_TO_PY` (§3), `ALLOW_UNPARSED_SHAPE` (§4.5), `PERMITTED_MODULE_STATEMENTS` (§4.1),
`PERMITTED_INTERVENING_ATTRS` (§4.3).

### 5.2 Extractors

* `rust_declarations(files) -> {name: Signature}` — set **A**, raising on §4 shapes
* `rust_registrations(files) -> set[str]` — set **B**, default-deny over the single `#[pymodule]` body
* `stub_definitions(path) -> {name: Signature}` — set **C**, via `ast.parse`
* `module_identity(crate) -> (pymodule_ident, maturin_module_name, stub_basename)` — §5.4

`Signature` is `(params: tuple[(name, py_type)], returns: py_type)` on both sides, with the Rust
side already mapped through `RUST_TO_PY`, so the comparison is plain equality on a normalized
value. `check()` reports **every** disagreement, not the first — a run that fixes one drift and
hides the next behind it wastes a CI round.

### 5.3 Real-tree positive controls

A gate that parses nothing reports clean. `ci/http-extractor/README.md:84-88` lists six rc-2
aborts for this reason, two of which have no analogue in a naive version of this gate. So `--check`
asserts, against the real tree:

* `SCAN_GLOB` and `STUB_GLOB` each match ≥ 1 file — **rc 2**, the scan root moved
* at least one PyO3-bearing crate is discovered, and `paigasus-py-bindings` is among them — rc 2
* **|A| ≥ 1, |B| ≥ 1, |C| ≥ 1**, and `sum_as_string ∈ A ∩ B ∩ C` — rc 2

The triangle is genuinely robust when only one set shrinks: a stripper that swallows part of
`lib.rs` drops **A** *and* **B** while **C** stays at 12, so it reds. But that robustness is
accidental, and these assertions make it explicit — the reasoning behind `REQUIRED_REPO_TASKS`
(`ci_targets.py:147-150`, "two EMPTY sets compare equal"), `REQUIRED_FFI_TASKS` and
`EXPECTED_PUBLISHABLE`.

**Deliberately not a strict-equality `EXPECTED_EXPORTS` pin of the 12 names.** That idiom is right
for `EXPECTED_PR_SUBJECTS` and `EXPECTED_PUBLISHABLE`, where the pinned set changes rarely and
each change is security-relevant. Here, *adding a `#[pyfunction]` correctly* is a routine
operation the gate should wave through; a strict pin would red every such PR and train people to
re-baseline reflexively, which is how a pin stops being read. The sentinel plus non-emptiness buys
the positive-control property without that tax. Arm 2 supplies a stronger control anyway, since it
compares against a live module that cannot be empty.

### 5.4 Module identity

`lib.rs:119-120` says the module's name "is provisional — it will be reconciled with the
`paigasus-kernel-py` wrapper when the wheel-integration issue lands." That rename is already
scheduled, and without this assertion it silently orphans the stub: rename the `#[pymodule]` fn
and `[tool.maturin] module-name` together, leave `paigasus_py_bindings.pyi` in place, and every
set still agrees while the stub now describes a module that does not exist and the type checker's
view of the real module is empty.

So arm 1 asserts three names are equal: the `#[pymodule]` fn ident (`lib.rs:122`),
`[tool.maturin] module-name` (`pyproject.toml:47`, read with `tomllib`), and the `.pyi` **source**
basename. Per §1.3 maturin relocates the file to `<package>/__init__.pyi` on install, so the
source basename is the right thing to bind.

This makes the crate's `pyproject.toml` a required input (§8).

### 5.5 `moon.yml`

```yaml
  pyo3-stub-drift:
    description: 'Assert the hand-written PyO3 stubs agree with the Rust they describe (SMA-600).'
    script: |
      set -euo pipefail
      python3 ci/pyo3-stub/check.py --self-test
      python3 ci/pyo3-stub/check.py --negative-control
      python3 ci/pyo3-stub/check.py --check
    toolchain: 'system'
    inputs:
      - 'rs/crates/bindings/*/src/**/*.rs'
      - 'rs/crates/bindings/*/*.pyi'
      - 'rs/crates/bindings/*/Cargo.toml'
      - 'rs/crates/bindings/*/pyproject.toml'
      - 'rs/Cargo.lock'
      - 'ci/pyo3-stub/**/*'
```

`set -euo pipefail` is required, not decorative: Moon does not enable errexit for `script:` blocks
and takes the block's status from its **last** command, so without it a failing `--self-test` is
masked by a passing `--check`.

**The three manifest inputs are load-bearing, not padding.** `Cargo.toml` — a `[features]` toggle
can `cfg`-gate a `#[pyfunction]`, which §4.3 says must be refused, so whether the gate should
refuse is decided by a file it must key on; CLAUDE.md's SMA-560 entry states this rule and
`paigasus-kernel-py:test` already lists all three for it. `pyproject.toml` — §5.4 reads
`module-name` from it. `rs/Cargo.lock` — pins the pyo3 version, and §9 N4 concedes a PyO3 upgrade
can change macro behaviour.

The name joins the existing `-drift` family (`parity-corpus-drift`, `next-env-drift`,
`observability-drift`).

---

## 6. Arm 2 — a runtime surface test

`py/packages/paigasus-kernel/tests/test_stub_surface.py`, inside the existing
`paigasus-kernel-py:test` task.

**Why here and not in arm 1.** That task already runs `touch` + `uv sync --reinstall-package
paigasus-py-bindings` before pytest (`py/packages/paigasus-kernel/moon.yml:36`), so the module it
imports is built from the working tree, and it already lists the `.pyi`, both Rust sources and
every manifest among its `inputs`. The runtime check therefore costs **zero registry edits and
zero new build work** — it spends a rebuild the repo is already paying for.

**What it asserts.** The exported surface of the *imported module* against the working-tree
`.pyi`:

* the set of exported **callables** equals the stub's `def` set
* for each, `inspect.signature()`'s parameter names **in order** equal the stub's

**What it catches that arm 1 cannot** — every channel in §4.1 and §4.2, plus `cfg`-gating, plus
N4's PyO3-upgrade case. The live module is ground truth: an aliased `m.add("alias", …)` shows up
as `alias`, a submodule's function is simply absent from the top level, and a macro-generated
export is present and countable. Arm 1 refuses these; arm 2 checks them.

**What it cannot do** — types. `__text_signature__` carries names and order only (§1.3), which is
why it cannot replace arm 1 and why §3's decision still needs a source-text scanner.

**Two implementation constraints, both measured (§1.3).** It must filter to callables and exclude
module objects, or it compares 13 against 12 and reds on a correct tree. And it must read the
**working-tree** `.pyi`, not the installed `__init__.pyi` — both are fresh after
`--reinstall-package`, but the working tree is the unambiguous one and avoids inheriting N5's
staleness class.

---

## 7. Proof it can red

Three mechanisms. They are not redundant, and the reasoning for each is stated because the first
draft of this spec got it wrong.

### 7.1 `--self-test` — synthetic fixtures, permanent

An in-process table of synthetic `(rust_src, pyi_src)` pairs with expected verdicts:

| Row | Mutation | Expect |
| -- | -- | -- |
| baseline | three sets agree | rc 0 |
| AC 1 | `#[pyfunction]` added and registered, absent from the stub | rc 1 |
| AC 2 | `wrap_pyfunction!` removed, declaration and stub kept | rc 1 |
| AC 3 | `def` deleted from the stub, Rust unchanged | rc 1 |
| §3 | Rust `f64 → i64`, stub untouched | rc 1 |
| §3 | two parameters transposed in Rust only | rc 1 |
| §3 | a parameter added in Rust only | rc 1 |
| §3 | return type changed in Rust only | rc 1 |
| §3 | a Rust type absent from `RUST_TO_PY` | rc 1, naming the type |
| §3.1 | a `Python<'_>` parameter | rc 1 — refused, never mapped |
| §3 | `PyResult<()>`, bare `()`, and an absent return all normalize to `None` | rc 0 |
| §4.1 | `m.add("alias", wrap_pyfunction!(f, m)?)?` | rc 1 |
| §4.1 | a submodule registration | rc 1 |
| §4.1 | `add_class` / `add_submodule` / path-qualified `wrap_pyfunction!` | rc 1 |
| §4.2 | a `macro_rules!` anywhere in scope | rc 1 |
| §4.3 | each refused Rust shape — one row each, incl. the attribute window, `async fn`, `fn r#type`, inline `mod`, duplicate names | rc 1 |
| §4.3 | `#[allow(…)]` between the attribute and the `fn` | rc 0 — permitted |
| §4.3 | a signature rustfmt has split one-parameter-per-line | rc 0 — layout-blind |
| §4.3 | each refused stub shape — one row each | rc 1 |
| §4.3 | `strip_noise`'s two inherited rows (lifetime-vs-char-literal, line offset) | rc 0 |
| §4.3 | `#[pyfunction]` inside a `///` comment or a raw string | rc 0 — no phantom |
| §4.5 | a waiver naming a shape not present | rc 1 |
| §5.1 | a PyO3-bearing crate with no `.pyi`; two `.pyi` in one crate | rc 1 |
| §5.4 | `#[pymodule]` ident, `module-name` and stub basename disagree | rc 1 |

### 7.2 `--negative-control` — the real tree, permanent

**This replaces the first draft's by-hand transcript, and the reasoning that omitted it was
wrong.** That draft argued no control was needed because this gate has no `run.sh` — but a
`run.sh` exists in `repo:workflow-credentials` solely to translate rc 3 into rc 1, while
`--negative-control` is a **mode of the checker** and an entirely separate thing. Per
`ci/workflow-credentials/README.md:13-18`, a self-test asserts against a fixture table and a
negative control asserts against the **real tree**; CLAUDE.md's SMA-601 entry records the measured
lesson that "the bare mode alone is a gate that can lie". `ci/http-extractor` genuinely has no
control, but it is the weakest precedent in the repo on this axis and should not have been cited
as settled.

So `--negative-control` copies the real crate into a `mktemp -d`, applies each of the three AC
mutations in turn to that copy, and asserts each reds. This **discharges AC 4 permanently** rather
than as a one-time transcript that decays the moment someone edits the checker.

### 7.3 AC 4's by-hand run, once

AC 4 asks for the three mutations proven red on a scratch copy and restored. §7.2 automates
exactly that, but the run is also performed by hand during implementation and its transcript
recorded in the PR — the automation's first execution is not itself evidence that the automation
is right.

---

## 8. Registry wiring (AC 5, AC 7)

Four edits, unchanged in number from rev 1 despite the widened inputs.

1. **`.github/workflows/ci.yml`** — `:pyo3-stub-drift` appended to the `T=(…)` array, which must
   stay a **single-line** bash array (SMA-541).
2. **`CLAUDE.md`** — the same target inside the `<!-- ci-targets:begin -->` / `<!-- ci-targets:end -->`
   markers. `check_docs` compares the two lists for **ORDERED** equality, reporting the "first
   divergence at position {i}" (`ci_targets.py:1142-1152`), so the target must occupy the same
   position in both. Do not add a second copy of either marker anywhere in the file, including
   inside backticks in prose: the count becomes 2 and the gate reds (SMA-541).
3. **`ci_targets.py` — `SELF_SCHEDULED_GATES["pyo3-stub-drift"]`** — the four `moon.yml` script
   lines, whole-line matched: `set -euo pipefail`, `--self-test`, `--negative-control`, `--check`.
   Whole-line matching is load-bearing in the usual direction — all three invocations share the
   prefix `python3 ci/pyo3-stub/check.py`, so a substring test would report the gate fully wired
   after any one had been deleted.
4. **`ci_targets.py` — `SELF_TASK_EXPECTED_GLOBS["pyo3-stub-drift"]`** — the six `inputs`, exact.
   `check_gate_inputs` compares **globs sorted, then literal files sorted**
   (`ci_targets.py:1381-1396`), not the authored order, so the tuple reads:
   `ci/pyo3-stub/**/*`, `rs/crates/bindings/*/Cargo.toml`, `rs/crates/bindings/*/*.pyi`,
   `rs/crates/bindings/*/pyproject.toml`, `rs/crates/bindings/*/src/**/*.rs`, then the literal
   `rs/Cargo.lock`. The pairing rule requires this or a reasoned `SELF_TASK_GLOBS_EXEMPT` entry;
   this gate takes the `EXPECTED_GLOBS` route because its whole authored set is static. Holding
   both would itself be reported.

**No `*_SH_CALL_SITES` pin** — those pin discrete lines inside a `run.sh` that
`SELF_SCHEDULED_GATES` cannot see. There is no shell script here. **No
`T_AFFECTED_SMOKE_REQUIRED_INPUTS` entry** and **no `REQUIRED_REPO_TASKS` entry**, both per §1.6.

Arm 2 needs **no** registry edits: it is a test file inside a task that already exists and already
keys on every path it reads.

---

## 9. Non-goals and limitations

Recorded here and in `ci/pyo3-stub/README.md`'s Limitations section, so each is a known residual
rather than an oversight.

**N1 — the wrapper's re-export set.** `paigasus_kernel/__init__.py` imports all 12 names at module
scope and repeats them in `__all__`. The two halves fail differently, and the first draft
conflated them:

* a stale name in the **import list** (`__init__.py:5-18`) is an `ImportError` at collection,
  since `tests/test_parity.py` imports the package — genuinely covered
* a stale entry in **`__all__` alone** (`:26-40`) raises nothing at import. It would red, if at
  all, via basedpyright's `reportUnsupportedDunderAll` under `py:typecheck` — a different task,
  and one N5 documents as reading a stale installed copy. **Not covered.**
* a **missing** re-export is silent either way: add a `#[pyfunction]`, stub it, pass both arms,
  and consumers still cannot reach it through `paigasus_kernel`

Out of scope deliberately: different workspace, different task, and a legitimate curation point
rather than a mirror (`__all__` already carries `mint`, which is not a `#[pyfunction]`), so it
would need a subset rule plus an exemption table from day one. File a follow-up.

**N2 — the napi/wasm glue.** SMA-434's regenerate-and-diff shape. Not this one; see §2.

**N3 — semantics.** Both arms prove the stub *describes* the Rust. Neither proves either is
*correct*; the parity corpus covers that.

**N4 — PyO3 macro behaviour.** Arm 1 reads source text and cannot see what the macros emit. Arm 2
closes most of this by asking the compiled module, which is why the split exists (§4.4).

**N5 — `py:typecheck`'s cached wheel.** SMA-535 §2's measurement stands and is not addressed here:
`uv run basedpyright` reads the installed copy of the stub. Both arms read the working tree (arm 2
after a forced reinstall), so both are unaffected — but the staleness in `py:typecheck` remains, and
closing it is a separate issue.

**N6 — nothing pins `check.py`'s internals.** `SELF_SCHEDULED_GATES` proves the three invocations
run; no analogue of `WORKFLOW_CREDENTIALS_SH_CALL_SITES` reaches inside a Python checker, so §7.1's
fixture table could be emptied and the gate would stay green. This is precedent-consistent —
equally true of `ci/http-extractor` and `ci/error-registry` — so it is a recorded residual and not
a defect, in the style of `ci/release-parity/README.md`'s L5.

---

## 10. Acceptance criteria mapping

| AC | Where |
| -- | -- |
| 1 — unstubbed `#[pyfunction]` fails | §7.1 row AC 1; §7.2; arm 2 §6 |
| 2 — removed registration fails | §7.1 row AC 2; §7.2; arm 2 §6 |
| 3 — deleted stub `def` fails | §7.1 row AC 3; §7.2; arm 2 §6 |
| 4 — each proven red by mutation, then restored | §7.2 (automated, permanent) + §7.3 (by hand, once) |
| 5 — selected by a Rust **or** stub change; reachability | §5.5 `inputs`; §1.5, §1.6; §8 item 4. **Note** the inputs now include the crate's manifests, on the §5.5 reasoning — read AC 5's "Rust" as including the files that decide what the Rust compiles to |
| 6 — signature comparison decided explicitly | §3 — **full signatures**, with §3.1's admission criterion and §3.2's no-divergence decision |
| 7 — `T=(…)` and CLAUDE.md's marker command | §8 items 1–2 |

---

## 11. Changelog — rev 1 → rev 2

Rev 1 was challenged adversarially and came back NEEDS REWORK with three BLOCKERs. Folded:

* **§4.1 default-deny module body** — closes `m.add("alias", …)`, submodules, and path-qualified
  registrations, three channels that yielded A == B == C over a wrong exported surface
* **§4.2 + §6 arm 2** — `macro_rules!`-generated pyfunctions are invisible to any source scanner;
  arm 1 refuses, arm 2 checks. This is the largest change and the reason the design has two arms
* **§5.4 module identity** — the stub filename was bound to nothing, and `lib.rs:119-120` announces
  the rename that would have orphaned it
* **§4.3 parse contract** — six under-specified points enumerated; `strip_noise` reused verbatim
* **§7.2 negative control** — rev 1's reason for omitting one was a non-sequitur, conflating a
  `run.sh`'s exit-code translation with a checker mode. It also automates AC 4
* **§5.3 positive controls** — non-emptiness and a sentinel, with the reasoning for rejecting a
  strict-equality `EXPECTED_EXPORTS` pin recorded
* **§5.1 multi-crate globs** — hard-coding one crate was the same class of finding this repo has
  already paid for twice
* **§3.1 map admission criterion** — "grow the map" steers into an arity bug on `Python<'_>`
* **§5.5 manifest inputs** — `Cargo.toml`, `pyproject.toml`, `rs/Cargo.lock` all decide the answer
* **§1.3** — three runtime measurements taken in response to the challenge, one of which (13
  exports, not 12) would have broken arm 2 had it been assumed rather than measured
* four MINORs: rc 1 vs rc 2 divergence stated (§5.1), `check_docs` ordered equality (§8.2),
  `SELF_TASK_EXPECTED_GLOBS` sort order (§8.4), N1 split into its two halves (§9)

**Considered and rejected:** an `ALLOW_SIGNATURE_DIVERGENCE` table (§3.2 — decided as
"mechanical projection", cost recorded); a strict-equality `EXPECTED_EXPORTS` pin (§5.3 — would
tax every legitimate addition); replacing arm 1 with arm 2 entirely (§6 — `__text_signature__`
carries no types, so §3's decision would be unimplementable).
