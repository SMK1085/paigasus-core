# SPDX-License-Identifier: Apache-2.0
# SMA-587 — single-site gate for HTTP request-body extractors.
#
# WHAT THIS GATES: no function under an `adapters/http` tree may take a bare `axum::Json<T>` in
# REQUEST position (its parameter list). A bare `Json<T>` answers a refused body — malformed JSON,
# wrong content-type, schema mismatch, oversized body — with axum's default PLAIN TEXT rejection,
# escaping the service's stable `{"error":{code,message}}` envelope. The house extractor
# `EnvelopeJson<T>` (rs/crates/services/paigasus-iam/src/adapters/http/json.rs) exists precisely so
# that cannot happen. Fourteen handlers were converted by SMA-587; nothing but this gate stops a
# fifteenth being written with bare `Json<T>` tomorrow.
#
# WHAT THIS DOES NOT GATE: RESPONSE position. `-> Result<Json<Dto>, ApiError>` is the correct and
# universal way to render a success body here and is deliberately untouched — which is the whole
# reason this is a parser and not a grep. See ci/http-extractor/README.md's Limitations for the
# residuals the scan genuinely cannot see (an aliased import, a body taken as `Bytes`/`String`).
#
# It never shells out to cargo (the Moon task is toolchain: 'system') and reads no YAML.
#
# usage: check.py [--self-test | --check]
#   rc 0 clean · rc 1 a violation (or a stale ALLOW row) · rc 2 the checker itself is broken
import glob
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]

# Every `.rs` under any service crate's `adapters/http` tree. `**` matches zero or more
# directories, so today's flat layout AND a future `http/v2/users.rs` are both covered. This is
# also, verbatim, the Moon task's first `inputs` entry — the two MUST stay identical, or the gate
# is scheduled on files it never scans (or scans files that never schedule it). `self_test`
# asserts the glob still resolves to the real tree.
SCAN_GLOB = "rs/crates/services/*/src/adapters/http/**/*.rs"

# (type name, enabled, required replacement)
#
# One row per extractor type with an explicit on/off flag, so closing the other known instances of
# this same hole later is a FLAG FLIP rather than a second gate.
#
# `Query` and `Path` are RESERVED, NOT FORGOTTEN. Ten `Query<…>` bindings and two `Path<String>`
# in this tree still answer a refused query string / path segment outside the envelope — the same
# class of escape, a different extractor. SMA-587's spec defers them explicitly (its "Out of
# scope"), and their replacement is the follow-up's design call, not this gate's; turning the rows
# on today would red the build on work this ticket is not doing.
#
# NOTE for whoever flips `Path` on: the match is an identifier-boundary match on the bare name, so
# an enabled `Path` row also matches `std::path::Path` (`p: &Path`). That is not a problem in an
# `adapters/http` tree today, but it is the first thing to check, and `UuidPath<…>` — the house
# replacement already in use — is correctly NOT matched (see `_banned_pattern`).
BANNED = (
    ("Json", True, "EnvelopeJson"),
    ("Query", False, None),
    ("Path", False, None),
)

# Files permitted to name a banned extractor in a parameter span. Each row states why.
#
# Kept deliberately tiny: an ALLOW row switches the gate OFF for a whole file, which is the one
# structural way this check could come to guard nothing.
#
# There is intentionally NO "stale row" red here (unlike ci/error-registry/check.py's MANIFEST).
# json.rs today produces ZERO parameter-span hits — every `Json` it names lives in an impl-block
# `where` clause (`Json<T>: FromRequest<S, …>`), a turbofish call (`Json::<T>::from_request`), or a
# match arm (`Ok(Json(value))`), none of which is a parameter list. The row is therefore DEFENSIVE
# and must stay: the definition site legitimately has to be able to name `axum::Json` anywhere,
# including in a future helper's parameters, and a gate that reds on the file whose job is to wrap
# the banned type would just get deleted. What IS asserted is that the path still exists — a rename
# reds rather than silently exempting nothing.
ALLOW = (
    ("rs/crates/services/paigasus-iam/src/adapters/http/json.rs",
     "the extractor's own definition site — it wraps `axum::Json` by construction"),
)


class InfraError(RuntimeError):
    """The inputs or environment are broken, NOT 'the tree regressed'.

    main() maps this to rc 2 so a broken checker aborts loudly instead of folding into a green.
    A gate that fails OPEN is worse than no gate: it converts "unguarded" into "believed guarded".
    """


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
# Signature parsing
# --------------------------------------------------------------------------------------------

