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
import ast
import glob
import re
import sys
import tomllib
from pathlib import Path
from typing import NamedTuple

# Repo root — three levels up from ci/pyo3-stub/check.py (ci/pyo3-stub -> ci -> repo root).
# Anchors SCAN_GLOB/STUB_GLOB so this file behaves the same run from any cwd, matching the
# convention `repo:*` gates use elsewhere in this repo.
REPO = Path(__file__).resolve().parents[2]

# --------------------------------------------------------------------------------------------
# Copied from ci/http-extractor/check.py — infrastructure signal and the lexical pre-pass
# --------------------------------------------------------------------------------------------

# Copied VERBATIM from ci/http-extractor/check.py:84-90 (SMA-587). rc-2 signal for the checker's
# own infrastructure failures, never for "the repo is wrong" — that is Refused, below. Copy
# rather than import — ci/ gates are standalone by design (SMA-600).
class InfraError(RuntimeError):
    """The inputs or environment are broken, NOT 'the tree regressed'.

    main() maps this to rc 2 so a broken checker aborts loudly instead of folding into a green.
    A gate that fails OPEN is worse than no gate: it converts "unguarded" into "believed guarded".
    """


# Copied VERBATIM from ci/http-extractor/check.py:105-186 (SMA-587). Do not re-derive: this
# function already handles raw strings, Rust's nesting block comments, and the lifetime-vs-char
# -literal trap, each of which cost a measurement there. Its two self-test rows are copied with
# it, below. Copy rather than import — ci/ gates are standalone by design (SMA-600).
#
# (The three regexes immediately below — _CHAR_LIT, _STR_OPEN, _IDENT — are strip_noise's own
# lexical dependencies, defined at check.py:96-102, just above the cited :105-186 range; they are
# included here verbatim too because strip_noise cannot run without them.)

# --------------------------------------------------------------------------------------------
# Lexical pre-pass
# --------------------------------------------------------------------------------------------

# A Rust char literal: exactly one char (or one escape) between single quotes. Anchored this way
# so a LIFETIME (`&'static str`, `<'a>`) is not mistaken for the opening of a char literal — the
# classic way a naive stripper swallows the rest of a file and takes the gate with it.
_CHAR_LIT = re.compile(r"'(?:\\.|[^'\\])'")
# A raw / byte string opener: `r"`, `br"`, `r#"`, `br##"`, `b"`.
_STR_OPEN = re.compile(r'(?:br|rb|b|r)?(#*)"')
_IDENT = re.compile(r"[A-Za-z0-9_]")


def strip_noise(text):
    """Blank out comments and string/char literals, PRESERVING length and newlines.

    Offsets and therefore line numbers are unchanged, so a violation is still reported at its real
    line. Two things depend on this pass:

      * a `(` or `)` inside a string or comment cannot unbalance the parameter-span walk, and
      * a comment INSIDE a parameter list (`// a Json<T> here would be a bug`) cannot be read as a
        violation. Both shapes are ordinary Rust and both would otherwise be wrong answers.
    """
    out = list(text)
    n = len(text)
    i = 0

    def blank(a, b):
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        two = text[i:i + 2]
        if two == "//":
            j = text.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
        elif two == "/*":
            # Rust block comments NEST, unlike C's.
            depth, j = 1, i + 2
            while j < n and depth:
                if text[j:j + 2] == "/*":
                    depth += 1
                    j += 2
                elif text[j:j + 2] == "*/":
                    depth -= 1
                    j += 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif text[i] == "'":
            m = _CHAR_LIT.match(text, i)
            if m:
                blank(i, m.end())
                i = m.end()
            else:
                i += 1  # a lifetime, not a literal
        elif text[i] == '"' or (
            text[i] in "brR" and (i == 0 or not _IDENT.match(text[i - 1]))
        ):
            m = _STR_OPEN.match(text, i)
            if m is None:
                i += 1
                continue
            hashes = m.group(1)
            close = '"' + hashes
            if hashes:
                j = text.find(close, m.end())
                j = n if j < 0 else j + len(close)
            else:
                j = m.end()
                while j < n:
                    if text[j] == "\\":
                        j += 2
                    elif text[j] == '"':
                        j += 1
                        break
                    else:
                        j += 1
            blank(i, j)
            i = j
        else:
            i += 1
    return "".join(out)


# --------------------------------------------------------------------------------------------
# Type mapping (§3)
# --------------------------------------------------------------------------------------------

# Signature = ((param_name, py_type), …), return_py_type — one entry per `#[pyfunction]`/stub
# `def` in set A/C respectively. A plain tuple, not a typing.NamedTuple: the shape is documented
# here rather than declared as a TypeAlias so this file needs no `typing` import for it alone.


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


