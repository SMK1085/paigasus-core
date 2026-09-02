# SMA-600 — A PyO3 stub-drift gate

**Status:** design
**Issue:** SMA-600 (owns the real fix for SMA-535)
**Branch:** `feature/sma-600-py-a-pyo3-stub-drift-gate-the-hand-written-pyi-can-disagree`
**Verified against `main` @ `925fdd4` (moon 2.5.3, proto 0.61.1).**

`rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi` is hand-written, and it is
basedpyright's **entire** view of the PyO3 surface. Change a `#[pyfunction]` and forget the stub
and the stub stays self-consistent, the type checker passes, and nothing in the repo compares the
two. This spec adds the thing that compares them.

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

### 1.3 The current tree is the simplest possible shape

Measured across `rs/crates/`:

* the only PyO3 attributes present anywhere are `#[pyfunction]` (12) and `#[pymodule]` (1)
* **zero** `#[pyo3(name = …)]` renames, `#[pyclass]`, `#[pymethods]`, `text_signature`
* the stub carries **zero** top-level nodes that are not `def`s, and **zero** `*args`,
  `**kwargs`, defaults, positional-only or keyword-only parameters

Read this as a warning, not as comfort. Every shape absent today is a shape the scanner would
have to guess at the day it appears, and a wrong guess is a silent bypass. §4 is the answer.

### 1.4 What each edit selects today

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

After this change both rows gain `repo:pyo3-stub-drift` — 5 and 12 — and AC 5 is satisfied in
both directions rather than only from the stub side.

### 1.5 Reachability premises

* `repo:affected-smoke` lists `ci/**/*` among its own `inputs` (`moon.yml:204`), floored by
  `T_AFFECTED_SMOKE_REQUIRED_INPUTS` in `ci/actionlint/run.sh:2123`. A change under a **new**
  `ci/` directory therefore already schedules `repo:affected-smoke`, which is what makes §7's
  two `ci_targets.py` pins reachable. **No new `T_AFFECTED_SMOKE_REQUIRED_INPUTS` entry is
  required**. Check 8e's array is a containment test with an `-ge 20` floor over its 23 entries
  (measured — `moon.yml:202-203`'s comment still says 21 and is stale), so adding nothing to it
  cannot break it.
* All three of the gate's proposed `inputs` match at least one tracked file, so
  `repo:input-liveness` passes without an `ALLOW_DEAD_INPUT` entry.

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
coverage here.

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
and PyO3 raises rather than returning it to Python.

A Rust type absent from that map is **rc 1**, naming the type and its line and demanding either a
map entry or a waiver. It is never a skip. See §4.

---

## 4. Fail-closed: the gate refuses to guess

This is the gate's central property, and the one most likely to be eroded later by someone who
finds it noisy. Each refused shape below is a **bypass**, not a curiosity: if the scanner guessed
wrong on any of them it would report a green comparison over a set it had mis-extracted.

**Rust side — refuse and exit 1:**

| Shape | Why refusing matters |
| -- | -- |
| `#[pyfunction(…)]` / `#[pyo3(…)]` with arguments | may carry `name = "…"` or `signature = (…)`, renaming or reshaping the exported symbol behind the scanner's back |
| `#[pyclass]`, `#[pymethods]` | a whole class surface the stub would have to describe and the scanner does not model |
| `#[pymodule_export]`, a declarative module, a second `#[pymodule]` | changes where registrations come from; set **B** would be extracted from the wrong place |
| `#[cfg(…)]`-gated `#[pyfunction]` | the exported set becomes configuration-dependent, so a single static answer is wrong |
| `m.add_function(x)` where `x` is not `wrap_pyfunction!(…)`, or a `wrap_pyfunction!` outside the module body | set **B** would be incomplete or over-counted |
| a `fn` signature the scanner cannot parse | never silently dropped from set **A** |

**Stub side — refuse and exit 1:** `*args`, `**kwargs`, parameter defaults, positional-only or
keyword-only parameters, decorators (`@overload` included), `async def`, a missing parameter or
return annotation, and any top-level node that is not a `def`, an `import`/`from … import`, or the
module docstring.

**Comment and string stripping.** Both are stripped before the Rust scan, so a `///` doc comment
mentioning `#[pyfunction]` cannot mint a phantom declaration and a commented-out registration
cannot inflate set **B**. The crate's own module docstring at `lib.rs:3-7` already names
`paigasus-kernel` in prose, so this is not hypothetical hygiene.

**One waiver table.** `ALLOW_UNPARSED_SHAPE` **ships empty** and requires a non-empty reason
string per entry — the repo's universal idiom (`T_EXEMPT`, `ALLOW_DEAD_INPUT`,
`REQUIRED_INPUT_SKIP`, `BRANCH_SKIP`, `COE_SKIP`, `ALLOW_NO_CARGO_BACKING` all ship empty or
reasoned). A waiver that names a shape no longer present is itself an error, so the table cannot
silently rot.