_FN = re.compile(r"\bfn\s+(?:r#)?([A-Za-z_$][A-Za-z0-9_]*)")


def _skip_generics(text, i):
    """If `text[i:]` opens a generic parameter list, return the index just past its `>`.

    `->` and `=>` are skipped as UNITS: a bound such as `<F: Fn() -> T>` otherwise closes the list
    on the arrow's `>` and the parameter walk then starts in the wrong place.
    """
    n = len(text)
    while i < n and text[i].isspace():
        i += 1
    if i >= n or text[i] != "<":
        return i
    depth, j = 0, i
    while j < n:
        if text[j : j + 2] in ("->", "=>"):
            j += 2
            continue
        if text[j] == "<":
            depth += 1
        elif text[j] == ">":
            depth -= 1
            if depth == 0:
                return j + 1
        j += 1
    raise InfraError("unterminated generic parameter list")


def parameter_spans(text, origin="<fixture>"):
    """Yield (fn_name, start, end) for every function in `text`, `start:end` being the PARAMETER
    LIST'S CONTENT — the text strictly between `fn <name>(` and its matching `)`.

    This is the single mechanism that answers all four ways a line-oriented grep gets this wrong:

      1. ONE LINE, BOTH CONTEXTS. `… Json(b): Json<RenameBody>) -> Result<Json<OrgDto>, ApiError> {`
         carries a banned request binding and a legal return type on one line. The walk stops at
         the parameter list's own `)`, so the return type is simply never in the span — the
         separation a line scan cannot make. (`_cut_at_return_arrow` below is a second, guarded
         belt on the same buckle.)
      2. MULTI-LINE SIGNATURES. Several handlers put one parameter per line. Paren-balancing is
         layout-blind, so both shapes parse identically and no "join the continuation lines"
         heuristic is needed.
      3. THE NON-DESTRUCTURING FORM. The span is TEXT, and the rule applied to it is "the banned
         type appears here" — not "a `Json(x):` binding pattern appears". The house style already
         writes extractors both ways (`body: Option<EnvelopeJson<RetireBody>>`,
         `path: UuidPath<OrganizationId>`), so `body: Json<CreateNodeBody>` is caught exactly like
         `Json(b): Json<CreateNodeBody>`.
      4. `where` CLAUSES ARE A THIRD CONTEXT. A `where` clause sits after the return type, outside
         the parentheses, so it is outside the span by construction — for a free function AND for
         an impl block's (`impl<S, T> FromRequest<S> for EnvelopeJson<T> where Json<T>: …`), whose
         `where` belongs to no `fn` at all. The same is true of a turbofish call
         (`Json::<T>::from_request(…)`) and a match arm (`Ok(Json(value)) =>`) in a function BODY.

    An unbalanced signature raises InfraError rather than being skipped: a file this parser cannot
    read must abort the gate loudly, never pass quietly.
    """
    n = len(text)
    for m in _FN.finditer(text):
        i = _skip_generics(text, m.end())
        while i < n and text[i].isspace():
            i += 1
        if i >= n or text[i] != "(":
            # `_FN` only matches `fn <ident>` (or `fn r#<ident>`) with whitespace between them, which
            # in well-formed Rust is always a function declaration — and every function declaration,
            # including a zero-argument one, is followed by `(`. No fixture or scanned file has ever
            # exercised a legitimate `fn <ident>` with no following `(`, so this is not a shape to
            # skip past — it is the parser failing to read a signature it claims to recognise.
            # Per this function's own docstring: abort the gate loudly, never pass quietly.
            raise InfraError(
                f"{origin}: `fn {m.group(1)}` at offset {m.start()} is not followed by `(` — "
                "the parser cannot read this signature, so it must not report it clean"
            )
        depth, j = 0, i
        while j < n:
            if text[j] == "(":
                depth += 1
            elif text[j] == ")":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        else:
            raise InfraError(
                f"{origin}: unbalanced parameter list for `fn {m.group(1)}` at offset {i} — "
                "the parser cannot read this file, so it must not report it clean"
            )
        yield m.group(1), i + 1, j