# --------------------------------------------------------------------------------------------
# Set A — `#[pyfunction]` declarations
# --------------------------------------------------------------------------------------------

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
        # configuration-dependent, so one static answer is wrong. The visibility group mirrors
        # the `fn` visibility regex below (`pub(\s*\([^)]*\))?\s+`) — review finding 1: the
        # earlier `pub\s+(\([^)]*\)\s+)?` ordering required `pub` then whitespace BEFORE the
        # optional paren, so it matched bare `mod` and space-separated `pub mod` only. Idiomatic
        # `pub(crate) mod`, `pub(super) mod`, and `pub(in path) mod` fell through unmatched and
        # their nested #[pyfunction]s were silently extracted as if top-level — the exact
        # unmodelled-shape-treated-as-modelled failure this gate exists to prevent.
        if re.search(r"^\s*(pub(\s*\([^)]*\))?\s+)?mod\s+\w+\s*\{", text, re.M):
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
            # `\bwhere\b`, not `where\b` (review finding 2): a plain `where\b` has no boundary
            # BEFORE the literal, so it matches the substring "where" inside an identifier —
            # `-> Anywhere { }` used to stop at "Any" (`.+?` lazy-matches up to the first
            # "where"-then-boundary it finds, which sits mid-word at "Any|where"). `\bwhere\b`
            # requires a boundary on both sides, so it only fires on a standalone `where` token.
            ret = re.match(r"\s*->\s*(.+?)\s*(?:\bwhere\b|\{)", tail, re.S)
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


# --------------------------------------------------------------------------------------------
# Set B — `wrap_pyfunction!` registrations (§4.1)
# --------------------------------------------------------------------------------------------

# §4.1 — a PERMISSION list, not a refusal list. The ways to register a function under a name
# other than its own are open-ended (PyModule::add takes an arbitrary string, a submodule
# relocates the export), so anything not matched here is refused by construction.
_PERMITTED_MODULE_STATEMENTS = (
    re.compile(r"^m\.add_function\(\s*wrap_pyfunction!\(\s*(\w+)\s*,\s*m\s*\)\?\s*\)\?$"),
    re.compile(r"^m\.add_wrapped\(\s*wrap_pyfunction!\(\s*(\w+)\s*\)\s*\)\?$"),
)


def _pymodule_body(sources):
    """Return (ident, body_text). Exactly one #[pymodule] fn must exist across all sources.

    RULING (task-2 controller amendment, SMA-600): a first draft located the module `fn` with an
    UNBOUNDED `re.search` starting right after the `#[pymodule]` attribute. `re.search` scans
    forward without limit, so if anything unexpected sat between the attribute and its `fn` —
    another attribute, a stray earlier `fn` — that scan would silently bind a LATER, UNRELATED
    function's body as "the" module body, and set B would then be extracted from the wrong place
    while reporting success. That is the same silent-wrong-extraction class review caught as a
    Critical in Task 1 (the mod-visibility regex). The fix mirrors `rust_declarations`' own
    attribute-window walk (§4.3 item 1) exactly: only whitespace and
    `PERMITTED_INTERVENING_ATTRS` may sit between `#[pymodule]` and `fn`; anything else refuses by
    construction instead of being silently skipped past.
    """
    found = []
    for path, raw in sorted(sources.items()):
        text = strip_noise(raw)
        for m in re.finditer(r"#\[\s*pymodule[^\]]*\]", text):
            if m.group(0).strip() != "#[pymodule]":
                raise Refused(f"{path}: {m.group(0)!r} carries arguments — it may rename the module (§4.3)")

            # Bounded attribute-window walk, default-deny — see the ruling above.
            cursor = m.end()
            while True:
                am = re.match(r"\s*#\[\s*(\w+)", text[cursor:])
                if not am:
                    break
                if am.group(1) not in PERMITTED_INTERVENING_ATTRS:
                    raise Refused(f"{path}: attribute #[{am.group(1)}...] sits between #[pymodule] and the fn (§4.3)")
                depth, i = 0, cursor + am.start() + len(am.group(0)) - len(am.group(1)) - 2
                while i < len(text):
                    if text[i] == "[":
                        depth += 1
                    elif text[i] == "]":
                        depth -= 1
                        if depth == 0:
                            break
                    i += 1
                cursor = i + 1

            fn = re.match(r"\s*fn\s+(\w+)\s*\([^)]*\)[^{]*\{", text[cursor:])
            if not fn:
                raise Refused(f"{path}: no parsable `fn` follows #[pymodule] (§4.3)")
            open_at = cursor + fn.end() - 1
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
    # FIX (controller review round 1, finding 1) — guard the access itself so this ONE check
    # above is the only path to a wrong answer. Before this line existed alone, disabling just
    # the check above crashed `found = []` with an uncaught IndexError (a Python traceback, not
    # a graceful red) while a `found` of length 2 silently returned its first element with NO
    # exception at all — dropping every OTHER module's registrations, the exact "silently
    # dropping a name from set B" failure this whole gate exists to prevent. The fallback below
    # makes both of those consequences observable (a clean, empty-but-successful return) rather
    # than one of them crashing past the self-test's own `except Refused` and the other being
    # invisible.
    return found[0] if found else (None, "")


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


