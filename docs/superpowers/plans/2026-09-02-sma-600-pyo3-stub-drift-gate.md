# PyO3 Stub-Drift Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a hand-written PyO3 `.pyi` that disagrees with the Rust it describes fail CI, in two arms — a source-text gate that reads types, and a runtime test that reads the compiled module's actual exported surface.

**Architecture:** Arm 1 is `ci/pyo3-stub/check.py`, a stdlib-only checker invoked by a new `repo:pyo3-stub-drift` Moon task in three modes (`--self-test`, `--negative-control`, `--check`). Its core is a pure function `analyze(crates) -> list[str]` over in-memory source text, so the same code path serves synthetic fixtures and the real tree. Arm 2 is a pytest inside the existing `paigasus-kernel-py:test` task, which already rebuilds the wheel and already keys on every path it reads, so it costs no registry wiring.

**Tech Stack:** Python 3.12 standard library only (`ast`, `re`, `glob`, `sys`, `pathlib`, `tomllib`, `shutil`, `tempfile`). Moon 2.5.3 `toolchain: 'system'`. pytest for arm 2. No new dependency in any workspace.

**Spec:** `docs/superpowers/specs/2026-09-02-sma-600-pyo3-stub-drift-gate-design.md` (rev 2). The plan argues from the spec; read both. Section references below (§3, §4.1, …) are to that file.

## Global Constraints

- **SPDX header** on every new source file, first line: `# SPDX-License-Identifier: Apache-2.0`.
- **Bash PATH.** The Bash tool's PATH lacks the proto-managed CLIs. Prefix any command using `moon`, `uv`, `cargo`, `nextest` or `buf` with:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- **Commits** are conventional with a workspace scope — `feat(ci): …`, `test(py): …`, `docs(ci): …`. Never `--no-verify`; the commit-msg hook runs commitlint and it must pass.
- **Branch** is `feature/sma-600-py-a-pyo3-stub-drift-gate-the-hand-written-pyi-can-disagree`, already checked out. Do not create another.
- **Exit-code contract for `check.py`** (§5.1): `rc 0` clean, `rc 1` the repo is wrong (this includes **every** refusal in §4 — a commit introduced it and a commit can remove it), `rc 2` the checker or its environment is broken (scan root moved, positive control failed, unexpected exception). Never collapse 1 and 2.
- **Never write `except Exception: pass`** or any construct that turns a parse failure into a skip. The gate's whole property is that an unreadable shape reds. §4 is not advisory.
- **`.github/CODEOWNERS` is Moon-generated** — never hand-edit it.
- **Python style:** this repo's `ci/` checkers are plain module-level functions with heavy explanatory comments, no classes beyond a single `InfraError(RuntimeError)`. Match `ci/http-extractor/check.py`. Comments explain *why*, and cite the SMA issue or the measurement.

---

### Task 1: Scanner foundation and set A (declarations)

**Files:**
- Create: `ci/pyo3-stub/check.py`
- Read for verbatim copy: `ci/http-extractor/check.py:84-90` (`InfraError`), `:105-186` (`strip_noise`), `:648-665` (two self-test rows)

**Interfaces:**
- Consumes: nothing (first task)
- Produces:
  - `class InfraError(RuntimeError)` — rc 2 signal
  - `strip_noise(text) -> str` — comments and string literals blanked, **byte offsets and line numbers preserved**
  - `Signature = tuple[tuple[tuple[str, str], ...], str]` — `((param_name, py_type), …), return_py_type`
  - `RUST_TO_PY: dict[str, str]`, `ALLOW_UNPARSED_SHAPE: dict[tuple[str, str], str]`, `PERMITTED_INTERVENING_ATTRS: frozenset[str]`
  - `map_rust_type(rust_type: str, where: str) -> str` — raises `Refused`
  - `class Refused(Exception)` with a `.message: str` — a §4 refusal, collected into the rc-1 problem list
  - `rust_declarations(sources: dict[str, str]) -> dict[str, Signature]` — set **A**; `sources` maps a display path to file text
  - `self_test() -> int`

- [ ] **Step 1: Create the file with its header, `InfraError`, and the copied `strip_noise`**

Copy `ci/http-extractor/check.py:84-90` and `:105-186` **verbatim** — do not re-derive, retype, or "improve" them. That stripper already handles raw strings (`r#"…"#`), Rust's *nesting* block comments, and the lifetime-vs-char-literal trap its own comment calls "the classic way a naive stripper swallows the rest of a file and takes the gate with it" (spec §4.3 item 6). Reuse is by **copy with attribution**, not import: `ci/` gates are deliberately standalone and a shared module would couple two gates' Moon schedules.

Add above the copied block:

```python
# Copied VERBATIM from ci/http-extractor/check.py:105-186 (SMA-587). Do not re-derive: this
# function already handles raw strings, Rust's nesting block comments, and the lifetime-vs-char
# -literal trap, each of which cost a measurement there. Its two self-test rows are copied with
# it, below. Copy rather than import — ci/ gates are standalone by design (SMA-600).
```

File header:

```python
# SPDX-License-Identifier: Apache-2.0
# SMA-600 — PyO3 stub-drift gate, arm 1 (source text).
#
# WHAT THIS GATES: the hand-written `.pyi` beside a PyO3 crate must agree with the Rust it
# describes. Three sets must match — `#[pyfunction]` declarations (A), `wrap_pyfunction!`
# registrations (B), and the stub's `def` names (C) — and A must match C on FULL SIGNATURES:
# name, arity, parameter names in order, parameter types, return type.
#
# WHAT THIS DOES NOT GATE: what PyO3's macros actually emit. This file reads source text. Arm 2
# (py/packages/paigasus-kernel/tests/test_stub_surface.py) asks the compiled module instead, and
# the two are blind to different things — see the design doc's §4.4.
#
# usage: check.py [--self-test | --negative-control | --check]
#   rc 0 clean · rc 1 the repo is wrong (every §4 refusal included) · rc 2 the checker is broken
```

- [ ] **Step 2: Write the failing self-test rows for `strip_noise` and the type map**

Add a `self_test()` that starts with the two rows copied from `ci/http-extractor/check.py:648-665`, **adapted** to call this file's own scanner rather than `violations_in`. The lifetime row must prove a `&'static str` does not derail the scan; the offset row must prove a reported line is the real line.

```python
def self_test():
    rc = 0

    # Copied from ci/http-extractor/check.py:655-665 (SMA-587), adapted to this gate's scanner.
    # `strip_noise` must not mistake a lifetime for a char literal and swallow the rest of the file.
    lifetimes = (
        "#[pyfunction]\n"
        "fn parts(s: &str) -> String { let _: &'static str = \"x\"; }\n"
    )
    got = sorted(rust_declarations({"<lifetimes>": lifetimes}))
    if got != ["parts"]:
        print(f"  FAIL [strip_noise] lifetimes derailed the scan: {got}", file=sys.stderr)
        rc = 1

    # ...and offsets must survive stripping, so a reported line is the real line.
    numbered = (
        "// padding\n"
        "/* block\n   comment */\n"
        "#[pyfunction]\n"
        "fn a(s: &str) -> String {}\n"
    )
    # Line 4, not 5: the fixture's literal newlines put #[pyfunction] on 4 and `fn a` on 5,
    # and both declaration_line and rust_declarations report the ATTRIBUTE's line. A collapsed
    # block comment would report < 4, which is the property this row exists to test.
    if declaration_line(numbered, "a") != 4:
        print(f"  FAIL [strip_noise] line numbers shifted after stripping", file=sys.stderr)
        rc = 1

    # A `#[pyfunction]` inside a doc comment or a raw string must mint NO phantom declaration.
    phantom = (
        '/// see #[pyfunction] in the docs\n'
        'const S: &str = r#"#[pyfunction] fn ghost() {}"#;\n'
        "#[pyfunction]\n"
        "fn real(s: &str) -> String {}\n"
    )
    got = sorted(rust_declarations({"<phantom>": phantom}))
    if got != ["real"]:
        print(f"  FAIL [phantom] comment/raw-string minted a declaration: {got}", file=sys.stderr)
        rc = 1

    print("self-test: OK" if rc == 0 else "self-test: FAILED", file=sys.stderr)
    return rc