def _cut_at_return_arrow(span):
    """Cut a parameter span at a top-level `->`, but ONLY when nothing is lost.

    With a correctly balanced span the return type is already excluded, so this is normally a
    no-op; it is the second belt on failure mode 1, for the case where balancing was defeated and
    the span ran on into `-> Result<Json<Dto>, _>`.

    The guard matters. A depth-0 `->` is also legal INSIDE a parameter list — an fn-pointer or
    closure-typed parameter, `f: fn(u32) -> Json<X>, body: Json<Y>`. Cutting unconditionally there
    would silently drop `body: Json<Y>` and the gate would fail OPEN on exactly the file it was
    meant to read. So the cut is taken only when the tail holds no depth-0 `,`, i.e. when it
    really is a trailing return type and no parameter can be lost by removing it.
    """
    arrow = _top_level_index(span, "->")
    if arrow is None:
        return span
    if _top_level_index(span[arrow:], ",") is not None:
        return span  # a parameter follows: this arrow belongs to a parameter's own type
    return span[:arrow]


def _top_level_index(span, needle):
    """Index of `needle` in `span` at paren depth 0 and angle depth 0, else None.

    Angle depth tracks `<`/`>` so a `,` inside `Result<A, B>` is not read as a top-level comma;
    `->` and `=>` are consumed as units so their `>` never closes a generic.
    """
    paren = angle = 0
    i, n = 0, len(span)
    while i < n:
        if span[i : i + 2] in ("->", "=>"):
            if paren == 0 and angle == 0 and span[i : i + len(needle)] == needle:
                return i
            i += 2
            continue
        c = span[i]
        if c in "([{":
            paren += 1
        elif c in ")]}":
            # Clamped at zero, never negative. A RUNAWAY span — the very case
            # `_cut_at_return_arrow` exists for — opens with an unmatched `)`, and an unclamped
            # counter would then sit at -1 for the rest of the span so the `->` is never seen at
            # "top level" and the cut is silently never taken.
            paren = max(paren - 1, 0)
        elif c == "<":
            angle += 1
        elif c == ">":
            angle = max(angle - 1, 0)
        elif paren == 0 and angle == 0 and span[i : i + len(needle)] == needle:
            return i
        i += 1
    return None


def _banned_pattern(name):
    """Match `name` as a whole IDENTIFIER inside a parameter span.

    The boundaries are what make the rule usable:
      * `EnvelopeJson<T>` and `UuidPath<T>` — the house replacements — do NOT match, because the
        preceding character is an identifier character. Without that, the gate would red on every
        handler SMA-587 just fixed.
      * `JsonRejection` does not match either (trailing identifier characters).
      * `axum::Json<T>` DOES match: `:` is not an identifier character. Catching the
        fully-qualified spelling is free here and strictly better than not.

    No `<` is required after the name. Requiring one would be narrower for no benefit and would
    miss a hypothetical non-generic alias; matching the bare identifier fails CLOSED.
    """
    return re.compile(r"(?<![A-Za-z0-9_])" + re.escape(name) + r"(?![A-Za-z0-9_])")


_PATTERNS = {name: _banned_pattern(name) for name, _, _ in BANNED}


def violations_in(text, origin="<fixture>", enabled=None):
    """[(fn_name, line, extractor, replacement)] for one file's worth of Rust source.

    `enabled` defaults to the BANNED rows whose flag is on; the self-test passes an explicit set to
    exercise the reserved rows without turning them on for the real tree.
    """
    if enabled is None:
        enabled = {name: repl for name, on, repl in BANNED if on}
    stripped = strip_noise(text)
    found = []
    for fn_name, start, end in parameter_spans(stripped, origin):
        span = _cut_at_return_arrow(stripped[start:end])
        for extractor, replacement in sorted(enabled.items()):
            hit = _PATTERNS[extractor].search(span)
            if hit:
                line = stripped.count("\n", 0, start + hit.start()) + 1
                found.append((fn_name, line, extractor, replacement))
    return found


def scanned_files():
    """Repo-relative paths the gate reads, in a stable order."""
    return sorted(
        Path(p).relative_to(REPO).as_posix()
        for p in glob.glob(str(REPO / SCAN_GLOB), recursive=True)
    )


# --------------------------------------------------------------------------------------------
# The real check
# --------------------------------------------------------------------------------------------

# The positive control for the whole parser (see `check`). `EnvelopeJson` is the house extractor
# every converted handler now takes in request position, so if the parameter-span walk stops
# reaching real handler signatures — a rename, a layout change, a regex that quietly matches
# nothing — this identifier disappears from every span and the gate aborts instead of reporting a
# tree it never actually read as clean.
LIVENESS_IDENT = "EnvelopeJson"