# --------------------------------------------------------------------------------------------
# Set C — the stub's `def`s (§4.3)
# --------------------------------------------------------------------------------------------


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


# --------------------------------------------------------------------------------------------
# Comparison, module identity, discovery, and the real-tree gate (SMA-600)
# --------------------------------------------------------------------------------------------

# Byte-identical to the Moon task's `inputs` entries — scheduling and scanning must not be able
# to drift apart, the same requirement repo:http-extractor-envelope and repo:workflow-credentials
# each state of themselves. Crate-generic rather than hard-coded: a gate whose whole job is to
# notice a NEW crate must not be scoped to today's one.
SCAN_GLOB = "rs/crates/bindings/*/src/**/*.rs"
STUB_GLOB = "rs/crates/bindings/*/*.pyi"

# §5.3 — a positive control. A gate that parses nothing reports clean, so `--check` proves it
# still sees a known-present symbol before believing a green.
SENTINEL = "sum_as_string"
SENTINEL_CRATE = "paigasus-py-bindings"

# §4.5 — ships EMPTY. (crate, marker) -> why. A waiver naming a shape no longer present is
# itself rc 1, so the table cannot rot into a silent blanket.
ALLOW_UNPARSED_SHAPE = {}

# rust: dict[str, str] display-path -> text, mirroring the `sources` shape every extractor above
# already takes. stub_path/stub_text/pyproject are all `| None` — a crate can legitimately have
# no stub (§5.1, itself a problem) or no pyproject.toml (§5.4, itself a Refused).
Crate = NamedTuple("Crate", [("name", str), ("rust", dict), ("stub_path", object), ("stub_text", object), ("pyproject", object)])


def stale_waivers(table=None, live=None):
    """Waiver keys naming a crate that is no longer present. §4.5.

    A waiver that has outlived the shape it excused would otherwise become a silent
    blanket, so a stale row is itself an error rather than a no-op.

    `live` is injectable so the self-test can assert this against a fixed set rather than
    the real tree — `analyze`/this function must stay pure and filesystem-free for Task 4's
    `--negative-control` to hold; only `check()` (via the default `live=None`) touches disk.
    """
    table = ALLOW_UNPARSED_SHAPE if table is None else table
    if live is None:
        live = {
            Path(p).relative_to(REPO).parts[3]
            for p in glob.glob(str(REPO / SCAN_GLOB), recursive=True)
        }
    return sorted(k for k in table if k[0] not in live)


def _maturin_module_name(pyproject_text, crate):
    """§5.4 — the module name maturin actually builds, read from [tool.maturin] module-name."""
    if pyproject_text is None:
        raise Refused(f"{crate}: no pyproject.toml, so [tool.maturin] module-name cannot be read (§5.4)")
    data = tomllib.loads(pyproject_text)
    name = data.get("tool", {}).get("maturin", {}).get("module-name")
    if not name:
        raise Refused(f"{crate}: [tool.maturin] module-name is absent (§5.4)")
    return name