---

## 5. Component design

### 5.1 `ci/pyo3-stub/check.py`

Standard library only — `ast`, `re`, `glob`, `sys`, `pathlib`. No `uv`, no lockfile, no
third-party dependency, and therefore **no `run.sh` wrapper**. `repo:workflow-credentials` needs
one solely to translate its checker's rc 3 into rc 1, because `uv` itself exits 1 on a failed
resolution and a shared code would let a PyPI outage read as an assertion failure. This gate
shells out to nothing, so that hazard does not exist and inventing the wrapper for symmetry would
be cargo-culting. `ci/http-extractor/check.py` is the precedent and the template.

```
usage: check.py [--self-test | --check]
  rc 0 clean · rc 1 the repo is wrong · rc 2 the checker itself is broken
```

`rc 2` comes from an `InfraError` / `OSError` handler in `main()`, exactly as
`ci/http-extractor/check.py:716-731` does. The distinction is load-bearing in one direction: a
missing scan root must not read as "the stub agrees with the Rust".

Module-level constants, each with the comment explaining why it is what it is:

* `SCAN_GLOB = "rs/crates/bindings/paigasus-py-bindings/src/**/*.rs"` — **byte-identical** to the
  Moon task's first `inputs` entry. `repo:http-extractor-envelope` and
  `repo:workflow-credentials` both state this requirement of themselves in `moon.yml`, for the
  same reason: scheduling and scanning must not be able to drift apart. `**` covers a future
  `src/prn/mod.rs`, so a new file is scanned the day it lands rather than the day someone
  remembers to widen a list.
* `STUB_GLOB = "rs/crates/bindings/paigasus-py-bindings/*.pyi"` — a glob, not a literal, matching
  the Moon input. One file today.
* `RUST_TO_PY` — §3's closed map.
* `ALLOW_UNPARSED_SHAPE` — §4's empty waiver table.

Three extractors, each returning a structure and raising on a shape it does not model:

* `rust_declarations(files) -> {name: Signature}` — set **A**
* `rust_registrations(files) -> set[str]` — set **B**, scoped to the single `#[pymodule]` body
* `stub_definitions(path) -> {name: Signature}` — set **C**, via `ast.parse`

`Signature` is `(params: tuple[(name, py_type)], returns: py_type)` on both sides, with the Rust
side already mapped through `RUST_TO_PY`, so the comparison is a plain equality on a normalized
value rather than a bespoke matcher.

`check()` then asserts `A == B` on names and `A == C` on full signatures, reporting **every**
disagreement rather than the first — a run that fixes one drift and hides the next behind it wastes
a CI round.

### 5.2 `ci/pyo3-stub/README.md`

Following `ci/http-extractor/README.md`'s sections: what it gates, what it does **not** gate, how
it reads Rust without a Rust parser, fail-closed properties, the `ALLOW_UNPARSED_SHAPE` table, and
Limitations (§8 here).