def check():
    files = scanned_files()
    if not files:
        raise InfraError(f"{SCAN_GLOB} matched no file — the scan root moved and the gate is scanning nothing")

    allowed = {path for path, _ in ALLOW}
    rc = 0

    for path, reason in ALLOW:
        if not (REPO / path).is_file():
            print(f"stale ALLOW row: {path} no longer exists ({reason})", file=sys.stderr)
            rc = 1

    signatures = 0
    live = 0
    offenders = []
    for path in files:
        text = (REPO / path).read_text(encoding="utf-8")
        stripped = strip_noise(text)
        for _fn, start, end in parameter_spans(stripped, path):
            signatures += 1
            if LIVENESS_IDENT in stripped[start:end]:
                live += 1
        if path in allowed:
            continue
        for fn_name, line, extractor, replacement in violations_in(text, path):
            offenders.append((path, line, fn_name, extractor, replacement))

    if not signatures:
        raise InfraError(
            f"parsed {len(files)} file(s) under {SCAN_GLOB} and found no function signature at all "
            "— the parameter-span parser is broken, so a clean report would be meaningless"
        )
    if not live:
        raise InfraError(
            f"no `{LIVENESS_IDENT}` found in any request position across {len(files)} file(s). Either "
            "every handler stopped using the house extractor, or the parser is no longer reaching "
            "handler signatures. Both mean this gate is guarding nothing"
        )

    if offenders:
        print("HTTP handler takes a banned extractor in REQUEST position "
              "(a refused body would answer outside the error envelope):", file=sys.stderr)
        for path, line, fn_name, extractor, replacement in sorted(offenders):
            print(f"    {path}:{line}  fn {fn_name}(…)  takes `{extractor}` — use `{replacement}` instead",
                  file=sys.stderr)
        print("  the house extractor lives in "
              "rs/crates/services/paigasus-iam/src/adapters/http/json.rs (SMA-587).", file=sys.stderr)
        print("  a legitimate exception needs a reasoned row in ci/http-extractor/check.py's ALLOW.",
              file=sys.stderr)
        rc = 1
    return rc


# --------------------------------------------------------------------------------------------
# Self-test
# --------------------------------------------------------------------------------------------