def analyze(crates):
    """The pure core. `crates` is a list[Crate]; returns a list of problem strings ([] = clean).

    PURE over in-memory text — no filesystem access here, ever. Task 4's `--negative-control`
    and the whole self-test above depend on that: this function must behave identically whether
    its `Crate.rust`/`stub_text` came from `discover()` reading real files or from a self-test
    fixture built in memory.

    Every §4 refusal a sub-extractor raises arrives here as `Refused` and becomes ONE problem
    string — rc 1, because a commit introduced the shape and a commit can remove it (§5.1).
    `InfraError` is NOT caught here: it means the checker's own environment is broken and must
    propagate to `main()` as rc 2, never fold into "the repo has N problems".

    Every disagreement is reported, not just the first — a single run should surface the whole
    drift, not make the fixer re-run the gate once per symbol. §5.2 states this explicitly:
    a run that fixes one drift and hides the next behind it wastes a CI round.

    RESTRUCTURE (fix round 1, SMA-600 controller ruling): the first cut of this function ran
    the whole per-crate pipeline — decls -> regs -> stub -> identity -> A/B -> A/C — inside ONE
    try/except, so a `Refused` from an early stage (say, an unpermitted `#[pymodule]` statement)
    silently swallowed every LATER check for that crate, even ones with no dependency on the
    stage that refused (a module-identity mismatch, an unrelated A-vs-C signature drift). That
    is exactly what §5.2's "report every disagreement" rules out. Each extraction stage below is
    now attempted INDEPENDENTLY — a `Refused` from one becomes a problem string and leaves that
    stage's result as `None` (extraction failed), without aborting the others — and each
    comparison runs only when BOTH of its inputs are known (not `None`). `decls` stays the one
    exception: nothing else can be decided without knowing whether the crate is even PyO3-bearing
    and, if so, what it exports, so a `Refused` there still short-circuits the rest of this crate.
    """
    problems = []
    for crate in crates:
        try:
            decls = rust_declarations(crate.rust)
        except Refused as exc:
            # Nothing downstream can be attempted without `decls`: whether a stub is a leftover
            # (§5.1) and whether a stub is even expected both depend on knowing what — if
            # anything — the crate exports. Report the refusal and move to the next crate.
            problems.append(f"{crate.name}: {exc.message}")
            continue

        # A crate with no #[pyfunction] at all is simply not PyO3-bearing. A stub sitting
        # beside it anyway is a leftover (a crate that shed its last binding but not its
        # .pyi) and must be reported, not silently ignored.
        if not decls:
            if crate.stub_path:
                problems.append(f"{crate.name}: {crate.stub_path} exists but the crate declares no #[pyfunction] (§5.1)")
            continue

        if crate.stub_path is None:
            problems.append(f"{crate.name}: declares {len(decls)} #[pyfunction] but has no .pyi stub (§5.1)")
            continue

        # From here `decls` is known-good and non-empty, and a stub file exists. Every remaining
        # extraction is attempted independently — see the restructure note above — so each is
        # `None` (failed, already reported) or its real result, never a reason to skip a sibling
        # stage's own attempt.
        regs = None
        try:
            regs = rust_registrations(crate.rust)
        except Refused as exc:
            problems.append(f"{crate.name}: {exc.message}")

        stub = None
        try:
            stub = stub_definitions(crate.stub_path, crate.stub_text)
        except Refused as exc:
            problems.append(f"{crate.name}: {exc.message}")

        ident = None
        try:
            ident = pymodule_ident(crate.rust)
        except Refused as exc:
            problems.append(f"{crate.name}: {exc.message}")

        declared = None
        try:
            declared = _maturin_module_name(crate.pyproject, crate.name)
        except Refused as exc:
            problems.append(f"{crate.name}: {exc.message}")

        # §5.4 — bind the stub's FILENAME to the module it claims to describe. lib.rs's own
        # comment says the module name is provisional; without this check a rename of the
        # #[pymodule] fn (or of [tool.maturin] module-name) orphans the stub file while every
        # OTHER set still agrees on the function names inside it. `basename` needs only
        # `crate.stub_path`, already known non-None above, so it is read directly rather than
        # stage-guarded; the comparison itself runs only once BOTH `ident` and `declared` are
        # known — an unknown side must never be silently treated as "agrees".
        basename = crate.stub_path.rsplit("/", 1)[-1][:-4]
        if ident is not None and declared is not None and not (ident == declared == basename):
            problems.append(
                f"{crate.name}: module identity disagrees — #[pymodule] fn {ident!r}, "
                f"[tool.maturin] module-name {declared!r}, stub basename {basename!r} (§5.4)")

        # A vs B, on names — runs only once `regs` extracted cleanly (`is not None`, not merely
        # truthy: an empty-but-VALID registration set, e.g. AC2's "nothing registered", must
        # still be compared, not mistaken for an extraction failure). An unregistered
        # #[pyfunction] is an AttributeError at import time; a registration with no matching
        # declaration cannot exist under normal compilation, but is reported anyway rather than
        # assumed impossible.
        if regs is not None:
            for name in sorted(set(decls) - regs):
                problems.append(f"{crate.name}: `{name}` is declared #[pyfunction] but never registered — an AttributeError at import")
            for name in sorted(regs - set(decls)):
                problems.append(f"{crate.name}: `{name}` is registered but has no #[pyfunction] declaration")

        # A vs C, on FULL SIGNATURES (§3) — runs only once `stub` extracted cleanly (`is not
        # None`; AC3's "stub deleted every def" leaves a valid, empty `{}` that must still be
        # compared). Name, arity, parameter names IN ORDER, parameter types, and return type all
        # have to agree; a mere name match is not enough.
        if stub is not None:
            for name in sorted(set(decls) - set(stub)):
                problems.append(f"{crate.name}: `{name}` is exported but absent from {crate.stub_path} — invisible to type checkers")
            for name in sorted(set(stub) - set(decls)):
                problems.append(f"{crate.name}: `{name}` is in {crate.stub_path} but is not a #[pyfunction]")
            for name in sorted(set(decls) & set(stub)):
                if decls[name] != stub[name]:
                    problems.append(
                        f"{crate.name}: `{name}` signature drift — Rust says {decls[name]}, "
                        f"{crate.stub_path} says {stub[name]}")
    return problems


def discover():
    """Build the Crate list from the real tree. Raises InfraError on a §5.3 scope failure.

    This is the ONLY function in this file (besides `check()`, which calls it) that touches the
    filesystem for `analyze`'s purposes — `analyze` itself stays pure per its own docstring.
    """
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
    """Real-tree entry point. rc 0 clean, rc 1 the repo is wrong, rc 2 the checker is broken."""
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