### 5.3 `moon.yml`

```yaml
  pyo3-stub-drift:
    description: 'Assert the hand-written PyO3 stub agrees with the Rust it describes (SMA-600).'
    script: |
      set -euo pipefail
      python3 ci/pyo3-stub/check.py --self-test
      python3 ci/pyo3-stub/check.py --check
    toolchain: 'system'
    inputs:
      - 'rs/crates/bindings/paigasus-py-bindings/src/**/*.rs'
      - 'rs/crates/bindings/paigasus-py-bindings/*.pyi'
      - 'ci/pyo3-stub/**/*'
```

`set -euo pipefail` is required, not decorative: Moon does not enable errexit for `script:`
blocks and takes the block's status from its **last** command, so without it a failing
`--self-test` is masked by a passing `--check` and the gate ships with no proof it bites.
`--self-test` runs **first**, per `repo:error-code-single-site` and
`repo:http-extractor-envelope`.

The name joins the existing `-drift` family (`parity-corpus-drift`, `next-env-drift`,
`observability-drift`).

---

## 6. Proof it can red

Two mechanisms, and they are not redundant.

### 6.1 `--self-test` — permanent

An in-process fixture table of synthetic `(rust_src, pyi_src)` pairs with expected verdicts. One
row per acceptance criterion and one per fail-closed rule:

| Row | Mutation | Expect |
| -- | -- | -- |
| AC 1 | `#[pyfunction]` added and registered, absent from the stub | rc 1 |
| AC 2 | `wrap_pyfunction!` removed, declaration and stub kept | rc 1 |
| AC 3 | `def` deleted from the stub, Rust unchanged | rc 1 |
| — | baseline: three sets agree | rc 0 |
| §3 | Rust `f64 → i64`, stub untouched | rc 1 |
| §3 | two parameters transposed in Rust only | rc 1 |
| §3 | a parameter added in Rust only | rc 1 |
| §3 | return type changed in Rust only | rc 1 |
| §3 | a Rust type absent from `RUST_TO_PY` | rc 1, naming the type |
| §4 | each refused Rust shape (one row each) | rc 1 |
| §4 | each refused stub shape (one row each) | rc 1 |
| §4 | `#[pyfunction]` inside a `///` comment | rc 0 — no phantom |
| §4 | a waiver naming a shape not present | rc 1 |

Plus two scope assertions against the **real** tree, in the style of
`ci/http-extractor/check.py:704-710`: `SCAN_GLOB` must still match at least one file, and the
match set must still contain `lib.rs`. A gate scanning zero files otherwise reports clean, which
is the failure mode a moved crate produces.

### 6.2 AC 4 — mutation on a scratch copy, by hand, once

AC 4 demands each of the three mutations be applied to a scratch copy of the real tree, observed
red, and restored. That is done during implementation and the transcript recorded in the PR. It
proves the gate bites **today**, against the real files; §6.1 is what keeps that true after the
scratch copy is gone and someone edits the checker. Neither substitutes for the other, which is
why both are in scope.

---

## 7. Registry wiring (AC 5, AC 7)

Four edits. Each is load-bearing; omitting any one reds `repo:affected-smoke` rather than this
gate, which is the design working as intended.

1. **`.github/workflows/ci.yml`** — `:pyo3-stub-drift` appended to the `T=(…)` array. It must stay
   a **single-line** bash array (SMA-541).
2. **`CLAUDE.md`** — the same target added inside the `<!-- ci-targets:begin -->` /
   `<!-- ci-targets:end -->` markers, kept in agreement with `T`. Do not add a second copy of
   either marker anywhere in the file, including inside backticks in prose: the count becomes 2
   and the gate reds (SMA-541).