# (label, source, expected [(fn_name, extractor)] sorted)
#
# The PLANTED VIOLATION cases are what prove this gate can red at all: it carries no separate
# --negative-control mode, so the proof lives here.
FIXTURES = (
    (
        "planted violation — the destructuring form a handler would actually be written with",
        "async fn create_user(State(s): State<AppState>, Json(b): Json<CreateUserBody>) -> Response { }",
        [("create_user", "Json")],
    ),
    (
        "planted violation — ONE LINE, BOTH CONTEXTS: banned request binding + legal return type",
        "async fn rename(State(s): State<AppState>, path: UuidPath<OrganizationId>, "
        "Json(b): Json<RenameBody>) -> Result<Json<OrgDto>, ApiError> {",
        [("rename", "Json")],
    ),
    (
        "planted violation — the NON-DESTRUCTURING form, which no `Json(x):` pattern scan sees",
        "async fn create_node(State(s): State<AppState>, body: Json<CreateNodeBody>) -> Response {",
        [("create_node", "Json")],
    ),
    (
        "planted violation — MULTI-LINE signature, one parameter per line",
        "async fn create_api_key(\n"
        "    State(s): State<AppState>,\n"
        "    Extension(ctx): Extension<AuthContext>,\n"
        "    Json(b): Json<CreateApiKeyBody>,\n"
        ") -> Result<(StatusCode, Json<ApiKeyDto>), ApiError> {\n",
        [("create_api_key", "Json")],
    ),
    (
        "planted violation — the FULLY QUALIFIED spelling",
        "async fn create(body: axum::Json<CreateBody>) -> Response {",
        [("create", "Json")],
    ),
    (
        "legal — RETURN position only, the universal success-body shape",
        "async fn get_team(State(s): State<AppState>, path: UuidPath<TeamId>) "
        "-> Result<Json<TeamDto>, ApiError> {\n    Ok(Json(view.into()))\n}",
        [],
    ),
    (
        "legal — return position in a MULTI-LINE signature with a tuple return type",
        "async fn create_project(\n"
        "    State(s): State<AppState>,\n"
        "    EnvelopeJson(b): EnvelopeJson<CreateProjectBody>,\n"
        ") -> Result<(StatusCode, Json<ProjectDto>), ApiError> {\n",
        [],
    ),
    (
        "legal — `where Json<T>: FromRequest<…>`, the extractor's own impl block (a THIRD context)",
        "impl<S, T> FromRequest<S> for EnvelopeJson<T>\n"
        "where\n"
        "    Json<T>: FromRequest<S, Rejection = JsonRejection>,\n"
        "    S: Send + Sync,\n"
        "{\n"
        "    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {\n"
        "        match Json::<T>::from_request(req, state).await {\n"
        "            Ok(Json(value)) => Ok(EnvelopeJson(value)),\n"
        "        }\n"
        "    }\n"
        "}\n",
        [],
    ),
    (
        "legal — a fn's OWN where clause naming the banned type",
        "fn helper<T>(value: T) -> Response\nwhere\n    Json<T>: IntoResponse,\n{\n}",
        [],
    ),
    (
        "legal — the house replacements must not match by suffix/prefix",
        "async fn retire(State(s): State<AppState>, path: UuidPath<OrganizationId>, "
        "body: Option<EnvelopeJson<RetireBody>>) -> Result<StatusCode, ApiError> {",
        [],
    ),
    (
        "legal — `JsonRejection` in request position is not `Json`",
        "fn envelope_rejection(rejection: JsonRejection) -> Response {",
        [],
    ),
    (
        "legal — a body taken as Bytes (the gateway's shape); see README Limitations L3",
        "pub async fn chat_completions(State(state): State<AppState>, "
        "caller: Option<Extension<CallerContext>>, body: Bytes) -> Response {",
        [],
    ),
    (
        "legal — a COMMENT inside the parameter list must not be read as a binding",
        "async fn create(\n"
        "    // a bare Json<CreateBody> here would be a bug\n"
        "    body: EnvelopeJson<CreateBody>,\n"
        ") -> Response {",
        [],
    ),
    (
        "legal — a STRING literal naming the type must not be read as a binding",
        'fn describe(label: &str) -> &str {\n    "Json<T> is banned in request position"\n}',
        [],
    ),
    (
        "planted violation — a generic bound containing `->` must not derail the walk",
        "async fn run<F: Fn() -> u32>(State(s): State<AppState>, Json(b): Json<Body>) -> Response {",
        [("run", "Json")],
    ),
    (
        "planted violation — a RAW IDENTIFIER function name (`fn r#type(...)`) must still be caught",
        "async fn r#type(State(s): State<AppState>, Json(b): Json<TypeBody>) -> Response {",
        [("type", "Json")],
    ),
    (
        "planted violation — a MACRO-TEMPLATE function name (`fn $n(...)`) must still be caught",
        "async fn $n(State(s): State<AppState>, Json(b): Json<Body>) -> Response {",
        [("$n", "Json")],
    ),
)


