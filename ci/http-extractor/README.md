<!-- SPDX-License-Identifier: Apache-2.0 -->

# http-extractor — the request-body extractor single-site gate

`repo:http-extractor-envelope` runs `check.py --self-test` then `check.py --check`, in one
`script:` block under `set -euo pipefail`.

## What it gates

No function under `rs/crates/services/*/src/adapters/http/**/*.rs` may take a bare `axum::Json<T>`,
`axum::extract::Query<T>`, `axum::extract::Path<T>` or `axum::body::Bytes` in **request
position** — its parameter list. `BANNED` carries one row per extractor type with an on/off flag,
and all four rows are enabled: this is now the complete set of request-input kinds used in this
tree.

A bare extractor of any of these kinds answers a refused input with axum's default **plain-text**
rejection: malformed JSON, a wrong `Content-Type`, a schema mismatch, an unparseable query value, a
malformed path segment, an oversized body. That escapes the stable `{"error":{code,message}}`
envelope every other response in these two services uses. The house extractors —
`EnvelopeJson<T>`
([`paigasus-iam/src/adapters/http/json.rs`](../../rs/crates/services/paigasus-iam/src/adapters/http/json.rs)),
`EnvelopeQuery<T>`
([`paigasus-iam/src/adapters/http/query.rs`](../../rs/crates/services/paigasus-iam/src/adapters/http/query.rs)),
`UuidPath<F>`/`StringPath<F>`
([`paigasus-iam/src/adapters/http/path.rs`](../../rs/crates/services/paigasus-iam/src/adapters/http/path.rs))
and `EnvelopeBytes`
([`paigasus-gateway/src/adapters/http/bytes.rs`](../../rs/crates/services/paigasus-gateway/src/adapters/http/bytes.rs))
— exist so that cannot happen. SMA-587 converted fourteen `Json` handlers; SMA-588 converted the
ten `Query` handlers, the two `Path<String>` handlers, and the gateway's one `Bytes` handler. This
gate is what stops a new one being written with a bare extractor tomorrow.

Violations report `path:line  fn <name>(…)` and name the required replacement.

## What it does NOT gate

**Response position.** `-> Result<Json<Dto>, ApiError>` is the correct and universal way to render
a success body here, and it is untouched — that is the whole reason this is a parser and not a
grep. `organizations.rs` really does carry both contexts on one physical line.

## How it reads Rust without a Rust parser

`check.py` is pure Python (the Moon task is `toolchain: 'system'`) and never shells out to cargo.

1. **Lexical pre-pass.** Comments and string/char literals are blanked to spaces, preserving length
   and newlines. A `(` inside a string can no longer unbalance the walk, a comment inside a
   parameter list can no longer be read as a binding, and reported line numbers stay real. A
   lifetime (`&'static str`) is not mistaken for a char literal.
2. **Paren-balance from `fn <name>(` to its matching `)`.** The result is the parameter span, and
   nothing else. Generic parameter lists are skipped first, with `->`/`=>` consumed as units so a
   bound like `<F: Fn() -> T>` does not close the list early.
3. **Identifier-boundary match for each enabled banned type inside that span.** The rule is *the
   type appears in the parameter span*, not *a `Json(x):` binding pattern appears*.

That single mechanism answers all four ways a line grep gets this wrong:

| Failure mode | Real example | How it is handled |
| --- | --- | --- |
| One line, both contexts | `… Json(b): Json<RenameBody>) -> Result<Json<OrgDto>, ApiError> {` | The walk stops at the parameter list's own `)`, so the return type is never in the span. `_cut_at_return_arrow` is a second, *guarded* belt for the case where balancing was defeated. |
| Multi-line signature | `api_keys.rs`, `organizations.rs` — one parameter per line | Paren-balancing is layout-blind; both shapes parse identically. |
| Non-destructuring form | `body: Option<EnvelopeJson<RetireBody>>`, `path: UuidPath<OrganizationId>` | The span is scanned as text, so `body: Json<CreateNodeBody>` is caught exactly like `Json(b): Json<…>`. |
| `where` clauses (a third context) | `where Json<T>: FromRequest<S, Rejection = JsonRejection>` in `json.rs` | A `where` clause sits outside the parentheses — for a free function and for an impl block, whose `where` belongs to no `fn` at all. So do `Json::<T>::from_request(…)` and `Ok(Json(value))`, which are function *bodies*. |

