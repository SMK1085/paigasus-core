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
import re
import sys
from pathlib import Path

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


# --------------------------------------------------------------------------------------------
# Self-test and CLI
# --------------------------------------------------------------------------------------------


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

    print("self-test: OK" if rc == 0 else "self-test: FAILED", file=sys.stderr)
    return rc


def main():
    args = sys.argv[1:]
    try:
        if args == ["--self-test"]:
            return self_test()
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
