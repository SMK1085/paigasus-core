<!-- SPDX-License-Identifier: Apache-2.0 -->

# http-extractor — the request-body extractor single-site gate

`repo:http-extractor-envelope` runs `check.py --self-test` then `check.py --check`, in one
`script:` block under `set -euo pipefail`.

## What it gates

No function under `rs/crates/services/*/src/adapters/http/**/*.rs` may take a bare `axum::Json<T>`
in **request position** — its parameter list.

A bare `Json<T>` answers a refused body with axum's default **plain-text** rejection: malformed
JSON, a wrong `Content-Type`, a schema mismatch, an oversized body. That escapes the service's
stable `{"error":{code,message}}` envelope that every other IAM response uses. The house extractor
`EnvelopeJson<T>`
([`paigasus-iam/src/adapters/http/json.rs`](../../rs/crates/services/paigasus-iam/src/adapters/http/json.rs))
exists so that cannot happen; SMA-587 converted fourteen handlers to it. This gate is what stops a
fifteenth being written with bare `Json<T>` tomorrow.

Violations report `path:line  fn <name>(…)` and name the required replacement.

## What it does NOT gate

**Response position.** `-> Result<Json<Dto>, ApiError>` is the correct and universal way to render
a success body here, and it is untouched — that is the whole reason this is a parser and not a
grep. `organizations.rs` really does carry both contexts on one physical line.

**`Query` and `Path`.** `BANNED` carries a row per extractor with an on/off flag. `Query` and
`Path` are **reserved and deliberately off**: ten `Query<…>` bindings and two `Path<String>` in
this tree still answer outside the envelope, the same class of escape with a different extractor,
and SMA-587's spec defers them explicitly. Closing them later is a **flag flip**, not a second
gate. (Whoever flips `Path` on: the match is on the bare identifier, so it will also match
`std::path::Path` — `p: &Path`. `UuidPath<…>`, the house replacement already in use, is correctly
not matched.)

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

Identifier boundaries are what keep the rule usable: `EnvelopeJson<T>`, `UuidPath<T>` and
`JsonRejection` do **not** match, while `axum::Json<T>` does (`:` is not an identifier character).

## Fail-closed properties

A gate that fails open is worse than no gate, because it converts "unguarded" into "believed
guarded". Four things here abort with **rc 2** (`InfraError`) rather than reporting a clean tree:

- `SCAN_GLOB` matching no file (the scan root moved).
- A signature whose parentheses do not balance — the parser cannot read that file, so it must not
  call it clean.
- Zero function signatures parsed across the whole scope.
- **The positive control:** zero occurrences of `EnvelopeJson` in *any* request position. If the
  parameter-span walk stops reaching real handler signatures, this identifier vanishes from every
  span and the gate aborts. Eighteen request positions carry it today.

`--self-test` runs **first and in the same script block**, and carries the planted violations that
prove the gate can red at all — it has no separate `--negative-control` mode. `set -euo pipefail`
in the Moon task is required: Moon does not enable errexit for `script:` blocks, so without it a
failing `--self-test` would be masked by a passing `--check`.

## The ALLOW table

One row, stating its reason: `paigasus-iam/src/adapters/http/json.rs`, *the extractor's own
definition site — it wraps `axum::Json` by construction*.

An ALLOW row switches the gate off for a **whole file**, so rows are named literally (globs are
rejected by the self-test) and must each state a reason. The row's path is asserted to still
exist, so a rename reds rather than silently exempting nothing.

There is deliberately **no stale-row red** (unlike `ci/error-registry/check.py`'s `MANIFEST`).
`json.rs` produces zero parameter-span hits today — all of its `Json` mentions are impl-block
`where` clauses, turbofish calls and match arms. The row is *defensive*: the definition site
legitimately has to be able to name `axum::Json` anywhere, including in a future helper's
parameters, and a gate that red on the file whose job is to wrap the banned type would just get
deleted.

## Limitations

**L1 — An aliased import.** `use axum::Json as J;` followed by `body: J<CreateBody>` is invisible:
the scan matches type *names*, and resolving aliases means resolving `use` trees. Not closed
because the alias would have to be introduced deliberately, in a file whose whole module already
imports `EnvelopeJson`, and it survives neither review nor a reader.

**L2 — A path spelling this gate cannot predict.** `axum::Json<T>` *is* caught (identifier
boundary), and so is any `…::Json<T>`. A re-export renamed on the way through
(`crate::compat::JsonBody<T>` aliasing `axum::Json`) is not.

**L3 — A body taken as `Bytes` or `String`.** These extract successfully and defer parsing to the
handler, so no rejection is produced for this gate to care about — but the handler's own
`serde_json::from_slice` error then answers in whatever shape that handler chooses. The gateway's
`chat_completions` takes `body: Bytes` for streaming reasons and is legitimately outside the IAM
envelope contract. Catching "hand-rolled parsing of an opaque body" is a different gate.

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