Identifier boundaries are what keep the rule usable: `EnvelopeJson<T>`, `EnvelopeQuery<T>`,
`UuidPath<T>`, `StringPath<T>`, `EnvelopeBytes` and the `*Rejection` types do **not** match, while
`axum::Json<T>` does (`:` is not an identifier character).

The `Path` row additionally requires a following `<` (`_banned_pattern`'s `require_generic`), which
none of the other rows do. A bare `Path` also matches `std::path::Path` (`p: &Path`) — a shape that
does not exist in either `adapters/http` tree today, but the `<` requirement is cheap and fails
safe against it. `PathBuf` needs no such flag: the trailing identifier-boundary lookahead already
excludes it. Every other row keeps plain bare-identifier matching, which fails **closed**: it would
catch a hypothetical non-generic alias that a `<`-anchored match would miss.

## Fail-closed properties

A gate that fails open is worse than no gate, because it converts "unguarded" into "believed
guarded". Six things here abort with **rc 2** (`InfraError`) rather than reporting a clean tree:

- An unterminated generic parameter list (`_skip_generics`) — a `<...>` that never closes.
- A `fn <ident>` token not followed by `(` — the parser cannot read this shape. A `macro_rules!`
  matcher (`fn $n:ident`) is the known case; see L7 in Limitations below.
- A signature whose parentheses do not balance — the parser cannot read that file, so it must not
  call it clean.
- `SCAN_GLOB` matching no file (the scan root moved).
- Zero function signatures parsed across the whole scope.
- **The positive control:** zero occurrences of `EnvelopeJson` in *any* request position. If the
  parameter-span walk stops reaching real handler signatures, this identifier vanishes from every
  span and the gate aborts. Eighteen request positions carry it today.

`--self-test` runs **first and in the same script block**, and carries the planted violations that
prove the gate can red at all — it has no separate `--negative-control` mode. `set -euo pipefail`
in the Moon task is required: Moon does not enable errexit for `script:` blocks, so without it a
failing `--self-test` would be masked by a passing `--check`.

## The ALLOW table

Three rows today, each stating its reason and each an extractor's own definition site:

- `paigasus-iam/src/adapters/http/json.rs` — wraps `axum::Json` by construction.
- `paigasus-iam/src/adapters/http/query.rs` — wraps `axum::Query` by construction.
- `paigasus-gateway/src/adapters/http/bytes.rs` — wraps `axum::body::Bytes` by construction.

**ALLOW is per-FILE, not per-row.** An ALLOW row switches the gate off for the **whole file** it
names — every enabled extractor, not just the one the row's reason mentions — so rows are named
literally (globs are rejected by the self-test) and must each state a reason. The row's path is
asserted to still exist, so a rename reds rather than silently exempting nothing. This is the one
structural way this check could come to guard nothing, so the table stays deliberately tiny.

Two files that are NOT in this table, on purpose:

- `paigasus-iam/src/adapters/http/path.rs` — `UuidPath` and `StringPath` reach
  `Path::<String>::from_request_parts` inside a function **body**, outside every parameter span,
  and the `<`-requiring `Path` pattern does not match a turbofish either. The file needs no
  exemption; adding one would be a silent widening with no matching need.
- `paigasus-gateway/src/adapters/http/chat.rs` — `chat_completions` is the exact handler the
  `Bytes` row exists to catch. An ALLOW row here would report it green over an unconverted
  handler — a fail-open on the exact target this row was added for.

There is deliberately **no stale-row red** (unlike `ci/error-registry/check.py`'s `MANIFEST`).
Each of the three files produces zero parameter-span hits today — every mention of its wrapped
type lives in an impl-block `where` clause, a turbofish call, or a match arm. Each row is
*defensive*: the definition site legitimately has to be able to name its wrapped axum type
anywhere, including in a future helper's parameters, and a gate that red on the file whose job is
to wrap the banned type would just get deleted.

## Limitations

**L1 — An aliased import.** `use axum::Json as J;` followed by `body: J<CreateBody>` is invisible:
the scan matches type *names*, and resolving aliases means resolving `use` trees. Not closed
because the alias would have to be introduced deliberately, in a file whose whole module already
imports `EnvelopeJson`, and it survives neither review nor a reader.

**L2 — A path spelling this gate cannot predict.** `axum::Json<T>` *is* caught (identifier
boundary), and so is any `…::Json<T>`. A re-export renamed on the way through
(`crate::compat::JsonBody<T>` aliasing `axum::Json`) is not.

**L3 — A body taken as `String`.** A bare `String` extractor honours `DefaultBodyLimit` exactly
like `Bytes` did, so an over-limit body produces the identical plain-text rejection SMA-588 closed
for `Bytes` — the `BANNED` table has no `String` row because no `String` request-body extractor
exists in either tree today, not because the hole does not apply to it. A future one needs the
same treatment: a house `EnvelopeString` extractor and a new enabled row, exactly like `Bytes`.

**L4 — Scope is `adapters/http` only.** A handler mounted from outside that tree is not scanned.
The Moon task's `inputs` and `SCAN_GLOB` are the same glob, so the two cannot silently disagree
about *which* files matter — but neither of them looks any wider.

**L5 — An `#[allow]`-style escape does not exist, by design.** The only escape is an ALLOW row,
which is a reviewed diff to this directory. That is the point.

**L6 — `_cut_at_return_arrow` is guarded, not smart.** It declines to cut when a depth-0 `,`
follows the arrow, because an fn-pointer-typed parameter (`f: fn(u32) -> Dto, body: Json<Y>`)
would otherwise lose every parameter after it — a fail-open. The consequence is that on a runaway
span *also* containing such a parameter, the cut is skipped and the return type stays in scope,
which can only produce a false **positive** (a red on correct code). Failing that way round is the
correct trade, and the self-test pins both branches.

**L7 — `_FN` now recognises raw identifiers and macro-template names, and an unrecognisable `fn`
token aborts rather than being skipped.** Task 7's review found that `_FN` required
`[A-Za-z_]` immediately after `fn`, so a raw identifier (`fn r#type(body: Json<X>)`) was not
recognised as a function at all — and the branch handling a matched `fn` whose `(` could not be
found did a bare `continue`, skipping it silently. Both contradicted `parameter_spans`' own
docstring ("must abort the gate loudly, never pass quietly"). Measured impact at the time was
zero (339 `fn` tokens produced 339 parsed spans), but a raw identifier is already live in the
scanned tree — `pub r#type` in
[`paigasus-gateway/src/adapters/http/error.rs:42`](../../rs/crates/services/paigasus-gateway/src/adapters/http/error.rs) —
so the shape is not hypothetical, even though that particular occurrence is a struct field, not a
`fn` name. `_FN` now matches `fn (?:r#)?name`
with `name` also allowing a leading `$` (a `macro_rules!` template parameter), and a matched `fn`
token with no following `(` now raises `InfraError` instead of being silently skipped: the parser
cannot read this shape. A `macro_rules!` matcher (`fn $n:ident`) is the known case — the same `$`
support this change adds also makes `_FN` match inside a matcher arm like
`macro_rules! r { (fn $n:ident) => {...} }`, which is not a function declaration and is never
followed by `(` — and it must be handled explicitly rather than skipped.