def self_test():
    """Exercise the parser against in-process fixtures, so a rotted parser reds even when the real
    tree happens to be clean. Runs FIRST in the Moon task, per repo:error-code-single-site."""
    rc = 0

    for label, source, want in FIXTURES:
        got = sorted((fn, ext) for fn, _line, ext, _repl in violations_in(source, label))
        if got != sorted(want):
            print(f"  FAIL [fixture] {label}\n        got  {got}\n        want {sorted(want)}", file=sys.stderr)
            rc = 1

    # At least one fixture must be a planted violation, or the table proves the gate never reds.
    if not any(want for _label, _src, want in FIXTURES):
        print("  FAIL [fixtures] no planted violation — nothing proves this gate can red", file=sys.stderr)
        rc = 1

    # `_cut_at_return_arrow` must cut a runaway span but must NOT eat a parameter that follows an
    # fn-pointer's own arrow. The second case is a fail-OPEN if the guard is ever removed.
    runaway = "a: State<S>) -> Result<Json<Dto>, ApiError"
    if _cut_at_return_arrow(runaway) != "a: State<S>) ":
        print(f"  FAIL [_cut_at_return_arrow] a runaway span was not cut: "
              f"{_cut_at_return_arrow(runaway)!r}", file=sys.stderr)
        rc = 1
    guarded = "f: fn(u32) -> Dto, body: Json<Y>"
    if _cut_at_return_arrow(guarded) != guarded:
        print("  FAIL [_cut_at_return_arrow] cutting at an fn-pointer's arrow dropped a parameter "
              "— the gate would fail OPEN", file=sys.stderr)
        rc = 1

    # A signature the parser cannot balance must abort, never be skipped.
    try:
        list(parameter_spans("fn broken(a: State<S>", "<fixture>"))
    except InfraError:
        pass
    else:
        print("  FAIL [parameter_spans] an unbalanced signature did not raise InfraError", file=sys.stderr)
        rc = 1

    # A `fn <ident>` token with no following `(` must abort loudly too — the bare `continue` this
    # used to be (SMA-587 Task 8 review) silently skipped it instead. There is no known legitimate
    # Rust shape that reaches this branch; a matched `fn <ident>` is always a declaration and every
    # declaration has a parameter list, even an empty `()`.
    try:
        list(parameter_spans("fn no_parens_here", "<fixture>"))
    except InfraError:
        pass
    else:
        print("  FAIL [parameter_spans] `fn <ident>` with no following `(` did not raise InfraError",
              file=sys.stderr)
        rc = 1

    # `strip_noise` must not mistake a lifetime for a char literal and swallow the file.
    lifetimes = "fn parts(self) -> (&'static str, &'static str) {}\nfn take(Json(b): Json<B>) {}"
    got = sorted(fn for fn, _l, _e, _r in violations_in(lifetimes, "<lifetimes>"))
    if got != ["take"]:
        print(f"  FAIL [strip_noise] lifetimes derailed the scan: {got}", file=sys.stderr)
        rc = 1
    # ...and offsets must survive stripping, so a reported line is the real line.
    numbered = 'fn a() {\n    // padding\n    let s = "x";\n}\nfn take(body: Json<B>) {}'
    lines = [line for _fn, line, _e, _r in violations_in(numbered, "<numbered>")]
    if lines != [5]:
        print(f"  FAIL [strip_noise] line numbers shifted after stripping: {lines}", file=sys.stderr)
        rc = 1

    # BANNED table shape: exactly the enabled rows name a replacement, and no name repeats.
    names = [name for name, _on, _repl in BANNED]
    if len(names) != len(set(names)):
        print("  FAIL [BANNED] duplicate extractor rows", file=sys.stderr)
        rc = 1
    for name, on, repl in BANNED:
        if on != (repl is not None):
            print(f"  FAIL [BANNED] {name}: exactly the enabled rows must name a replacement", file=sys.stderr)
            rc = 1
    if not any(on for _n, on, _r in BANNED):
        print("  FAIL [BANNED] every row is disabled — the gate would guard nothing", file=sys.stderr)
        rc = 1

    # A reserved row must still WORK when flipped on. Without this the `Query`/`Path` rows could
    # rot for a year and the flag flip would ship a gate that matches nothing.
    reserved = "async fn list(State(s): State<AppState>, Query(q): Query<PageQuery>) -> Response {"
    got = sorted(ext for _fn, _l, ext, _r in violations_in(reserved, "<reserved>", {"Query": "EnvelopeQuery"}))
    if got != ["Query"]:
        print(f"  FAIL [BANNED reserved] flipping the Query row on matched nothing: {got}", file=sys.stderr)
        rc = 1
    # ...and it must be OFF for the real tree right now, or SMA-587 reds work it does not do.
    if any(on for name, on, _r in BANNED if name in ("Query", "Path")):
        print("  FAIL [BANNED] a reserved row is enabled — that is a follow-up's call", file=sys.stderr)
        rc = 1

    # ALLOW rows must each state a reason, and none may be a glob: an ALLOW row switches the gate
    # off for a whole file, so it is named literally and reviewed.
    seen = [path for path, _why in ALLOW]
    if len(seen) != len(set(seen)):
        print("  FAIL [ALLOW] duplicate path rows", file=sys.stderr)
        rc = 1
    for path, why in ALLOW:
        if not why:
            print(f"  FAIL [ALLOW] {path} has no stated reason", file=sys.stderr)
            rc = 1
        if any(ch in path for ch in "*?["):
            print(f"  FAIL [ALLOW] {path} is a glob — name exempted files literally", file=sys.stderr)
            rc = 1

    # The scan glob must still resolve to the real tree. A gate scanning zero files reports clean.
    files = scanned_files()
    if not files:
        print(f"  FAIL [scan scope] {SCAN_GLOB} matched no file", file=sys.stderr)
        rc = 1
    elif not any(p.endswith("/adapters/http/json.rs") for p in files):
        print(f"  FAIL [scan scope] the extractor's own module is not in scope: {files[:3]}", file=sys.stderr)
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
    except InfraError as exc:
        print(f"INFRASTRUCTURE ERROR: {exc}", file=sys.stderr)
        return 2
    except OSError as exc:
        print(f"INFRASTRUCTURE ERROR: {exc}", file=sys.stderr)
        return 2
    print(f"usage: {Path(__file__).name} [--self-test | --check]", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