# --------------------------------------------------------------------------------------------
# Self-test and CLI
# --------------------------------------------------------------------------------------------


def _expect_refused(label, sources):
    """Self-test helper: `sources` must make rust_declarations() raise Refused.

    Returns a FAIL message string if it did not (either it returned cleanly, or it raised
    something other than Refused), or None if the expectation held. A row that can only ever
    pass is worse than no row (review finding 3) — every case fed through this helper was
    confirmed to actually go red when its corresponding check was disabled in a scratch copy of
    this file; see task-1-report.md's fix-round-1 section for that transcript.
    """
    try:
        got = rust_declarations(sources)
    except Refused:
        return None
    return f"  FAIL [{label}] expected Refused, got {got!r}"


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
    # NOTE (task-1 deviation, flagged for controller review): the task-1 brief's own fixture
    # trace claims this is line 5, but the four lines above `#[pyfunction]` — padding, the two
    # `/* block\n   comment */` lines, and the attribute itself — put `#[pyfunction]` on line 4;
    # `fn a(...)` is line 5. `declaration_line`'s contract (its docstring, and the identical
    # `raw[:start].count("\n") + 1` computation `rust_declarations` uses for `where`) reports the
    # ATTRIBUTE's line, not the `fn`'s, so 4 is the value consistent with the rest of this file.
    # Verified independently with strip_noise + _find_attribute_sites before changing this constant.
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

    # --------------------------------------------------------------------------------------
    # §4.3 refusal coverage (review finding 3). The spec (§7.1) requires a row per refused
    # shape; their absence is what let review finding 1 (the mod-visibility regex) ship. Each
    # of these was independently confirmed to go red when its corresponding check was disabled
    # in a scratch copy of this file — see task-1-report.md's fix-round-1 section.
    # --------------------------------------------------------------------------------------
    refusal_cases = [
        # Inline `mod { ... }`, all four visibility spellings (finding 1's own bug: the old
        # regex refused only "bare" and "pub" — "pub(crate)" and "pub(in ...)" fell through).
        ("mod-bare", {"<t>": "mod inner {\n    #[pyfunction]\n    fn nested(s: &str) -> String {}\n}\n"}),
        ("mod-pub", {"<t>": "pub mod inner {\n    #[pyfunction]\n    fn nested(s: &str) -> String {}\n}\n"}),
        ("mod-pub-crate", {"<t>": "pub(crate) mod inner {\n    #[pyfunction]\n    fn nested(s: &str) -> String {}\n}\n"}),
        ("mod-pub-in", {"<t>": "pub(in crate::x) mod inner {\n    #[pyfunction]\n    fn nested(s: &str) -> String {}\n}\n"}),
        # macro_rules! anywhere in scope (§4.2) — invisible expansions, refused outright.
        ("macro_rules", {"<t>": "macro_rules! foo { () => {}; }\n"}),
        # #[pyfunction(...)] carrying arguments may rename/reshape the export.
        ("attr-args", {"<t>": '#[pyfunction(name = "x")]\nfn f(s: &str) -> String {}\n'}),
        # An un-permitted attribute in the window between #[pyfunction] and `fn` — #[pyo3(...)]
        # can rename the export, so the window walk is default-deny.
        ("attr-window", {"<t>": '#[pyfunction]\n#[pyo3(name = "x")]\nfn f(s: &str) -> String {}\n'}),
        # Refused item modifiers (§4.3 item 2) — PyO3 handling of each is not modelled.
        ("async-fn", {"<t>": "#[pyfunction]\nasync fn f(s: &str) -> String {}\n"}),
        ("unsafe-fn", {"<t>": "#[pyfunction]\nunsafe fn f(s: &str) -> String {}\n"}),
        ("const-fn", {"<t>": "#[pyfunction]\nconst fn f(s: &str) -> String {}\n"}),
        ("extern-fn", {"<t>": '#[pyfunction]\nextern "C" fn f(s: &str) -> String {}\n'}),
        # A raw identifier — PyO3 strips the `r#` prefix from the exported name.
        ("raw-ident", {"<t>": "#[pyfunction]\nfn r#type(s: &str) -> String {}\n"}),
        # The same name declared twice across two different `sources` entries.
        ("dup-name", {
            "<a>": "#[pyfunction]\nfn dup(s: &str) -> String {}\n",
            "<b>": "#[pyfunction]\nfn dup(s: &str) -> String {}\n",
        }),
        # A Rust type absent from RUST_TO_PY, general case.
        ("unmapped-type", {"<t>": "#[pyfunction]\nfn f(s: i32) -> String {}\n"}),
        # Python<'_> specifically (§3.1): PyO3 injects it and does not export it to Python, so
        # it must REFUSE, never map — a row here would make the gate demand a stub parameter
        # callers must never pass.
        ("python-injected", {"<t>": "#[pyfunction]\nfn f(py: Python<'_>, s: &str) -> String {}\n"}),
    ]
    for label, sources in refusal_cases:
        fail = _expect_refused(label, sources)
        if fail:
            print(fail, file=sys.stderr)
            rc = 1

    # The positive half of the attribute-window walk: a PERMITTED intervening attribute must
    # NOT raise, and the resulting signature must still be exactly right. Without this row the
    # window walk could refuse every single-attribute case above (a bug that "passes" every
    # refusal row) and still look correct.
    permitted = {"<t>": "#[pyfunction]\n#[allow(dead_code)]\nfn f(s: &str) -> String {}\n"}
    try:
        got = rust_declarations(permitted)
    except Refused as exc:
        print(f"  FAIL [attr-window-permitted] #[allow(...)] should not refuse: {exc.message}", file=sys.stderr)
        rc = 1
    else:
        if got != {"f": ((("s", "str"),), "str")}:
            print(f"  FAIL [attr-window-permitted] wrong signature: {got}", file=sys.stderr)
            rc = 1

    # Review finding 2 — the return-type regex must stop at a STANDALONE `where`, not any place
    # the substring appears inside an identifier. Both the buggy and fixed regex raise Refused
    # here (neither "Any" nor "Anywhere" is in RUST_TO_PY), so asserting Refused alone cannot
    # catch a regression of this specific bug — assert the un-truncated identifier by name.
    anywhere = {"<t>": "#[pyfunction]\nfn f(s: &str) -> Anywhere {}\n"}
    try:
        rust_declarations(anywhere)
        print("  FAIL [where-boundary-refuse] 'Anywhere' unexpectedly mapped, no Refused raised", file=sys.stderr)
        rc = 1
    except Refused as exc:
        if "'Anywhere'" not in exc.message:
            print(f"  FAIL [where-boundary-refuse] return type truncated before reaching the map check: {exc.message}", file=sys.stderr)
            rc = 1

    # ...and a genuine `where` clause must still cut the return type correctly — the fix must
    # not break the legitimate case it exists to preserve.
    where_ok = {"<t>": "#[pyfunction]\nfn f(s: &str) -> String where T: Clone {}\n"}
    try:
        got = rust_declarations(where_ok)
    except Refused as exc:
        print(f"  FAIL [where-boundary-ok] a real where clause misparsed: {exc.message}", file=sys.stderr)
        rc = 1
    else:
        if got != {"f": ((("s", "str"),), "str")}:
            print(f"  FAIL [where-boundary-ok] wrong signature: {got}", file=sys.stderr)
            rc = 1

    # --------------------------------------------------------------------------------------
    # Set B — §4.1 the #[pymodule] body is DEFAULT-DENY. Each row below is a channel that would
    # otherwise make all three sets agree over a module exporting something ELSE.
    # --------------------------------------------------------------------------------------
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

    # §4.1 — the same name registered twice in one module body (controller review round 1,
    # finding 2). Both statements are individually well-formed and individually permitted; only
    # the explicit `if m.group(1) in names: raise Refused(...)` guard catches the collision, since
    # a bare `set.add` would silently accept a repeat with no error at all.
    dup_reg = _MOD % '    m.add_function(wrap_pyfunction!(f, m)?)?;\n    m.add_wrapped(wrap_pyfunction!(f))?;'
    try:
        rust_registrations({"<x>": dup_reg})
    except Refused:
        pass
    else:
        print(f"  FAIL [module body] duplicate registration of the same name was accepted (§4.1)", file=sys.stderr)
        rc = 1

    # §4.3 — zero or two #[pymodule] fns is refused; set B would come from the wrong place.
    #
    # FIX (controller review round 1, finding 1): the original fixtures here were unfalsifiable
    # for their own stated reason. "zero" used a source with no #[pymodule] at all, which is
    # right, but on disabling only the count check `_pymodule_body` used to fall straight into
    # `found[0]` on an EMPTY list — an uncaught IndexError, not the graceful red this row's own
    # print claims. "two" built each module body from `_MOD % "    Ok(())"`, and `_MOD`'s own
    # template already appends a second, unterminated `Ok(())` with no separating `;` — so the
    # body text was the malformed statement `"Ok(()) Ok(())"`, which the UNRELATED default-deny
    # statement check also refuses. That masked the count check entirely: disabling ONLY it left
    # "two" still red, for the wrong reason, proving nothing about the count check itself.
    #
    # Fixed: "zero" now uses a syntactically ordinary (non-pymodule) fn, so nothing else in scope
    # can raise. "two" now uses TWO well-formed #[pymodule] fns, each with a real, individually
    # permitted registration under a DIFFERENT name (`f`, `g`) — a body the default-deny check has
    # no reason to touch. With the count check active both must still raise Refused. With it
    # disabled (see neuter-test table in task-2-report.md), `_pymodule_body`'s new `found[0] if
    # found else (None, "")` fallback (see its docstring) makes "zero" return an empty, unraising
    # set, and "two" silently return only `{"f"}` — the second module's `g` dropped with no error
    # at all, which is the exact realistic bug this pair of rows exists to catch.
    zero_src = "fn helper(m: &Bound<'_, PyModule>) -> PyResult<()> {\n    Ok(())\n}\n"
    two_src = (
        _MOD % "    m.add_function(wrap_pyfunction!(f, m)?)?;"
        + ("#[pymodule]\nfn m2(m: &Bound<'_, PyModule>) -> PyResult<()> {\n"
           "    m.add_function(wrap_pyfunction!(g, m)?)?;\n    Ok(())\n}\n")
    )
    for label, src in [("zero", zero_src), ("two", two_src)]:
        try:
            rust_registrations({"<x>": src})
        except Refused:
            pass
        else:
            print(f"  FAIL [pymodule] {label} #[pymodule] was accepted (§4.3)", file=sys.stderr)
            rc = 1

    # RULING (task-2 controller amendment, SMA-600) — the #[pymodule]-to-`fn` window walk must be
    # BOUNDED like rust_declarations' own (§4.3 item 1), not an unbounded re.search: an unexpected
    # intervening item (here, a renaming #[pyo3(...)]) must refuse rather than silently binding a
    # LATER, unrelated `fn` as the module body and reporting success over the wrong extraction.
    try:
        bad = rust_registrations({"<x>": '#[pymodule]\n#[pyo3(name = "x")]\nfn m(m: &Bound<\'_, PyModule>) -> PyResult<()> {\n    m.add_function(wrap_pyfunction!(f, m)?)?;\n    Ok(())\n}\n'})
    except Refused:
        pass
    else:
        print(f"  FAIL [pymodule-window] unexpected intervening item was accepted, got {bad!r} (ruling)", file=sys.stderr)
        rc = 1

    # ...and a PERMITTED intervening attribute (#[allow(dead_code)], same list rust_declarations
    # uses) must still parse, so the window walk is not simply refusing everything.
    try:
        got = rust_registrations({"<x>": "#[pymodule]\n#[allow(dead_code)]\nfn m(m: &Bound<'_, PyModule>) -> PyResult<()> {\n    m.add_function(wrap_pyfunction!(f, m)?)?;\n    Ok(())\n}\n"})
    except Refused as exc:
        print(f"  FAIL [pymodule-window] #[allow(...)] should not refuse: {exc.message}", file=sys.stderr)
        rc = 1
    else:
        if got != {"f"}:
            print(f"  FAIL [pymodule-window] permitted-window signature wrong: {got}", file=sys.stderr)
            rc = 1

    # --------------------------------------------------------------------------------------
    # Set C — §4.3 stub side. Every one of these is refused, because the Rust side has nothing
    # to compare against and a silent skip would leave the symbol unchecked.
    # --------------------------------------------------------------------------------------
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

    # --------------------------------------------------------------------------------------
    # Comparison, module identity, and the AC mutations (Task 3, SMA-600). `_fixture` returns a
    # one-crate list whose Rust and stub agree unless a kwarg overrides one.
    # --------------------------------------------------------------------------------------
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
    # cannot silently rot. `live` is passed explicitly (see stale_waivers' docstring): this must
    # not depend on what the real tree happens to contain right now.
    if not stale_waivers({("nonexistent-crate", "macro_rules"): "reason"}, live={"paigasus-py-bindings"}):
        print("  FAIL [waiver] a stale ALLOW_UNPARSED_SHAPE row was not reported (§4.5)", file=sys.stderr)
        rc = 1

    # ...and a waiver naming a crate that IS live must NOT be reported stale — otherwise every
    # legitimate waiver would also fail this check, and the row above could pass for the wrong
    # reason (stale_waivers reporting everything, live or not).
    if stale_waivers({("paigasus-py-bindings", "macro_rules"): "reason"}, live={"paigasus-py-bindings"}):
        print("  FAIL [waiver] a live crate's waiver was reported stale (§4.5)", file=sys.stderr)
        rc = 1

    # --------------------------------------------------------------------------------------
    # Fix round 1 (controller ruling, SMA-600): three `analyze()` branches disclosed as
    # untested in the task-3 report, plus the regression test for the try/except restructure
    # itself. Each below builds a fixture that lands ONLY in the named branch and asserts the
    # exact problem string it must produce.
    # --------------------------------------------------------------------------------------

    # `regs - decls` — a name registered via wrap_pyfunction! that has no matching
    # #[pyfunction] declaration. Not reachable from valid, compiling Rust (wrap_pyfunction!
    # requires the function to exist), but the branch exists and must still fire correctly if
    # it is ever reached — e.g. by a declaration this scanner separately refuses to see.
    reg_only = [Crate("c", {"lib.rs":
        "#[pyfunction]\nfn f(a: i64) -> String {}\n"
        "#[pymodule]\nfn mod_x(m: &Bound<'_, PyModule>) -> PyResult<()> {\n"
        "    m.add_function(wrap_pyfunction!(f, m)?)?;\n"
        "    m.add_function(wrap_pyfunction!(ghost, m)?)?;\n    Ok(())\n}\n"},
        "mod_x.pyi", "def f(a: int) -> str: ...\n", '[tool.maturin]\nmodule-name = "mod_x"\n')]
    problems = analyze(reg_only)
    if not any("`ghost` is registered but has no #[pyfunction] declaration" in p for p in problems):
        print(f"  FAIL [regs-decls] extra registration was not reported: {problems}", file=sys.stderr)
        rc = 1

    # `stub - decls` — a `def` in the stub with no matching #[pyfunction]. A hand-written stub
    # can drift ahead of the Rust (someone typed a def for a function not yet wired up), and
    # that must be visible, not silently accepted as "the stub knows more than the code".
    stub_only = [Crate("c", {"lib.rs":
        "#[pyfunction]\nfn f(a: i64) -> String {}\n"
        "#[pymodule]\nfn mod_x(m: &Bound<'_, PyModule>) -> PyResult<()> {\n"
        "    m.add_function(wrap_pyfunction!(f, m)?)?;\n    Ok(())\n}\n"},
        "mod_x.pyi", "def f(a: int) -> str: ...\ndef ghost(x: int) -> str: ...\n", '[tool.maturin]\nmodule-name = "mod_x"\n')]
    problems = analyze(stub_only)
    if not any("`ghost` is in" in p and "is not a #[pyfunction]" in p for p in problems):
        print(f"  FAIL [stub-decls] extra stub def was not reported: {problems}", file=sys.stderr)
        rc = 1

    # Leftover stub beside a crate with NO #[pyfunction] at all (§5.1's "not decls" arm) — a
    # crate that shed its last binding but kept its .pyi. `pyproject` is deliberately `None`
    # here: this branch `continue`s before ever needing it, so a valid pyproject is not a
    # precondition for reaching it.
    leftover_stub = [Crate("c", {"lib.rs": "fn helper() -> i64 { 1 }\n"}, "mod_x.pyi", "def ghost() -> None: ...\n", None)]
    problems = analyze(leftover_stub)
    if not any("exists but the crate declares no #[pyfunction]" in p for p in problems):
        print(f"  FAIL [leftover-stub] orphaned stub was not reported: {problems}", file=sys.stderr)
        rc = 1

    # Regression test for the try/except restructure (FINDING 1): one crate carrying a
    # registration-form refusal (unrelated to sets A/C) AND an independent module-identity
    # mismatch AND an independent A-vs-C signature drift must report ALL THREE — not just the
    # refusal. Before the restructure, the single enclosing try/except stopped at the first
    # Refused (from `rust_registrations`) and the identity mismatch and signature drift were
    # never reached at all. Asserting the exact count (3), not just "at least one of each",
    # is deliberate: it also catches a future regression that reintroduces a duplicate report
    # (e.g. from `pymodule_ident` and `rust_registrations` sharing `_pymodule_body`) without
    # anyone noticing the count creep.
    multi = [Crate("c", {"lib.rs":
        "#[pyfunction]\nfn f(a: i64) -> String {}\n"
        "#[pymodule]\nfn mod_x(m: &Bound<'_, PyModule>) -> PyResult<()> {\n"
        "    m.add_class::<Thing>()?;\n    Ok(())\n}\n"},
        "mod_x.pyi", "def f(a: int) -> int: ...\n", '[tool.maturin]\nmodule-name = "other"\n')]
    problems = analyze(multi)
    has_refusal = any("not a permitted registration form" in p for p in problems)
    has_identity = any("module identity disagrees" in p for p in problems)
    has_drift = any("signature drift" in p for p in problems)
    if not (has_refusal and has_identity and has_drift):
        print(f"  FAIL [restructure] a refusal hid an independent problem: {problems}", file=sys.stderr)
        rc = 1
    if len(problems) != 3:
        print(f"  FAIL [restructure] expected exactly 3 problems, got {len(problems)}: {problems}", file=sys.stderr)
        rc = 1

    print("self-test: OK" if rc == 0 else "self-test: FAILED", file=sys.stderr)
    return rc


def main():
    args = sys.argv[1:]
    try:
        if args == ["--self-test"]:
            return self_test()
        if args == ["--check"]:
            return check()
    except Refused as exc:
        # A refusal means the REPO is wrong (a §4 shape the scanner won't guess at), not that the
        # checker is broken — rc 1, never rc 2. Left uncaught this escapes as a raw traceback:
        # right exit code by accident (Python exits 1 on an uncaught exception too), unreadable
        # message on purpose. Ruling: task-1-brief.md controller amendment (SMA-600).
        print(f"REFUSED: {exc.message}", file=sys.stderr)
        return 1
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