def main():
    args = sys.argv[1:]
    try:
        if args == ["--self-test"]:
            return self_test()
    except InfraError as exc:
        print(f"INFRASTRUCTURE ERROR: {exc}", file=sys.stderr)
        return 2
    except OSError as exc:
        print(f"INFRASTRUCTURE ERROR: {exc}", file=sys.stderr)
        return 2
    print(f"usage: {Path(__file__).name} [--self-test | --negative-control | --check]", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
```

- [ ] **Step 3: Run it to verify it fails**

```bash
python3 ci/pyo3-stub/check.py --self-test
```
Expected: FAIL — `NameError: name 'rust_declarations' is not defined`, surfacing as rc 2 or a traceback. That is the point: the test exists before the implementation.

- [ ] **Step 4: Implement `RUST_TO_PY`, `Refused`, `map_rust_type`**

The map is closed (§3). The admission criterion in §3.1 is the reason it must stay closed: `Python<'_>` and `&Bound<'py, PyAny>` are **injected by PyO3 and not exported**, so mapping one would make the gate demand a stub parameter Python callers must never pass — enforcing a *wrong* stub. They are refusals, never map rows.

```python
class Refused(Exception):
    """A §4 shape the scanner will not guess at. Collected into the rc-1 problem list.

    NOT an InfraError: a commit introduced this shape and a commit can remove it, so the repo is
    wrong, not the tool (design §5.1). Never downgrade a Refused into a skip — that converts
    "unguarded" into "believed guarded", which is the one outcome worse than no gate.
    """

    def __init__(self, message):
        super().__init__(message)
        self.message = message


# Closed by design (§3). A type joins this map ONLY if PyO3 exports the parameter to Python AND
# the correspondence is 1:1. Everything else is a REFUSAL, never a row:
#   Python<'_>, &Bound<'py, PyAny>  injected by PyO3, NOT exported — a row here would make the
#                                   gate demand a stub parameter callers must never pass
#   Option<T>                       arity-preserving but shape-changing (T | None)
#   Vec<u8>, &[u8]                  more than one correct Python spelling (bytes/bytearray/...)
# Adding a row is a deliberate act that should stop a human, exactly as CONTRACTS_GENERATE_INPUTS
# records for its own list (SMA-600 §3.1).
RUST_TO_PY = {
    "&str": "str",
    "String": "str",
    "i64": "int",
    "u64": "int",
    "f64": "float",
    "bool": "bool",
    "()": "None",
}

_PYRESULT = re.compile(r"^PyResult<(.+)>$", re.S)


def map_rust_type(rust_type, where):
    """Normalize a Rust type to its Python spelling, or raise Refused.

    `PyResult<T>` unwraps to T: it is an error channel, not a value type, and PyO3 raises rather
    than returning it. So PyResult<()>, a bare (), and an absent return all normalize to "None".
    """
    t = " ".join(rust_type.split()) if rust_type else "()"
    m = _PYRESULT.match(t)
    if m:
        t = m.group(1).strip()
    if t not in RUST_TO_PY:
        raise Refused(
            f"{where}: Rust type {t!r} is not in RUST_TO_PY. Add a row ONLY if PyO3 exports this "
            f"parameter and the correspondence is 1:1 (see §3.1); otherwise it is a refusal."
        )
    return RUST_TO_PY[t]
```

- [ ] **Step 5: Implement `rust_declarations` and `declaration_line`**

The parse contract is §4.3 items 1–5, and every clause below implements one of them. **Paren-balanced across newlines** is not optional: `rs/rustfmt.toml` sets `max_width = 200` and `prn_build`'s signature is already 116 characters, so a sixth parameter makes rustfmt emit one parameter per line and a line-oriented regex would silently stop seeing it.

```python
# §4.3 item 1 — the attribute window between `#[pyfunction]` and the `fn` is DEFAULT-DENY. An
# intervening `#[pyo3(name = "x")]` renames the export, so skipping past it silently would put a
# wrong name in set A. Only these are permitted and ignored.
PERMITTED_INTERVENING_ATTRS = ("allow", "expect", "doc", "inline", "must_use")

_REFUSED_ITEM_MODIFIERS = ("async ", "unsafe ", "const ", "extern ")


def _find_attribute_sites(text):
    """Yield (index, attr_text) for every `#[pyfunction...]` in stripped text."""
    for m in re.finditer(r"#\[\s*pyfunction\b", text):
        depth, i = 0, m.start() + 1
        while i < len(text):
            if text[i] == "[":
                depth += 1
            elif text[i] == "]":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        else:
            raise Refused("unterminated #[pyfunction] attribute")
        yield m.start(), text[m.start():i + 1]


def rust_declarations(sources):
    """Set A. `sources` maps display path -> file text. Raises Refused on any §4 shape."""
    out = {}
    for path, raw in sorted(sources.items()):
        text = strip_noise(raw)

        # §4.2 — a macro_rules! that emits #[pyfunction] items is INVISIBLE here: we read the
        # definition, never the expansions. N real exports would be absent from A *and* B, and a
        # stub omitting them (the exact drift hunted) would pass green. Arm 2 covers this; arm 1
        # refuses outright.
        if re.search(r"\bmacro_rules!", text):
            raise Refused(f"{path}: macro_rules! is in scope — a source scanner cannot see what it emits (§4.2)")

        # §4.3 — an inline `mod { ... }` is unmodelled, and a #[cfg] on one makes the exported set
        # configuration-dependent, so one static answer is wrong.
        if re.search(r"^\s*(pub\s+(\([^)]*\)\s+)?)?mod\s+\w+\s*\{", text, re.M):
            raise Refused(f"{path}: an inline `mod {{ … }}` block is in scope — nesting is not modelled (§4.3)")

        for start, attr in _find_attribute_sites(text):
            line = raw[:start].count("\n") + 1
            where = f"{path}:{line}"

            # §4.3 — `#[pyfunction(...)]` with arguments may carry name= or signature=.
            if attr.strip() != "#[pyfunction]":
                raise Refused(f"{where}: {attr.strip()!r} carries arguments — it may rename or reshape the export (§4.3)")

            cursor = start + len(attr)
            # §4.3 item 1 — walk the attribute window, default-deny.
            while True:
                m = re.match(r"\s*#\[\s*(\w+)", text[cursor:])
                if not m:
                    break
                if m.group(1) not in PERMITTED_INTERVENING_ATTRS:
                    raise Refused(f"{where}: attribute #[{m.group(1)}...] sits between #[pyfunction] and the fn (§4.3)")
                depth, i = 0, cursor + m.start() + len(m.group(0)) - len(m.group(1)) - 2
                while i < len(text):
                    if text[i] == "[":
                        depth += 1
                    elif text[i] == "]":
                        depth -= 1
                        if depth == 0:
                            break
                    i += 1
                cursor = i + 1

            rest = text[cursor:]
            # §4.3 item 2 — pub/pub(crate)/pub(super) accepted and ignored; the rest refused.
            head = re.match(r"\s*(pub(\s*\([^)]*\))?\s+)?", rest)
            after_vis = rest[head.end():]
            for bad in _REFUSED_ITEM_MODIFIERS:
                if after_vis.startswith(bad):
                    raise Refused(f"{where}: `{bad.strip()} fn` is refused — PyO3 handling is not modelled (§4.3)")
            fn = re.match(r"fn\s+(r#)?(\w+)\s*\(", after_vis)
            if not fn:
                raise Refused(f"{where}: no parsable `fn NAME(` follows #[pyfunction] (§4.3)")
            if fn.group(1):
                raise Refused(f"{where}: a raw identifier `fn r#{fn.group(2)}` is refused — PyO3 strips the r# (§4.3)")
            name = fn.group(2)

            # §4.3 item 3 — paren-balanced across newlines. rustfmt's max_width is 200 and
            # prn_build is already 116 chars, so a sixth parameter splits the signature and a
            # line-oriented parse would stop seeing it.
            open_at = cursor + head.end() + fn.end() - 1
            depth, i = 0, open_at
            while i < len(text):
                if text[i] == "(":
                    depth += 1
                elif text[i] == ")":
                    depth -= 1
                    if depth == 0:
                        break
                i += 1
            else:
                raise Refused(f"{where}: unbalanced parameter list (§4.3)")
            params_src = text[open_at + 1:i]
            tail = text[i + 1:]
            ret = re.match(r"\s*->\s*(.+?)\s*(?:where\b|\{)", tail, re.S)
            ret_ty = ret.group(1).strip() if ret else "()"

            params = []
            for piece in _split_top_level(params_src):
                if not piece.strip():
                    continue
                if ":" not in piece:
                    raise Refused(f"{where}: parameter {piece.strip()!r} has no type annotation (§4.3)")
                pname, pty = piece.split(":", 1)
                params.append((pname.strip(), map_rust_type(pty, f"{where} parameter {pname.strip()!r}")))

            if name in out:
                raise Refused(f"{where}: `{name}` is declared more than once across the scanned files (§4.3)")
            out[name] = (tuple(params), map_rust_type(ret_ty, f"{where} return type"))
    return out


def _split_top_level(src):
    """Split a parameter list on commas that are not inside <>, () or []."""
    parts, depth, cur = [], 0, ""
    for ch in src:
        if ch in "<([":
            depth += 1
        elif ch in ">)]":
            depth -= 1
        if ch == "," and depth == 0:
            parts.append(cur)
            cur = ""
        else:
            cur += ch
    parts.append(cur)
    return parts


def declaration_line(raw, name):
    """1-based line of `name`'s #[pyfunction] attribute in raw text. Self-test helper."""
    text = strip_noise(raw)
    for start, _attr in _find_attribute_sites(text):
        m = re.search(r"fn\s+(\w+)", text[start:])
        if m and m.group(1) == name:
            return raw[:start].count("\n") + 1
    raise InfraError(f"no #[pyfunction] declaration named {name!r}")
```

- [ ] **Step 6: Run the self-test to verify it passes**

```bash
python3 ci/pyo3-stub/check.py --self-test
```
Expected: `self-test: OK`, rc 0. Verify with `echo $?`.

- [ ] **Step 7: Verify it extracts the real crate's 12 declarations**

```bash
python3 -c "
import sys; sys.path.insert(0, 'ci/pyo3-stub')
from check import rust_declarations
from pathlib import Path
p = Path('rs/crates/bindings/paigasus-py-bindings/src/lib.rs')
d = rust_declarations({str(p): p.read_text()})
print(len(d)); print(d['prn_build'])
"
```
Expected: `12`, then `((('service', 'str'), ('region', 'str'), ('org', 'str'), ('resource_type', 'str'), ('resource_id', 'str')), 'str')`.

- [ ] **Step 8: Commit**

```bash
git add ci/pyo3-stub/check.py
git commit -m "feat(ci): add the PyO3 stub-drift scanner foundation and set A (SMA-600)"
```

---

### Task 2: Set B (default-deny registrations) and set C (the stub)

**Files:**
- Modify: `ci/pyo3-stub/check.py`

**Interfaces:**
- Consumes: `Refused`, `strip_noise`, `InfraError` from Task 1
- Produces:
  - `rust_registrations(sources: dict[str, str]) -> set[str]` — set **B**
  - `stub_definitions(path: str, text: str) -> dict[str, Signature]` — set **C**
  - `pymodule_ident(sources: dict[str, str]) -> str`

- [ ] **Step 1: Write the failing self-test rows**

Append to `self_test()`. Every row here is a **measured bypass** from §4.1 — each yields `A == B == C` while the module exports something else, which is why the module body is a permission list rather than a refusal list.

```python
    # §4.1 — the #[pymodule] body is DEFAULT-DENY. Each row below is a channel that would
    # otherwise make all three sets agree over a module exporting something ELSE.
    _MOD = "#[pymodule]\nfn m(m: &Bound<'_, PyModule>) -> PyResult<()> {\n%s\n    Ok(())\n}\n"
    for label, body in [
        ("alias",       '    m.add("alias", wrap_pyfunction!(f, m)?)?;'),
        ("submodule",   "    let c = PyModule::new(py, \"c\")?;\n    m.add_submodule(&c)?;"),
        ("add_class",   "    m.add_class::<Thing>()?;"),
        ("qualified",   "    m.add_function(wrap_pyfunction!(a::b, m)?)?;"),
    ]:
        try:
            rust_registrations({"<x>": _MOD % body})
        except Refused:
            pass
        else:
            print(f"  FAIL [module body] {label} was accepted; it must be refused (§4.1)", file=sys.stderr)
            rc = 1

    # ...and the two PERMITTED forms must still parse.
    ok = rust_registrations({"<x>": _MOD % "    m.add_function(wrap_pyfunction!(f, m)?)?;\n    m.add_wrapped(wrap_pyfunction!(g))?;"})
    if ok != {"f", "g"}:
        print(f"  FAIL [module body] permitted forms did not parse: {ok}", file=sys.stderr)
        rc = 1

    # §4.3 — zero or two #[pymodule] fns is refused; set B would come from the wrong place.
    for label, src in [("zero", "fn m() {}"), ("two", _MOD % "    Ok(())" + _MOD % "    Ok(())")]:
        try:
            rust_registrations({"<x>": src})
        except Refused:
            pass
        else:
            print(f"  FAIL [pymodule] {label} #[pymodule] was accepted (§4.3)", file=sys.stderr)
            rc = 1

    # §4.3 stub side — every one of these is refused, because the Rust side has nothing to
    # compare against and a silent skip would leave the symbol unchecked.
    for label, stub in [
        ("varargs",    "def f(*args) -> str: ...\n"),
        ("kwargs",     "def f(**kw) -> str: ...\n"),
        ("default",    "def f(a: int = 1) -> str: ...\n"),
        ("kwonly",     "def f(*, a: int) -> str: ...\n"),
        ("posonly",    "def f(a: int, /) -> str: ...\n"),
        ("decorated",  "@overload\ndef f(a: int) -> str: ...\n"),
        ("async",      "async def f(a: int) -> str: ...\n"),
        ("no_ann",     "def f(a) -> str: ...\n"),
        ("no_return",  "def f(a: int): ...\n"),
        ("class",      "class C: ...\n"),
    ]:
        try:
            stub_definitions("<stub>", stub)
        except Refused:
            pass
        else:
            print(f"  FAIL [stub] {label} was accepted; it must be refused (§4.3)", file=sys.stderr)
            rc = 1

    # ...and the permitted top-level nodes must parse.
    got = stub_definitions("<stub>", '"""doc."""\nimport typing\nfrom typing import Any\ndef f(a: int) -> str: ...\n')
    if got != {"f": ((("a", "int"),), "str")}:
        print(f"  FAIL [stub] permitted nodes did not parse: {got}", file=sys.stderr)
        rc = 1
```

- [ ] **Step 2: Run to verify it fails**

```bash
python3 ci/pyo3-stub/check.py --self-test
```
Expected: FAIL — `NameError: name 'rust_registrations' is not defined`.

- [ ] **Step 3: Implement `rust_registrations` and `pymodule_ident`**

```python
# §4.1 — a PERMISSION list, not a refusal list. The ways to register a function under a name
# other than its own are open-ended (PyModule::add takes an arbitrary string, a submodule
# relocates the export), so anything not matched here is refused by construction.
_PERMITTED_MODULE_STATEMENTS = (
    re.compile(r"^m\.add_function\(\s*wrap_pyfunction!\(\s*(\w+)\s*,\s*m\s*\)\?\s*\)\?$"),
    re.compile(r"^m\.add_wrapped\(\s*wrap_pyfunction!\(\s*(\w+)\s*\)\s*\)\?$"),
)


def _pymodule_body(sources):
    """Return (ident, body_text). Exactly one #[pymodule] fn must exist across all sources."""
    found = []
    for path, raw in sorted(sources.items()):
        text = strip_noise(raw)
        for m in re.finditer(r"#\[\s*pymodule[^\]]*\]", text):
            fn = re.search(r"fn\s+(\w+)\s*\([^)]*\)[^{]*\{", text[m.end():])
            if not fn:
                raise Refused(f"{path}: no parsable `fn` follows #[pymodule] (§4.3)")
            if m.group(0).strip() != "#[pymodule]":
                raise Refused(f"{path}: {m.group(0)!r} carries arguments — it may rename the module (§4.3)")
            open_at = m.end() + fn.end() - 1
            depth, i = 0, open_at
            while i < len(text):
                if text[i] == "{":
                    depth += 1
                elif text[i] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                i += 1
            else:
                raise Refused(f"{path}: unbalanced #[pymodule] body (§4.3)")
            found.append((fn.group(1), text[open_at + 1:i]))
    if len(found) != 1:
        raise Refused(f"expected exactly one #[pymodule], found {len(found)} — set B would come from the wrong place (§4.3)")
    return found[0]


def pymodule_ident(sources):
    return _pymodule_body(sources)[0]


def rust_registrations(sources):
    """Set B, default-deny over the single #[pymodule] body (§4.1)."""
    _ident, body = _pymodule_body(sources)
    names = set()
    for stmt in body.split(";"):
        s = " ".join(stmt.split())
        if not s or s == "Ok(())":
            continue
        for pat in _PERMITTED_MODULE_STATEMENTS:
            m = pat.match(s)
            if m:
                if m.group(1) in names:
                    raise Refused(f"`{m.group(1)}` is registered twice (§4.1)")
                names.add(m.group(1))
                break
        else:
            raise Refused(
                f"statement {s!r} in the #[pymodule] body is not a permitted registration form. "
                f"Only `m.add_function(wrap_pyfunction!(NAME, m)?)?` and "
                f"`m.add_wrapped(wrap_pyfunction!(NAME))?` are allowed — anything else can export "
                f"under a different name or from a submodule (§4.1)."
            )
    return names
```

- [ ] **Step 4: Implement `stub_definitions`**

```python
def stub_definitions(path, text):
    """Set C, via the standard library's own parser. Raises Refused on any §4.3 stub shape."""
    try:
        tree = ast.parse(text, filename=path)
    except SyntaxError as exc:
        raise Refused(f"{path}: the stub does not parse: {exc}") from exc

    out = {}
    for node in tree.body:
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            continue
        if isinstance(node, ast.Expr) and isinstance(node.value, ast.Constant) and isinstance(node.value.value, str):
            continue  # module docstring
        if isinstance(node, ast.AsyncFunctionDef):
            raise Refused(f"{path}:{node.lineno}: `async def {node.name}` — PyO3 exports no coroutines here (§4.3)")
        if not isinstance(node, ast.FunctionDef):
            raise Refused(f"{path}:{node.lineno}: top-level {type(node).__name__} is not a `def`, an import or the docstring (§4.3)")
        a = node.args
        if node.decorator_list:
            raise Refused(f"{path}:{node.lineno}: `{node.name}` is decorated — @overload and friends are not modelled (§4.3)")
        if a.vararg or a.kwarg or a.kwonlyargs or a.posonlyargs or a.defaults or a.kw_defaults:
            raise Refused(f"{path}:{node.lineno}: `{node.name}` uses *args/**kwargs/defaults/pos- or kw-only params (§4.3)")
        if node.returns is None:
            raise Refused(f"{path}:{node.lineno}: `{node.name}` has no return annotation (§4.3)")
        params = []
        for arg in a.args:
            if arg.annotation is None:
                raise Refused(f"{path}:{node.lineno}: `{node.name}` parameter {arg.arg!r} has no annotation (§4.3)")
            params.append((arg.arg, ast.unparse(arg.annotation)))
        if node.name in out:
            raise Refused(f"{path}:{node.lineno}: `{node.name}` is defined twice (§4.3)")
        out[node.name] = (tuple(params), ast.unparse(node.returns))
    return out
```

Add `import ast` to the imports.

- [ ] **Step 5: Run the self-test to verify it passes**

```bash
python3 ci/pyo3-stub/check.py --self-test; echo "rc=$?"
```
Expected: `self-test: OK`, `rc=0`.

- [ ] **Step 6: Verify against the real crate**

```bash
python3 -c "
import sys; sys.path.insert(0, 'ci/pyo3-stub')
from check import rust_registrations, stub_definitions, pymodule_ident
from pathlib import Path
b = Path('rs/crates/bindings/paigasus-py-bindings/src/lib.rs')
s = Path('rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi')
print('B:', len(rust_registrations({str(b): b.read_text()})))
print('C:', len(stub_definitions(str(s), s.read_text())))
print('ident:', pymodule_ident({str(b): b.read_text()}))
"
```
Expected: `B: 12`, `C: 12`, `ident: paigasus_py_bindings`.

- [ ] **Step 7: Commit**

```bash
git add ci/pyo3-stub/check.py
git commit -m "feat(ci): add default-deny registration and stub extraction (SMA-600)"
```

---

### Task 3: Comparison, module identity, positive controls, `--check`

**Files:**
- Modify: `ci/pyo3-stub/check.py`

**Interfaces:**
- Consumes: everything from Tasks 1–2
- Produces:
  - `Crate` — a `NamedTuple(name, rust: dict[str,str], stub_path: str|None, stub_text: str|None, pyproject: str|None)`
  - `analyze(crates: list[Crate]) -> list[str]` — the pure core; `[]` means clean
  - `discover() -> list[Crate]` — reads the real tree; raises `InfraError` on §5.3 conditions
  - `check() -> int`
  - `SCAN_GLOB`, `STUB_GLOB`, `SENTINEL`

- [ ] **Step 1: Write the failing self-test rows for the comparison and identity**

```python
    # The three AC mutations, plus the signature-level drift §3 exists to catch. `_fixture`
    # returns a one-crate list whose Rust and stub agree unless a kwarg overrides one.
    def _fixture(rust_extra="", regs="    m.add_function(wrap_pyfunction!(f, m)?)?;", stub="def f(a: int) -> str: ...\n", pyproject='[tool.maturin]\nmodule-name = "mod_x"\n'):
        rust = (
            "#[pyfunction]\nfn f(a: i64) -> String {}\n" + rust_extra +
            "#[pymodule]\nfn mod_x(m: &Bound<'_, PyModule>) -> PyResult<()> {\n" + regs + "\n    Ok(())\n}\n"
        )
        return [Crate("c", {"lib.rs": rust}, "mod_x.pyi", stub, pyproject)]

    if analyze(_fixture()) != []:
        print(f"  FAIL [baseline] a clean fixture reported problems: {analyze(_fixture())}", file=sys.stderr)
        rc = 1

    cases = [
        ("AC1 unstubbed pyfunction", _fixture(
            rust_extra="#[pyfunction]\nfn g(a: i64) -> String {}\n",
            regs="    m.add_function(wrap_pyfunction!(f, m)?)?;\n    m.add_function(wrap_pyfunction!(g, m)?)?;")),
        ("AC2 missing registration", _fixture(regs="    ")),
        ("AC3 deleted stub def", _fixture(stub="")),
        ("type drift", _fixture(stub="def f(a: float) -> str: ...\n")),
        ("param rename", _fixture(stub="def f(b: int) -> str: ...\n")),
        ("arity drift", _fixture(stub="def f(a: int, b: int) -> str: ...\n")),
        ("return drift", _fixture(stub="def f(a: int) -> int: ...\n")),
        ("identity drift", _fixture(pyproject='[tool.maturin]\nmodule-name = "other"\n')),
    ]
    for label, crates in cases:
        if analyze(crates) == []:
            print(f"  FAIL [{label}] reported clean; it must red", file=sys.stderr)
            rc = 1

    # Param ORDER, not just the name set — a transposition breaks every keyword call site while
    # the stub keeps describing the old order (§3).
    swapped = [Crate("c", {"lib.rs":
        "#[pyfunction]\nfn f(a: i64, b: &str) -> String {}\n"
        "#[pymodule]\nfn mod_x(m: &Bound<'_, PyModule>) -> PyResult<()> {\n"
        "    m.add_function(wrap_pyfunction!(f, m)?)?;\n    Ok(())\n}\n"},
        "mod_x.pyi", "def f(b: str, a: int) -> str: ...\n", '[tool.maturin]\nmodule-name = "mod_x"\n')]
    if analyze(swapped) == []:
        print("  FAIL [param order] a transposition reported clean (§3)", file=sys.stderr)
        rc = 1

    # §3.1 — an injected Python<'_> must REFUSE, never resolve. Mapping it would make the gate
    # demand a stub parameter callers must never pass.
    injected = _fixture(rust_extra="#[pyfunction]\nfn g(py: Python<'_>) -> String {}\n",
                        regs="    m.add_function(wrap_pyfunction!(f, m)?)?;\n    m.add_function(wrap_pyfunction!(g, m)?)?;")
    problems = analyze(injected)
    if not any("Python<'_>" in p or "RUST_TO_PY" in p for p in problems):
        print(f"  FAIL [injected] Python<'_> did not refuse: {problems}", file=sys.stderr)
        rc = 1

    # §3 — PyResult<()>, bare (), and an absent return all normalize to None, consistently.
    for ret, decl in [("PyResult<()>", "-> PyResult<()> "), ("()", "-> () "), ("absent", "")]:
        crates = [Crate("c", {"lib.rs":
            f"#[pyfunction]\nfn f(a: i64) {decl}{{}}\n"
            "#[pymodule]\nfn mod_x(m: &Bound<'_, PyModule>) -> PyResult<()> {\n"
            "    m.add_function(wrap_pyfunction!(f, m)?)?;\n    Ok(())\n}\n"},
            "mod_x.pyi", "def f(a: int) -> None: ...\n", '[tool.maturin]\nmodule-name = "mod_x"\n')]
        if analyze(crates) != []:
            print(f"  FAIL [return {ret}] did not normalize to None: {analyze(crates)}", file=sys.stderr)
            rc = 1

    # §4.3 item 3 — layout-blind. rustfmt splits a long signature one parameter per line.
    split = [Crate("c", {"lib.rs":
        "#[pyfunction]\nfn f(\n    a: i64,\n    b: &str,\n) -> String {}\n"
        "#[pymodule]\nfn mod_x(m: &Bound<'_, PyModule>) -> PyResult<()> {\n"
        "    m.add_function(wrap_pyfunction!(f, m)?)?;\n    Ok(())\n}\n"},
        "mod_x.pyi", "def f(a: int, b: str) -> str: ...\n", '[tool.maturin]\nmodule-name = "mod_x"\n')]
    if analyze(split) != []:
        print(f"  FAIL [layout] a rustfmt-split signature did not parse: {analyze(split)}", file=sys.stderr)
        rc = 1

    # §4.3 item 1 — #[allow(...)] between the attribute and the fn is PERMITTED.
    allowed = _fixture(rust_extra="")
    allowed[0].rust["lib.rs"] = allowed[0].rust["lib.rs"].replace(
        "#[pyfunction]\nfn f", "#[pyfunction]\n#[allow(clippy::needless_pass_by_value)]\nfn f")
    if analyze(allowed) != []:
        print(f"  FAIL [attr window] #[allow] was refused; it is permitted: {analyze(allowed)}", file=sys.stderr)
        rc = 1

    # §5.1 — a PyO3-bearing crate with no stub, and two stubs in one crate, are both rc 1.
    if analyze([Crate("c", {"lib.rs": _fixture()[0].rust["lib.rs"]}, None, None, '[tool.maturin]\nmodule-name = "mod_x"\n')]) == []:
        print("  FAIL [no stub] a PyO3-bearing crate with no .pyi reported clean (§5.1)", file=sys.stderr)
        rc = 1

    # §4.5 — a waiver naming a shape that is not present is itself an error, so the table
    # cannot silently rot.
    if not stale_waivers({("nonexistent/path.rs", "macro_rules"): "reason"}):
        print("  FAIL [waiver] a stale ALLOW_UNPARSED_SHAPE row was not reported (§4.5)", file=sys.stderr)
        rc = 1
```

- [ ] **Step 2: Run to verify it fails**

```bash
python3 ci/pyo3-stub/check.py --self-test
```
Expected: FAIL — `NameError: name 'Crate' is not defined`.

- [ ] **Step 3: Implement `Crate`, `analyze`, `stale_waivers`**

```python
# Byte-identical to the Moon task's `inputs` entries — scheduling and scanning must not be able
# to drift apart, the same requirement repo:http-extractor-envelope and repo:workflow-credentials
# each state of themselves. Crate-generic rather than hard-coded: a gate whose whole job is to
# notice a NEW crate must not be scoped to today's one (moon.yml:706-709; ci_targets.py:228-231).
SCAN_GLOB = "rs/crates/bindings/*/src/**/*.rs"
STUB_GLOB = "rs/crates/bindings/*/*.pyi"

# §5.3 — a positive control. A gate that parses nothing reports clean, so `--check` proves it
# still sees a known-present symbol before believing a green.
SENTINEL = "sum_as_string"
SENTINEL_CRATE = "paigasus-py-bindings"

# §4.5 — ships EMPTY. (crate, marker) -> why. A waiver naming a shape no longer present is
# itself rc 1, so the table cannot rot into a silent blanket.
ALLOW_UNPARSED_SHAPE = {}

Crate = NamedTuple("Crate", [("name", str), ("rust", dict), ("stub_path", object), ("stub_text", object), ("pyproject", object)])


def stale_waivers(table=None):
    """Return the waiver keys naming a crate that no longer exists. §4.5."""
    table = ALLOW_UNPARSED_SHAPE if table is None else table
    live = {p.split("/")[3] for p in glob.glob(str(REPO / SCAN_GLOB), recursive=True)} if REPO.exists() else set()
    return [k for k in table if k[0] not in live and "/" in str(k[0])] or [k for k in table if k[0] not in live]


def _maturin_module_name(pyproject_text, crate):
    if pyproject_text is None:
        raise Refused(f"{crate}: no pyproject.toml, so [tool.maturin] module-name cannot be read (§5.4)")
    data = tomllib.loads(pyproject_text)
    name = data.get("tool", {}).get("maturin", {}).get("module-name")
    if not name:
        raise Refused(f"{crate}: [tool.maturin] module-name is absent (§5.4)")
    return name


def analyze(crates):
    """The pure core. Returns a list of problem strings; [] means clean.

    Every §4 refusal arrives here as a Refused and becomes a problem string — rc 1, because a
    commit introduced the shape and a commit can remove it (§5.1). InfraError is NOT caught here:
    it means the environment is broken and must reach main() as rc 2.
    """
    problems = []
    for crate in crates:
        try:
            decls = rust_declarations(crate.rust)

            # A crate with no #[pyfunction] is simply not PyO3-bearing; a stub beside it is a
            # leftover and must be reported rather than ignored.
            if not decls:
                if crate.stub_path:
                    problems.append(f"{crate.name}: {crate.stub_path} exists but the crate declares no #[pyfunction] (§5.1)")
                continue

            if crate.stub_path is None:
                problems.append(f"{crate.name}: declares {len(decls)} #[pyfunction] but has no .pyi stub (§5.1)")
                continue

            regs = rust_registrations(crate.rust)
            stub = stub_definitions(crate.stub_path, crate.stub_text)

            # §5.4 — bind the stub's filename to the module it describes. lib.rs's own comment
            # says the module name is provisional; without this a rename orphans the stub while
            # every set still agrees.
            ident = pymodule_ident(crate.rust)
            declared = _maturin_module_name(crate.pyproject, crate.name)
            basename = crate.stub_path.rsplit("/", 1)[-1][:-4]
            if not (ident == declared == basename):
                problems.append(
                    f"{crate.name}: module identity disagrees — #[pymodule] fn {ident!r}, "
                    f"[tool.maturin] module-name {declared!r}, stub basename {basename!r} (§5.4)")

            # A vs B, on names.
            for name in sorted(set(decls) - regs):
                problems.append(f"{crate.name}: `{name}` is declared #[pyfunction] but never registered — an AttributeError at import")
            for name in sorted(regs - set(decls)):
                problems.append(f"{crate.name}: `{name}` is registered but has no #[pyfunction] declaration")

            # A vs C, on FULL SIGNATURES (§3).
            for name in sorted(set(decls) - set(stub)):
                problems.append(f"{crate.name}: `{name}` is exported but absent from {crate.stub_path} — invisible to type checkers")
            for name in sorted(set(stub) - set(decls)):
                problems.append(f"{crate.name}: `{name}` is in {crate.stub_path} but is not a #[pyfunction]")
            for name in sorted(set(decls) & set(stub)):
                if decls[name] != stub[name]:
                    problems.append(
                        f"{crate.name}: `{name}` signature drift — Rust says {decls[name]}, "
                        f"{crate.stub_path} says {stub[name]}")
        except Refused as exc:
            problems.append(f"{crate.name}: {exc.message}")
    return problems
```

Add imports: `import glob`, `import tomllib`, `from typing import NamedTuple`, and `REPO = Path(__file__).resolve().parents[2]`.

- [ ] **Step 4: Implement `discover` and `check`**

```python
def discover():
    """Build the Crate list from the real tree. Raises InfraError on a §5.3 scope failure."""
    rust_files = sorted(glob.glob(str(REPO / SCAN_GLOB), recursive=True))
    stub_files = sorted(glob.glob(str(REPO / STUB_GLOB), recursive=True))
    if not rust_files:
        raise InfraError(f"{SCAN_GLOB} matched no file — the scan root moved and the gate is scanning nothing")
    if not stub_files:
        raise InfraError(f"{STUB_GLOB} matched no file — the scan root moved and the gate is scanning nothing")

    by_crate = {}
    for p in rust_files:
        crate = Path(p).relative_to(REPO).parts[3]
        by_crate.setdefault(crate, {})[str(Path(p).relative_to(REPO))] = Path(p).read_text()

    stubs = {}
    for p in stub_files:
        crate = Path(p).relative_to(REPO).parts[3]
        stubs.setdefault(crate, []).append(p)

    crates = []
    for name, rust in sorted(by_crate.items()):
        found = stubs.get(name, [])
        if len(found) > 1:
            raise InfraError(f"{name}: {len(found)} .pyi files match {STUB_GLOB}; exactly one is expected (§5.1)")
        stub_path = str(Path(found[0]).relative_to(REPO)) if found else None
        stub_text = Path(found[0]).read_text() if found else None
        pp = REPO / "rs/crates/bindings" / name / "pyproject.toml"
        crates.append(Crate(name, rust, stub_path, stub_text, pp.read_text() if pp.exists() else None))
    return crates


def check():
    crates = discover()

    # §5.3 — real-tree positive controls. Each is rc 2: a gate that parses nothing reports
    # clean, so "I found no problems" must be backed by "I found the thing I know is there".
    names = {c.name for c in crates}
    if SENTINEL_CRATE not in names:
        raise InfraError(f"{SENTINEL_CRATE} is not among the discovered crates {sorted(names)} — the scan root moved")
    sentinel_crate = next(c for c in crates if c.name == SENTINEL_CRATE)
    a = rust_declarations(sentinel_crate.rust)
    b = rust_registrations(sentinel_crate.rust)
    c = stub_definitions(sentinel_crate.stub_path, sentinel_crate.stub_text)
    if not (a and b and c):
        raise InfraError(f"one of the three sets is empty (A={len(a)} B={len(b)} C={len(c)}) — two empty sets compare equal")
    if not (SENTINEL in a and SENTINEL in b and SENTINEL in c):
        raise InfraError(f"the sentinel {SENTINEL!r} is missing from A/B/C — the extractors are not reading the real crate")

    stale = stale_waivers()
    problems = [f"ALLOW_UNPARSED_SHAPE{k!r} names a crate that no longer exists (§4.5)" for k in stale]
    problems += analyze(crates)

    print(f"pyo3-stub: crates: {' '.join(sorted(names))}", file=sys.stderr)
    if problems:
        for p in problems:
            print(f"  FAIL {p}", file=sys.stderr)
        print(f"pyo3-stub: {len(problems)} problem(s)", file=sys.stderr)
        return 1
    print(f"pyo3-stub: {len(a)} function(s) agree across declarations, registrations and the stub", file=sys.stderr)
    return 0
```

Wire `--check` into `main()` alongside `--self-test`.

- [ ] **Step 5: Run the self-test and the check**

```bash
python3 ci/pyo3-stub/check.py --self-test; echo "self-test rc=$?"
python3 ci/pyo3-stub/check.py --check;     echo "check rc=$?"
```
Expected: `self-test: OK` rc=0; then `pyo3-stub: 12 function(s) agree…` rc=0.

- [ ] **Step 6: Commit**

```bash
git add ci/pyo3-stub/check.py
git commit -m "feat(ci): compare signatures, bind module identity, add positive controls (SMA-600)"
```

---

### Task 4: `--negative-control` against the real tree

**Files:**
- Modify: `ci/pyo3-stub/check.py`

**Interfaces:**
- Consumes: `discover`, `analyze`, `Crate` from Task 3
- Produces: `negative_control() -> int`

**Why this task exists (spec §7.2):** the first draft of the design omitted a negative control for a bad reason — it conflated `repo:workflow-credentials`'s `run.sh` (which exists solely to translate rc 3 into rc 1) with a *checker mode*. A self-test asserts against a fixture table; a negative control asserts against the **real tree**. CLAUDE.md's SMA-601 entry records the measured lesson that the bare mode alone is a gate that can lie. This also discharges AC 4 permanently, rather than as a transcript that decays the moment someone edits the checker.

- [ ] **Step 1: Implement `negative_control`**

```python
def negative_control():
    """Mutate a COPY of the real crate three ways and assert each reds. §7.2 / AC 4.

    Operates on discover()'s output with the text swapped in memory — no tempdir is needed
    because analyze() is pure over source text, and mutating on disk risks leaving the real tree
    dirty if the process dies mid-run.
    """
    crates = discover()
    base = next(c for c in crates if c.name == SENTINEL_CRATE)
    lib = next(p for p in base.rust if p.endswith("/lib.rs"))
    failures = 0

    def _expect_red(label, crate):
        nonlocal failures
        if analyze([crate]) == []:
            print(f"  FAIL negative control [{label}] reported CLEAN against a mutated tree", file=sys.stderr)
            failures += 1

    # AC 1 — a #[pyfunction] added and registered, absent from the stub.
    rust = dict(base.rust)
    rust[lib] = base.rust[lib].replace(
        "#[pymodule]",
        "#[pyfunction]\nfn negative_control_probe(s: &str) -> String {}\n\n#[pymodule]", 1
    ).replace(
        "    Ok(())",
        "    m.add_function(wrap_pyfunction!(negative_control_probe, m)?)?;\n    Ok(())", 1)
    _expect_red("AC1 unstubbed pyfunction", base._replace(rust=rust))

    # AC 2 — a registration removed while the declaration and the stub stay.
    rust = dict(base.rust)
    rust[lib] = base.rust[lib].replace(
        f"    m.add_function(wrap_pyfunction!({SENTINEL}, m)?)?;\n", "", 1)
    _expect_red("AC2 missing registration", base._replace(rust=rust))

    # AC 3 — a def deleted from the stub while the Rust keeps the function.
    kept = "\n".join(l for l in base.stub_text.splitlines() if not l.startswith(f"def {SENTINEL}(")) + "\n"
    _expect_red("AC3 deleted stub def", base._replace(stub_text=kept))

    # ...and the UNMUTATED crate must still be clean, or the three rows above prove nothing.
    if analyze([base]) != []:
        print(f"  FAIL negative control: the unmutated crate is not clean: {analyze([base])}", file=sys.stderr)
        failures += 1

    if failures:
        print(f"pyo3-stub negative control: {failures} row(s) failed", file=sys.stderr)
        return 1
    print("== pyo3-stub negative control passed ==", file=sys.stderr)
    return 0
```

Wire `--negative-control` into `main()`.

- [ ] **Step 2: Run all three modes**

```bash
python3 ci/pyo3-stub/check.py --self-test;         echo "rc=$?"
python3 ci/pyo3-stub/check.py --negative-control;  echo "rc=$?"
python3 ci/pyo3-stub/check.py --check;             echo "rc=$?"
```
Expected: all three rc=0, with `== pyo3-stub negative control passed ==` from the second.

- [ ] **Step 3: Prove the control can actually fail (AC 4's by-hand run)**

Temporarily break the comparison, confirm the control reds, then restore. Capture the output for the PR body.

```bash
cp ci/pyo3-stub/check.py /tmp/check.py.bak
python3 - <<'PY'
import re, pathlib
p = pathlib.Path("ci/pyo3-stub/check.py"); s = p.read_text()
s = s.replace("            for name in sorted(set(decls) - set(stub)):", "            for name in []:", 1)
p.write_text(s)
PY
python3 ci/pyo3-stub/check.py --negative-control; echo "MUTATED rc=$? (expect 1)"
cp /tmp/check.py.bak ci/pyo3-stub/check.py && rm /tmp/check.py.bak
python3 ci/pyo3-stub/check.py --negative-control; echo "RESTORED rc=$? (expect 0)"
git diff --stat ci/pyo3-stub/check.py
```
Expected: `MUTATED rc=1` naming the AC1 row, `RESTORED rc=0`, and an empty `git diff --stat`. **If the mutated run reports rc=0, stop** — the control is not asserting and the gate would ship unable to bite.

- [ ] **Step 4: Commit**

```bash
git add ci/pyo3-stub/check.py
git commit -m "feat(ci): add the real-tree negative control for the stub gate (SMA-600)"
```

---

### Task 5: Arm 2 — the runtime surface test

**Files:**
- Create: `py/packages/paigasus-kernel/tests/test_stub_surface.py`

**Interfaces:**
- Consumes: nothing from arm 1 — deliberately independent, and it must not import `ci/pyo3-stub/check.py` (that would couple a py test to a `ci/` gate's schedule)
- Produces: nothing consumed by later tasks

**Why this arm exists (spec §4.4, §6):** a `macro_rules!` that emits `#[pyfunction]` items is invisible to any source scanner, and a PyO3 upgrade can change what the macros emit with no source diff. Arm 1 refuses both; arm 2 asks the compiled module what it actually exports. It lives here because `paigasus-kernel-py:test` already runs `touch` + `uv sync --reinstall-package` before pytest, so the module is built from the working tree, and the task already lists the `.pyi`, both Rust sources and every manifest among its `inputs` — the check costs no registry wiring and no new build.

**Two measured constraints (spec §1.3), both load-bearing:**
1. The live module exports **13** names, not 12 — maturin wraps the extension in a package, so the inner module object is itself an attribute. Filter to callables and exclude `ModuleType`, or this test reds on a correct tree.
2. Read the **working-tree** `.pyi`, not the installed `__init__.pyi`. maturin relocates the stub on install, and the working tree is the unambiguous source.

- [ ] **Step 1: Write the test**

```python
# SPDX-License-Identifier: Apache-2.0
"""Arm 2 of the PyO3 stub-drift gate (SMA-600): the COMPILED module vs the hand-written stub.

repo:pyo3-stub-drift reads source text, so it cannot see what PyO3's macros actually emit — a
`macro_rules!`-generated `#[pyfunction]`, a registration under an alias, a submodule, or a change
in macro behaviour across a PyO3 upgrade. It refuses those shapes rather than guessing. This test
is the other half: it asks the imported module what it really exports. Neither arm can replace the
other, because `__text_signature__` carries parameter names but no TYPES (design doc §4.4, §6).

This lives in paigasus-kernel-py:test because that task already forces a wheel rebuild
(`uv sync --reinstall-package`) and already keys on the stub, both Rust sources and every
manifest — so the check costs no new build and no registry wiring.
"""

import ast
import inspect
import types
from pathlib import Path

import paigasus_py_bindings

# From this file: tests -> paigasus-kernel -> packages -> py -> repo root == parents[4], the same
# derivation test_parity.py uses. The WORKING-TREE stub, deliberately: maturin relocates the file
# to <package>/__init__.pyi on install, and the working tree is the unambiguous source (§1.3).
STUB = (
    Path(__file__).resolve().parents[4]
    / "rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi"
)


def _exported_callables():
    """The module's real exported surface.

    MEASURED (§1.3): `dir()` yields 13 names, not 12 — maturin wraps the extension in a package,
    so the inner module object is itself visible as an attribute. Filtering to callables while
    excluding ModuleType is what makes this 12; without it the test reds on a correct tree.
    """
    out = {}
    for name in dir(paigasus_py_bindings):
        if name.startswith("_"):
            continue
        value = getattr(paigasus_py_bindings, name)
        if isinstance(value, types.ModuleType) or not callable(value):
            continue
        out[name] = value
    return out


def _stub_defs():
    tree = ast.parse(STUB.read_text(), filename=str(STUB))
    return {n.name: [a.arg for a in n.args.args] for n in tree.body if isinstance(n, ast.FunctionDef)}


def test_stub_exists():
    assert STUB.is_file(), f"{STUB} is missing — the stub moved and this test would assert nothing"


def test_exported_names_match_the_stub():
    exported = set(_exported_callables())
    stubbed = set(_stub_defs())
    assert exported, "the module exported no callables — the import or the filter is wrong"
    assert exported == stubbed, (
        f"exported-but-unstubbed: {sorted(exported - stubbed)}; "
        f"stubbed-but-not-exported: {sorted(stubbed - exported)}"
    )


def test_parameter_names_and_order_match_the_stub():
    stub = _stub_defs()
    drift = {}
    for name, fn in sorted(_exported_callables().items()):
        if name not in stub:
            continue  # reported by the test above; do not double-report
        live = list(inspect.signature(fn).parameters)
        if live != stub[name]:
            drift[name] = (live, stub[name])
    assert not drift, f"parameter names/order drift (live vs stub): {drift}"
```

- [ ] **Step 2: Run it and verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd py && uv run pytest packages/paigasus-kernel/tests/test_stub_surface.py -v; cd ..
```
Expected: 3 passed. If `test_exported_names_match_the_stub` reports a 13th name, the `ModuleType` filter is wrong — fix the filter, not the assertion.

- [ ] **Step 3: Prove it can fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cp rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi /tmp/stub.bak
grep -v '^def sum_as_string' /tmp/stub.bak > rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi
cd py && uv run pytest packages/paigasus-kernel/tests/test_stub_surface.py -q; echo "MUTATED rc=$? (expect non-zero)"; cd ..
cp /tmp/stub.bak rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi && rm /tmp/stub.bak
git diff --stat rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi
```
Expected: the mutated run fails naming `sum_as_string` as exported-but-unstubbed; the restore leaves an empty `git diff --stat`.

- [ ] **Step 4: Commit**

```bash
git add py/packages/paigasus-kernel/tests/test_stub_surface.py
git commit -m "test(py): assert the compiled PyO3 surface matches the stub (SMA-600)"
```

---

### Task 6: Moon task, registry wiring, README

**Files:**
- Create: `ci/pyo3-stub/README.md`
- Modify: `moon.yml` (append the task; fix the stale count at `:202-203`)
- Modify: `.github/workflows/ci.yml:234` (the `T=(…)` array)
- Modify: `CLAUDE.md:125-133` (the marker-delimited command)
- Modify: `ci/affected-graph/ci_targets.py` (`SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS`)
- Modify: `ci/actionlint/run.sh:2093` (the stale count)

**Interfaces:**
- Consumes: `ci/pyo3-stub/check.py`'s three modes from Tasks 1–4
- Produces: the `repo:pyo3-stub-drift` Moon target

**Omitting any one of these edits reds `repo:affected-smoke`, not this gate** — that is the design working. Per spec §1.6, **no** `T_AFFECTED_SMOKE_REQUIRED_INPUTS` entry and **no** `REQUIRED_REPO_TASKS` entry are needed: `ci/**/*` already floors reachability, and `check_gate_inputs`/`check_self_invocation` already floor the task's existence.

- [ ] **Step 1: Append the Moon task to `moon.yml`**

Append after the `workflow-credentials` block (currently the file's last task, ending at `:801`), keeping the two-space task-key indentation:

```yaml

  pyo3-stub-drift:
    description: 'Assert the hand-written PyO3 stubs agree with the Rust they describe (SMA-600).'
    # WHY THIS EXISTS — paigasus_py_bindings.pyi is hand-written and is basedpyright's ENTIRE
    # view of the PyO3 surface, so a signature change with no stub update leaves the stub
    # self-consistent and the type checker passes. Measured on SMA-600's branch: editing the
    # stub selects four tasks and editing lib.rs selects eleven, and not one of those fifteen
    # compares the two.
    #
    # WHY NOT py:typecheck INPUTS (SMA-535's filed fix) — basedpyright reads the stub and never
    # the Rust, so no input list changes what it reads; and `uv run basedpyright` is measured
    # in-repo to serve a CACHED wheel, so even the stub it reads is the installed copy.
    #
    # ARM 2 is py/packages/paigasus-kernel/tests/test_stub_surface.py, which asks the COMPILED
    # module what it exports. This arm reads source text and refuses shapes it cannot model
    # (macro_rules!, aliased registrations, submodules); that one sees through all of them but
    # carries no types. Neither replaces the other.
    #
    # The first two globs are IDENTICAL to check.py's SCAN_GLOB and STUB_GLOB on purpose:
    # scheduling and scanning must not be able to drift apart. Crate-generic, not scoped to
    # today's one crate — a gate whose job is to notice a NEW crate must not be blind to it.
    #
    # `--self-test` and `--negative-control` run FIRST and in the SAME block: a gate that cannot
    # report red is worse than no gate. `set -euo pipefail` is REQUIRED — Moon does not enable
    # errexit for `script:` blocks and takes the block's status from its LAST command, so without
    # it a failing control is masked by the passing real run. These four lines are pinned by
    # SELF_SCHEDULED_GATES.
    script: |
      set -euo pipefail
      python3 ci/pyo3-stub/check.py --self-test
      python3 ci/pyo3-stub/check.py --negative-control
      python3 ci/pyo3-stub/check.py --check
    toolchain: 'system'
    inputs:
      - 'rs/crates/bindings/*/src/**/*.rs'
      - 'rs/crates/bindings/*/*.pyi'
      # The manifests are load-bearing, not padding. A [features] toggle in Cargo.toml can
      # cfg-gate a #[pyfunction] (which the checker must then REFUSE), pyproject.toml carries the
      # [tool.maturin] module-name the identity assertion reads, and rs/Cargo.lock pins the pyo3
      # version whose macros decide what is emitted. Same rule as SMA-560's for the FFI wrappers.
      - 'rs/crates/bindings/*/Cargo.toml'
      - 'rs/crates/bindings/*/pyproject.toml'
      - 'rs/Cargo.lock'
      - 'ci/pyo3-stub/**/*'
```

- [ ] **Step 2: Add the target to `ci.yml` and `CLAUDE.md`, keeping their ORDER identical**

`check_docs` compares the two lists for **ordered** equality and reports the "first divergence at position {i}" (`ci_targets.py:1142-1152`), so the target must land in the same position in both. Append it last in each.

`.github/workflows/ci.yml:234` — must stay a **single-line** bash array (SMA-541):

```bash
python3 - <<'PY'
import pathlib
p = pathlib.Path(".github/workflows/ci.yml"); s = p.read_text()
old = " :publish-metadata :version-lockstep :workflow-credentials)"
new = " :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift)"
assert s.count(old) == 1, s.count(old)
p.write_text(s.replace(old, new))
PY
grep -n "T=(" .github/workflows/ci.yml
```

`CLAUDE.md:125-133` — inside the markers only. **Do not add a second copy of either marker anywhere in the file, including inside backticks in prose**: the count becomes 2 and `repo:affected-smoke` reds (SMA-541).

```bash
python3 - <<'PY'
import pathlib
p = pathlib.Path("CLAUDE.md"); s = p.read_text()
old = "  :publish-metadata :version-lockstep :workflow-credentials --base origin/main"
new = "  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift\n  --base origin/main"
assert s.count(old) == 1, s.count(old)
p.write_text(s.replace(old, new))
PY
sed -n '125,135p' CLAUDE.md
grep -c "ci-targets:begin" CLAUDE.md   # MUST print 1
```

- [ ] **Step 3: Add both `ci_targets.py` registry entries**

`SELF_SCHEDULED_GATES` — insert before the closing `}` of the dict, after the `"actionlint"` entry. Whole-line matched, which is load-bearing here in the usual direction: all three invocations share the prefix `python3 ci/pyo3-stub/check.py`, so under a substring test deleting the real run would leave the pin green.

```python
    # SMA-600 — repo:pyo3-stub-drift. FOUR lines, like version-lockstep and workflow-credentials:
    # a --self-test (a synthetic fixture table) AND a --negative-control (the REAL tree, three
    # mutations), because neither proves the other can report red. `set -euo pipefail` is exactly
    # as load-bearing as any invocation — Moon takes a `script:` block's status from its LAST
    # command, so without it a failing self-test or control is masked by the passing real run.
    # Whole-line matched: `python3 ci/pyo3-stub/check.py` is a strict PREFIX of all three, so a
    # substring test would report the gate fully wired after any one had been deleted.
    "pyo3-stub-drift": (
        "set -euo pipefail",
        "python3 ci/pyo3-stub/check.py --self-test",
        "python3 ci/pyo3-stub/check.py --negative-control",
        "python3 ci/pyo3-stub/check.py --check",
    ),
```

`SELF_TASK_EXPECTED_GLOBS` — `check_gate_inputs` compares **globs sorted, then literal files sorted** (`ci_targets.py:1381-1396`), *not* the authored order, so the tuple must read in that order:

```python
    # SMA-600. Five globs then one literal, in check_gate_inputs' comparison order (globs sorted,
    # then files sorted) — NOT the authored order in moon.yml. The first two are deliberately
    # identical to check.py's SCAN_GLOB and STUB_GLOB, which moon.yml's own comment on this task
    # records: scheduling and scanning must not be able to drift apart, so narrowing either one
    # silently reopens the gap the gate exists to close. The three manifest entries are what make
    # a cfg-gated #[pyfunction], a module-name rename, or a pyo3 bump re-key the gate.
    "pyo3-stub-drift": (
        "ci/pyo3-stub/**/*",
        "rs/crates/bindings/*/*.pyi",
        "rs/crates/bindings/*/Cargo.toml",
        "rs/crates/bindings/*/pyproject.toml",
        "rs/crates/bindings/*/src/**/*.rs",
        "rs/Cargo.lock",
    ),
```

- [ ] **Step 4: Correct the two stale entry counts**

Both describe `T_AFFECTED_SMOKE_REQUIRED_INPUTS`, which holds **23** entries (verify, don't trust). A wrong count is what makes a floor look re-baselinable.

```bash
python3 -c "
import re,pathlib
s=pathlib.Path('ci/actionlint/run.sh').read_text()
m=re.search(r'T_AFFECTED_SMOKE_REQUIRED_INPUTS=\((.*?)\n\)',s,re.S)
print('entries:',len([l for l in m.group(1).split(chr(10)) if l.strip() and not l.strip().startswith('#')]))"
sed -i '' 's|asserts containment over a 21-entry array and floors it at|asserts containment over a 23-entry array and floors it at|' moon.yml
sed -i '' 's|# CONTAINMENT, not equality: the list is twenty entries and legitimately grows every time a|# CONTAINMENT, not equality: the list is twenty-three entries and legitimately grows every time a|' ci/actionlint/run.sh
grep -n "23-entry array" moon.yml; grep -n "twenty-three entries" ci/actionlint/run.sh
```

- [ ] **Step 5: Write `ci/pyo3-stub/README.md`**

Follow `ci/http-extractor/README.md`'s sections: **What it gates** (the three sets and full-signature comparison), **What it does NOT gate** (arm 2's territory, and N1–N6 from spec §9), **How it reads Rust without a Rust parser** (the §4.3 parse contract, and that `strip_noise` is a verbatim copy), **Fail-closed properties** (§4.1's default-deny, the rc 2 positive controls), **The `ALLOW_UNPARSED_SHAPE` table** (ships empty; a stale row is itself an error), **Limitations** — which must carry, as its own numbered entry:

> **L1.** Nothing pins this checker's internals. `SELF_SCHEDULED_GATES` proves the three invocations run; no analogue of `WORKFLOW_CREDENTIALS_SH_CALL_SITES` reaches inside a Python checker, so the `--self-test` fixture table could be emptied and the gate would stay green. This is precedent-consistent — equally true of `ci/http-extractor` and `ci/error-registry` — and is recorded, not fixed.

- [ ] **Step 6: Verify the gate runs under Moon and the registries agree**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:pyo3-stub-drift --force
moon run repo:affected-smoke --force
moon run repo:input-liveness --force
moon run repo:actionlint --force
```
Expected: all four pass. `repo:affected-smoke` is the one that would catch a missing registry edit — if it reds, read which assertion (`check_docs`, `check_self_invocation`, `check_gate_inputs`) and fix the corresponding edit from Steps 2–3 rather than loosening the assertion.

If `repo:affected-smoke` fails in under 3 seconds, **capture the full output before re-running** — that is the intermittent `proto-shim … Permission denied` abort CLAUDE.md documents, and a re-run destroys the evidence. Grep the captured output for `proto-shim`: if present, the failure is not about the affected graph and `moon run repo:affected-smoke --force` alone will pass.

- [ ] **Step 7: Prove the gate is selected by both a Rust and a stub edit (AC 5)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cat > /tmp/q.py <<'PY'
import json, subprocess
d = json.loads(subprocess.run(["moon","query","tasks","--affected"],capture_output=True,text=True).stdout)
t = [f"{p}:{k}" for p, ts in d.get("tasks", {}).items() for k in ts]
print("pyo3-stub-drift selected:", "repo:pyo3-stub-drift" in t, f"({len(t)} tasks)")
PY
printf '\n' >> rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi && python3 /tmp/q.py
git checkout rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi
printf '\n' >> rs/crates/bindings/paigasus-py-bindings/src/lib.rs && python3 /tmp/q.py
git checkout rs/crates/bindings/paigasus-py-bindings/src/lib.rs
git status --porcelain   # MUST be empty
```
Expected: `selected: True` on both. Parse the JSON `tasks` map as above — never `grep -o '"target"'`, which counts scheduled upstreams as selections (CLAUDE.md).

Note `moon query` can take over two minutes cold; run it with a generous timeout rather than assuming it hung.

- [ ] **Step 8: Run the full CI graph as CI does**

Per CLAUDE.md, per-project tasks do not run the repo-level gates. Run the marker-delimited command from `CLAUDE.md` (now including `:pyo3-stub-drift`) against `origin/main`.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift \
  --base origin/main --include-relations
```

The three `repo:release-parity*` gates abort **INCONCLUSIVE at rc 2** inside an agent session, because `proto` emits NDJSON on stdout when it detects `AI_AGENT`/`CLAUDECODE`/`CLAUDE_CODE_ENTRYPOINT`. Prefix with `env -u AI_AGENT -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT` if they abort — an inconclusive abort otherwise reads as a pass.

- [ ] **Step 9: Commit**

```bash
git add moon.yml .github/workflows/ci.yml CLAUDE.md ci/affected-graph/ci_targets.py ci/actionlint/run.sh ci/pyo3-stub/README.md
git commit -m "feat(ci): wire repo:pyo3-stub-drift into the gate registries (SMA-600)"
```

---

## Self-review

**Spec coverage.** §3 type map → T1 S4; §3.1 admission criterion → T1 S4 + T3 S1 (`Python<'_>` row); §3.2 no divergence table → recorded in the spec, nothing to build; §4.1 default-deny → T2; §4.2 `macro_rules!` → T1 S5 + T5; §4.3 parse contract items 1–6 → T1 S5, T2 S4, T3 S1 (layout + attr-window rows); §4.4 → T5; §4.5 waiver table → T3; §5.1 multi-crate + rc contract → T3; §5.2 extractors → T1–T3; §5.3 positive controls → T3 S4; §5.4 module identity → T3 S3; §5.5 moon.yml → T6 S1; §6 arm 2 → T5; §7.1 self-test → T1–T3; §7.2 negative control → T4; §7.3 by-hand AC 4 → T4 S3 and T5 S3; §8 four registry edits → T6 S2–S3; §9 non-goals → T6 S5 (README); §1.6 stale counts → T6 S4. **No gaps.**

**AC coverage.** AC 1–3 → T3 S1 rows, T4 S1 rows, T5 S1. AC 4 → T4 S3 + T5 S3 (by hand, transcripts for the PR) and T4 (automated, permanent). AC 5 → T6 S1 inputs + T6 S7 measurement. AC 6 → decided in the spec, implemented T1 S4. AC 7 → T6 S2.

**Type consistency.** `Signature` is `((name, py_type), …), return_py_type` in T1 and is consumed unchanged by `analyze` in T3. `Crate` is defined once in T3 and used by `discover`, `analyze` and `negative_control`. `Refused` carries `.message`, read only in `analyze`'s handler. `rust_declarations`/`rust_registrations`/`pymodule_ident` all take the same `sources: dict[str, str]` shape. `stub_definitions(path, text)` takes two arguments everywhere it is called.

**One deliberate ordering note.** T5 (arm 2) has no dependency on T1–T4 and could run first; it is placed after arm 1 so the two by-hand AC 4 proofs (T4 S3, T5 S3) land close together, and so T6's full-graph run is last.