3. **`ci/affected-graph/ci_targets.py` — `SELF_SCHEDULED_GATES["pyo3-stub-drift"]`** — the three
   `moon.yml` script lines, whole-line matched:
   `set -euo pipefail`, `python3 ci/pyo3-stub/check.py --self-test`,
   `python3 ci/pyo3-stub/check.py --check`. Whole-line matching is load-bearing here in the usual
   direction: the `--self-test` line and the `--check` line share the prefix
   `python3 ci/pyo3-stub/check.py`, so a substring test would report the gate fully wired after
   one of them had been deleted.
4. **`ci/affected-graph/ci_targets.py` — `SELF_TASK_EXPECTED_GLOBS["pyo3-stub-drift"]`** — the
   three `inputs`, exact. The pairing rule requires either this or a reasoned
   `SELF_TASK_GLOBS_EXEMPT` entry; this gate takes the `EXPECTED_GLOBS` route because its whole
   authored input set is three static globs, so exact matching is cheap and does not fight a
   legitimately growing list. Holding both would itself be reported.

**No `*_SH_CALL_SITES` pin.** Those exist to pin discrete lines inside a `run.sh` that
`SELF_SCHEDULED_GATES` cannot see. There is no shell script here, so there is nothing for such a
pin to reach — the same position `repo:http-extractor-envelope` and
`repo:error-code-single-site` are in.

**No `T_AFFECTED_SMOKE_REQUIRED_INPUTS` entry**, per §1.5: `ci/**/*` already floors reachability
for any new `ci/` directory.

---

## 8. Non-goals

Recorded here and in `ci/pyo3-stub/README.md`'s Limitations section, so each is a known residual
rather than an oversight.

**N1 — the wrapper's re-export set.**
`py/packages/paigasus-kernel/src/paigasus_kernel/__init__.py` imports all 12 names and repeats
them in `__all__`. A **stale** name there reds at import time, because `paigasus-kernel-py:test`
runs pytest which imports the package. A **missing** re-export is silent: add a `#[pyfunction]`,
stub it correctly, pass this gate, and Python consumers still cannot reach it through
`paigasus_kernel`. Deliberately out of scope — it lives in a different workspace under a different
task, and it is a legitimate curation point rather than a mirror (`__all__` already carries
`mint`, which is not a `#[pyfunction]` at all), so it would need a subset rule plus an exemption
table from day one. File a follow-up if it should be closed.

**N2 — the napi/wasm glue.** SMA-434's regenerate-and-diff shape. Not this one; see §2.

**N3 — semantics.** The gate proves the stub *describes* the Rust. It cannot prove either is
*correct*: a function whose stub and signature agree perfectly may still do the wrong thing, and
the parity corpus is what covers that.

**N4 — source text, not the compiled module.** The gate reads `.rs` and `.pyi` as text. It cannot
see what PyO3's macros actually emit, so a PyO3 upgrade that changed macro behaviour without
changing source syntax would be invisible to it. `paigasus-kernel-py:test`'s runtime FFI smoke
test is the control that would notice.

**N5 — `py:typecheck`'s cached wheel.** SMA-535 §2's measurement stands and is not addressed here:
`uv run basedpyright` reads the installed copy of the stub, not the working tree's. This gate
reads the working tree directly, so it is unaffected — but the underlying staleness in
`py:typecheck` remains, and closing it is a separate issue.

---

## 9. Acceptance criteria mapping

| AC | Where |
| -- | -- |
| 1 — unstubbed `#[pyfunction]` fails | §6.1 row AC 1; §6.2 |
| 2 — removed registration fails | §6.1 row AC 2; §6.2 |
| 3 — deleted stub `def` fails | §6.1 row AC 3; §6.2 |
| 4 — each proven red by mutation, then restored | §6.2 |
| 5 — selected by a Rust **or** stub change; reachability | §5.3 `inputs`; §1.4, §1.5; §7 item 4 |
| 6 — signature comparison decided explicitly | §3 — decided: **full signatures** |
| 7 — `T=(…)` and CLAUDE.md's marker command | §7 items 1–2 |
