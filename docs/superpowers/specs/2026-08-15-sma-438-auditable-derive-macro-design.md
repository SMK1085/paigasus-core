# SMA-438 — `#[derive(Auditable)]` for DTOs embedding `AuditMetadata`

Linear: [SMA-438](https://linear.app/smaschek/issue/SMA-438/rust-derive-macro-to-auto-impl-auditable-for-dtos-embedding)
Follow-up from: [SMA-425](https://linear.app/smaschek/issue/SMA-425) (shipped the `Auditable` trait; the
derive was an explicit out-of-scope follow-up there — "manual impls fine for now")
ADR context: [ADR-0012 — per-language thin wrappers](https://app.notion.com/p/386830e8fbaa816d8a43e941ef5b4c4f),
ADR-0004 (contracts are the codegen source of truth)

> Revised after an adversarial review. Three blockers and six majors were folded in; the injection
> mechanism, the drift gate, and the testing strategy all changed materially. §12 records what was
> rejected and why.

## 1. Context

### 1.1 What exists today

SMA-425 landed `Auditable` in `rs/crates/libs/paigasus-proto/src/audit.rs`: one required method
plus four defaulted accessors (`created_by`, `modified_by`, `created_at`, `modified_at`).

**Seven** generated messages embed `AuditMetadata audit = N;`:

| Proto file | Message |
|---|---|
| `contracts/proto/paigasus/common/v1/auditable_example.proto` | `AuditableExample` |
| `contracts/proto/paigasus/iam/v1/iam.proto` | `Organization`, `Team`, `Project`, `Membership`, `ServiceAccount`, `ApiKey` |

The only `impl Auditable` in the repository is a `cfg(test)` one on `AuditableExample`, inside
`audit.rs` itself. SMA-425 put it there because the orphan rule blocks it from an integration-test
crate.

`paigasus-iam`'s `adapters/grpc/convert.rs` *writes* `audit: Some(audit(..))` on all six IAM DTOs.
**Nothing in the repository reads through the trait.** That is the honest value baseline: this issue
removes boilerplate that has not yet been written, for readers that do not yet exist. §12 records
why it is still worth doing now rather than deferring.

`syn`, `quote`, and `proc-macro2` are **already** in `rs/Cargo.lock` — `proc-macro2 1.0.106` (:2930),
`quote 1.0.45` (:3147), `syn 2.0.117` (:4467) **and `syn 3.0.3` (:4478)**, pulled by `async-trait`
and `thiserror-impl`. Pinning `syn = "3"` therefore adds no crate and no second major version to the
build. The marginal dependency cost of this issue is approximately zero.

### 1.2 What this issue delivers

A `#[derive(Auditable)]` proc macro in a new companion crate, applied to all seven generated
messages through the codegen pipeline, plus a test that keeps the applied set honest.

## 2. Findings established experimentally

Every load-bearing assumption was probed in the worktree before designing. F1–F6 were probed for
the first draft; F7 was probed in response to the adversarial review and **changed the mechanism**.

### 2.1 The pinned prost plugin accepts attribute injection (F1)

Generated structs are not hand-editable and `clean: true` wipes them on every regeneration, so
codegen injection is the only route. Probed against
`buf.build/community/neoeinstein-prost:v0.5.0`: `buf generate` exits 0 and emits the attribute
above the prost derives. The probe wrote only to a scratch out-dir; `git status` confirmed the
committed trees were untouched.

### 2.2 `extern crate self as …` resolves the derive's absolute paths in-crate (F2)

A derive crate cannot depend on `paigasus-proto` (that is the cycle), so its output must name the
trait by absolute path. But the derive is applied to code *inside* `paigasus-proto`, where
`::paigasus_proto` does not normally resolve. `extern crate self as paigasus_proto;` fixes it —
probed in a throwaway two-crate workspace under edition 2024 with `warnings = "deny"`. Same trick
`serde` uses.

### 2.3 A re-exported derive and a trait may share one name (F3)

`pub use paigasus_proto_derive::Auditable;` next to `pub trait Auditable` compiles — macros live in
the macro namespace, traits in the type namespace. One `use paigasus_proto::audit::Auditable;`
imports both. This is the `serde::Serialize` pattern.

### 2.4 The missing-field error renders correctly spanned (F4)

`syn::Error::new_spanned(&input.ident, …).to_compile_error()` anchors the diagnostic at the type
name:

```
error: #[derive(Auditable)] requires a field named `audit`
 --> hostcrate/src/bad.rs:2:12
  |
2 | pub struct NoAuditField {
  |            ^^^^^^^^^^^^
```

### 2.5 `syn` 3 works for this usage (F5)

The spike compiles and passes on `syn = "3"` unchanged. Combined with §1.1 — `syn 3.0.3` is already
in this workspace's lock — pinning `"3"` matches what `async-trait`/`thiserror-impl` already
resolve and introduces no new major.

### 2.6 The derive resolves through the re-export at the derive position (F6)

F2 and F3 together imply, but do not prove, the spelling codegen will inject. Probed directly by
naming the derive exactly as `buf.gen.yaml` will — through the `audit` re-export, via the
`extern crate self` path, from inside a nested module carrying `#![allow(warnings)]`:

```rust
#[derive(::hostcrate::audit::Auditable)]
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Organization { … }
```

Compiles and passes.

### 2.7 `type_attribute` leaks onto nested oneof enums; `message_attribute` does not (F7)

prost's path matching is prefix-based over sub-paths, so a `type_attribute` on a message also
matches types *nested inside* it — including the enum prost synthesizes for a `oneof`. Probed
against the real `ListMembershipsRequest` (`iam.proto:215`, which has a `oneof filter`):

```
type_attribute=.paigasus.iam.v1.ListMembershipsRequest=#[derive(TYPEATTR)]
    → line 326  on `pub struct ListMembershipsRequest`     ← wanted
    → line 339  on `pub enum Filter`                        ← LEAK

message_attribute=.paigasus.iam.v1.ListMembershipsRequest=#[derive(MSGATTR2)]
    → line 327  on `pub struct ListMembershipsRequest` only ← correct
```

None of the seven target messages has a `oneof` *today*, so `type_attribute` would have shipped
green. But adding a `oneof` to any of them later would silently apply the derive to a generated
enum, producing a `compile_error!` pointing into a file nobody edits. **Therefore: use
`message_attribute=`** (D2). `message_attribute` is confirmed supported by the pinned plugin.

## 3. Decisions

**D1 — Ship the derive and apply it to the generated DTOs.** See §12 for the full proportionality
argument, which the adversarial review challenged directly. Summary: the marginal dependency cost is
zero (§1.1), injection is one verified line per message (F7), and the "validate it against a real
aggregate first" caveat in the Linear issue was written assuming a derive that only reached
hand-written types — injection gives it seven real generated targets today.

**D2 — `message_attribute`, one explicit entry per message.** Not `type_attribute` (F7: leaks onto
nested oneof enums) and not a blanket `.=` match (would attach the derive to every message and enum
in the tree, bloat the committed codegen diff, and force the derive to be silent on a missing field,
contradicting the issue's error requirement).

**D3 — Validate the field name, not its type.** The derive requires a field literally named `audit`
and hard-errors when it is absent. It does **not** inspect the field's type: a proc macro sees
tokens, so a syntactic `Option<…AuditMetadata>` check would reject a legitimate type alias or
re-export while still not being a real type check. A wrongly-typed `audit` field is left to rustc.

**D4 — Name it `paigasus-proto-derive`.** A `serde_derive`-style companion crate whose charter is
"derives implementing `paigasus-proto`'s traits"; a future `Capability` or `ErrorReason` derive
lands here without a rename. This is a naming/charter argument, not a publishing one — the crate
must be published either way, as `paigasus-proto` will depend on it.

**D5 — Anchor the emitted paths on `paigasus_proto::audit`.** `audit.rs` re-exports
`AuditMetadata`, so the derive emits `::paigasus_proto::audit::AuditMetadata` rather than the
generated module path. The macro has no knowledge of the codegen layout.

**D6 — No `trybuild`.** The first draft specified UI tests for the error message. Dropped: the
unit tests already assert the `syn::Error`'s message directly, `trybuild` builds a *second* cargo
project under `rs/target/` that relinks `paigasus-proto` (prost + tonic), and this runner already
needed a disk-reclaim step and `CARGO_PROFILE_{DEV,TEST}_DEBUG: line-tables-only` to survive
(`ci.yml:26-42`), with `rs/target` additionally cached. Its unique value over the unit test is span
*placement*, which is fixed by a one-line `new_spanned(&input.ident, …)` and verified once by hand
(F4). Not worth a plausible red, hard-to-diagnose CI job.

**D7 — No new ADR.** This implements Rust-side ergonomics inside ADR-0004's existing codegen
pipeline and introduces no new architectural boundary. Note that ADR-0012 does **not** assume a
derive — its Decision (2) reads "a **thin, hand-written per-language wrapper** (Rust trait / TS
interface / Python Protocol) layered on the generated type". The first draft of this spec claimed
ADR-0012 assumed a derive; that claim was wrong and has been removed. The derive is an *addition*
to ADR-0012's pattern, not an implementation of it. *(Flagged for confirmation: if "first
proc-macro crate in the workspace" or "derives now generate the wrapper ADR-0012 called
hand-written" is judged ADR-worthy, that ADR is a prerequisite.)*

**D8 — `AuditableExample`'s impl becomes public, reversing an SMA-425 decision.** SMA-425 §
explicitly considered and rejected shipping a public impl on the fixture: *"it would make a fixture
type part of the crate's public API; keeping it test-only is faithful to the fixture's purpose."*
Injecting the derive into `AuditableExample` reverses that — the impl is now shipped, not
`cfg(test)`. Accepted deliberately, because the fixture is the cheapest available proof that the
macro works against real codegen (its two SMA-425 tests then exercise the *derived* impl unchanged),
and because a fixture whose entire purpose is to demonstrate the embedding pattern is a strange
thing to exclude from the demonstration. **Consequence for SMA-388:** once `paigasus-proto` is
published, this impl is public API and cannot be removed without a semver break.

## 4. Architecture

### 4.1 `paigasus-proto-derive`

```
rs/crates/libs/paigasus-proto-derive/
  Cargo.toml     # [lib] proc-macro = true; publish = false (mirrors paigasus-proto)
  moon.yml       # id: paigasus-proto-derive-rs, layer: library, language: rust
  src/lib.rs     # #[proc_macro_derive(Auditable)] — bridge only
  src/auditable.rs
```

`src/lib.rs` holds nothing but the bridge:

```rust
#[proc_macro_derive(Auditable)]
pub fn derive_auditable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    auditable::expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
```

All logic lives in `auditable::expand(&DeriveInput) -> syn::Result<TokenStream2>`, which touches
only `syn`/`proc-macro2` types. That signature is callable from an ordinary `#[cfg(test)]` module
(the `proc_macro` crate's API is unavailable outside a real expansion; `proc-macro2`'s is not), so
the macro's logic is unit-tested directly rather than through compilation.

### 4.2 Generated impl

```rust
impl #impl_generics ::paigasus_proto::audit::Auditable for #ident #ty_generics #where_clause {
    fn audit(&self) -> ::core::option::Option<&::paigasus_proto::audit::AuditMetadata> {
        self.audit.as_ref()
    }
}
```

Generics pass through via `Generics::split_for_impl()`. Paths are fully qualified — including
`::core::option::Option` — so the impl compiles in a module that has shadowed any of those names.

### 4.3 Accepted and rejected inputs

Two distinct error messages, not one:

- **E-shape**: `#[derive(Auditable)] can only be applied to a struct with named fields`
- **E-field**: ``#[derive(Auditable)] requires a field named `audit` ``

E-field deliberately does not name a *type*, because D3 declines to check one; promising
`audit: Option<AuditMetadata>` in a message emitted by a macro that never verifies it is a lie the
first draft told.

| Input | Result |
|---|---|
| Named struct with an `audit` field | impl emitted |
| Generic struct (defaults, `where`, lifetimes) with an `audit` field | impl emitted, generics forwarded |
| Named struct without an `audit` field | E-field at the type name |
| Named struct with zero fields (`struct S {}`) | E-field at the type name |
| Tuple struct / unit struct | E-shape at the type name |
| Enum / union | E-shape at the type name |
| Raw-ident field `r#audit` | impl emitted (`Ident` comparison is raw-insensitive) |
| `audit` field of the wrong type | a rustc type error at the generated impl — `E0308`, `E0599`, or `E0283` depending on the type |
| Derive where a manual `impl Auditable` exists | rustc `E0119`; the one known instance is removed in §4.4 |
| Derive from a crate not depending on `paigasus-proto` | rustc `E0433: could not find paigasus_proto`; documented on the derive |

### 4.4 Changes to `paigasus-proto`

- `src/lib.rs` gains `extern crate self as paigasus_proto;` with a comment explaining F2 — this
  line looks removable and is not.
- `src/audit.rs` gains `pub use paigasus_proto_derive::Auditable;` and promotes its existing
  `use crate::paigasus::common::v1::AuditMetadata;` to `pub use`.
- `src/audit.rs` **loses** its `cfg(test)` `impl Auditable for AuditableExample` — with the derive
  injected, the manual impl is a conflicting implementation (`E0119`) and the crate will not
  compile with both. Its two tests stay, now exercising the derived impl. See D8.
- `Cargo.toml` gains `paigasus-proto-derive` (dependency) and `syn` (dev-dependency, for the drift
  test in §6).

## 5. Codegen injection

`contracts/buf.gen.yaml`, on the `neoeinstein-prost` plugin only, gains seven **quoted** opts:

```yaml
      - 'message_attribute=.paigasus.common.v1.AuditableExample=#[derive(::paigasus_proto::audit::Auditable)]'
      - 'message_attribute=.paigasus.iam.v1.Organization=#[derive(::paigasus_proto::audit::Auditable)]'
      # … Team, Project, Membership, ServiceAccount, ApiKey
```

Two syntax constraints, both easy to trip:

- **Quote the scalars.** Unquoted, `- message_attribute=.X=#[derive(…)]` is only *not* a YAML
  comment because `#` is immediately preceded by `=`. One stray space silently truncates the opt.
- **The attribute must contain no comma.** buf joins a plugin's `opt` entries into one
  comma-separated parameter string, so a future `#[derive(A, B)]` must be written as two separate
  `message_attribute` lines for the same message.

The derive is named through the `paigasus_proto::audit` re-export (D5). The Python and TypeScript
plugins are untouched — `opt` is per-plugin, and ADR-0012 records that both languages satisfy the
contract structurally.

**Expected regeneration diff, to be asserted rather than assumed:** exactly 7 added lines across
`rs/crates/libs/paigasus-proto/src/generated/paigasus/{common,iam}/v1/*.rs`; **zero** bytes changed
under `py/packages/paigasus-proto/` and `ts/packages/paigasus-proto/`; and an **unchanged**
`FILE_DESCRIPTOR_SET` in all three generated Rust files. The descriptor set is a function of the
`.proto` sources, which this change does not touch — but CLAUDE.md records that a whitespace proto
edit once shifted it, so this is checked, not assumed. Per CLAUDE.md, regenerate with `buf generate`
directly: `contracts:generate` declares no `outputs:` and can serve stale cached output.

## 6. Drift protection — a test, not a new CI gate

**Why it is needed.** The seven-entry list is hand-maintained. A new message embedding
`AuditMetadata` would get no derive, no error, and no impl — a silent regression surfacing only when
someone eventually calls `.created_by()`.

**Where it lives.** `rs/crates/libs/paigasus-proto/tests/auditable_derive_drift.rs`, an ordinary
`cargo nextest` test in `paigasus-proto`. **No new Moon task, no new `ci.yml` target, no new
CLAUDE.md gate entry.** The files it reads (`src/generated/**`) are inside the crate's own project
directory, so a regeneration makes the crate affected directly; and a `contracts/` edit reaches it
through the existing `contracts → paigasus-proto-rs` edge that `ci/affected-graph/run.sh:106` already
asserts. This is the opposite of the `ops/`-reading gates (`observability-drift`,
`nats-permissions`), which needed a `repo`-scoped task precisely *because* their inputs sat outside
any crate.

**What it asserts.** The test parses `src/generated/**/*.rs` with **`syn`** — not text or regex —
and asserts a biconditional over every struct:

> a struct has a field named `audit` whose type is `Option<…AuditMetadata>`
> **if and only if**
> it carries `#[derive(::paigasus_proto::audit::Auditable)]`

Both directions fail loudly: a new audit-bearing message without the derive, and a stale
`buf.gen.yaml` entry naming a message that no longer embeds audit.

**Why this predicate and not a `.proto` text scan.** The first draft specified a `python3` parser
over `contracts/proto/**`. It was wrong three ways, and the review found all three:

1. **It false-positives on a file already in the tree.** `audit.proto:9` is the comment
   ``// (`AuditMetadata audit = N;`) rather than repeating the four fields per``, three lines above
   `message AuditMetadata {`. Any nearest-message association flags `AuditMetadata` as embedding
   itself, demands a derive for it, and the derive then hard-errors — it has no `audit` field. The
   same text repeats in `auditable_example.proto:9`.
2. **Its predicate differed from the derive's.** The gate asserted "embeds `AuditMetadata`" while
   the derive keys on "has a field named `audit`" (D3). A future
   `AuditMetadata provenance = 5;` or `repeated AuditMetadata revisions = 6;` would make the gate
   *demand* an entry that the derive then *rejects* — a wedged build with no escape hatch.
3. **It checked only the left-hand side of each opt**, so it passed on a misspelled or empty
   attribute value.

Asserting over generated Rust fixes all three at once: the predicate becomes identical to the
derive's *by construction*, it is immune to comments, `oneof`s, nested messages, one-line `{}`
bodies, and the `.paigasus.common.v1.AuditMetadata` / `common.v1.AuditMetadata` spelling variants,
and it checks the attribute text that was actually emitted. Cross-package embedding needs no import
resolution because prost has already resolved it.

**It must be able to fail.** The parsing/checking logic is a pure function over a `syn::File`,
exercised by in-test fixtures covering both failure directions (audit-field-without-derive,
derive-without-audit-field) plus the passing case. Those fixture assertions run on **every** CI
run, not behind an opt-in flag — the review correctly noted that `affected-smoke` invokes
`ci/affected-graph/run.sh` *without* `--negative-control` (`moon.yml:68-88`), making its negative
control a manual affordance that proves nothing in CI. This repo has shipped vacuously-passing
assertions twice (SMA-489's `# TYPE` line, SMA-466's `promtool check config`).

File reads use `encoding="utf-8"` semantics explicitly — Rust's `fs::read_to_string` is UTF-8 by
definition, which is one more reason to prefer it over a `python3` script under an unknown CI locale.

## 7. Testing

| Level | Location | Covers |
|---|---|---|
| Unit | `paigasus-proto-derive/src/auditable.rs` `#[cfg(test)]` | `expand()` over parsed fixtures: happy path, generics with defaults/`where`/lifetimes, raw-ident field, missing field, zero-field struct, tuple struct, unit struct, enum. Error cases assert the exact `syn::Error` message (E-shape vs E-field). |
| Behavior | `paigasus-proto/tests/auditable_derive.rs` | Per-type read-through for all seven generated types. |
| Drift | `paigasus-proto/tests/auditable_derive_drift.rs` | §6's biconditional, plus its own both-direction negative fixtures. |
| Regression | `paigasus-proto/src/audit.rs` `#[cfg(test)]` | The two SMA-425 tests, unchanged, now against the derived impl. |

Two anti-vacuity rules, both from review findings:

**No substring assertions on expansions.** `TokenStream::to_string()` renders as
`self . audit . as_ref ()`, so the natural literal assertion fails and the natural "fix" is to
loosen the needle — at which point `contains("audit")` is satisfied by the emitted path
`::paigasus_proto::audit::Auditable` *alone*, i.e. it passes with the method body deleted. The unit
tests therefore `syn::parse2::<syn::ItemImpl>(expansion)` and assert structurally: trait path, self
type, method ident, and the body compared as a parsed `syn::Expr` against `self.audit.as_ref()`.

**No bare trait-bound checks.** `fn assert_auditable<T: Auditable>()` proves an impl *exists*, not
that it reads the field: a derive emitting `{ None }` would pass it for six of seven types. Each of
the seven is instead constructed with a **distinct sentinel** —
`audit: Some(AuditMetadata { created_by: "<TypeName>".into(), ..Default::default() })` — and
asserted to return exactly that, plus an `audit: None` case per type. (Note that a generic
`assert_auditable::<T>()` must also actually be *called*; an uncalled one is dead code under
`warnings = "deny"`.)

## 8. Repo plumbing

- **`rs/crates/libs/paigasus-proto/moon.yml` gains the Moon edge** — `dependsOn:
  ['paigasus-proto-derive-rs']`, and `^:build` added to its `build` and `test` task `deps`
  *alongside* the existing `contracts:generate`. Without this the new crate is invisible to Moon's
  graph: `paigasus-proto/moon.yml` currently declares no `dependsOn` at all, and
  `paigasus-kernel-parity/moon.yml:7-9` records the rule — *"The task-level `^:build` is what
  propagates `affected` under `moon ci --include-relations` — a project dependsOn alone does not
  (SMA-389 D3)."* Cargo path deps are not auto-synced into Moon's graph here; every in-tree edge is
  hand-declared. The first draft omitted this entirely, which would have red-lined its own
  affected-graph case. Verify Moon tolerates `^:build` next to the `contracts:generate` task dep.
- `rs/Cargo.toml` `[workspace.dependencies]`: `syn = "3"`, `quote = "1"`, `proc-macro2 = "1"`, and
  the in-tree `paigasus-proto-derive = { path = …, version = "0.0.0" }`, each with the
  comment-the-rationale style the table already uses.
- `ci/affected-graph/run.sh`: a new strict-equality case asserting a derive-crate edit reaches
  `paigasus-proto-derive-rs,paigasus-proto-rs,paigasus-gateway-rs,paigasus-iam-rs`. All six existing
  expected sets are expected to stay byte-identical — the derive crate is strictly *upstream* of
  `paigasus-proto`, so `--downstream deep` from any existing case's touched file never reaches it.
  Strict equality reds either way, so this is verified, not assumed.
- `.github/CODEOWNERS` is Moon-generated — regenerate, never hand-edit.

To be confirmed by running, not assumed: `:deny` (all new deps MIT/Apache-2.0) and `:machete`. On
machete, note that `proc-macro2` may be reachable only through `quote!`'s inferred return type; the
repo already carries three `[package.metadata.cargo-machete] ignored` waivers for macro-only deps
(`paigasus-wasm/Cargo.toml:24-29` and the two binding crates) if one is needed.
`:wasm-getrandom-free` *will* run (its inputs include `rs/Cargo.toml`/`Cargo.lock`) and should pass
— proc-macro crates compile for the host and never enter the wasm target's tree.

## 9. Risks

| Risk | Mitigation |
|---|---|
| `extern crate self as paigasus_proto;` looks like dead code and gets deleted | Comment at the line explaining F2; deletion fails the build immediately. |
| The seven-entry list drifts | §6's biconditional test, with both-direction negative fixtures running on every CI run. |
| A future `oneof` added to one of the seven | Averted by `message_attribute` (F7); a `type_attribute` would have broken the build in generated code. |
| A future proto embeds `AuditMetadata` under a name other than `audit` | Out of contract by construction — §6's predicate keys on the field name, so the test neither demands nor forbids a derive there. Documented in `audit.proto`. |
| `audit` field renamed in a proto | The derive hard-errors at compile time; loud, not silent. |
| Publish ordering (SMA-388) | `paigasus-proto-derive` must be published *before* `paigasus-proto`, and the `path` + `version` dep bumped in lockstep. Recorded in §10. |

## 10. Out of scope

- Server-side audit **stamping** (`created_by`/`modified_by` are left empty by
  `convert.rs::audit()` today) — a separate follow-up.
- TypeScript and Python equivalents — ADR-0012: both satisfy the contract structurally.
- Any change to the `Auditable` trait's shape or its defaulted accessors.
- Making any consumer actually *call* the accessors.
- Flipping `publish = true` (`TODO(SMA-388)`) — but note the ordering constraint this adds:
  `paigasus-proto-derive` publishes first, and D8's public impl becomes semver-locked API.

## 11. Acceptance criteria

1. `#[derive(Auditable)]` exists in `paigasus-proto-derive` and is re-exported as
   `paigasus_proto::audit::Auditable`.
2. It generates `impl Auditable` for a named struct with an `audit` field, forwarding generics.
3. It emits a clear, correctly-spanned `compile_error!` — E-field for a struct with no `audit`
   field, E-shape for tuple structs, unit structs, enums, and unions.
4. All seven generated messages embedding `AuditMetadata` implement `Auditable` with no
   hand-written impl anywhere in the repository, each proven by a distinct-sentinel read-through
   assertion (§7).
5. `paigasus-proto`'s drift test enforces §6's biconditional, and its negative fixtures demonstrate
   both failure directions on every CI run.
6. Regeneration produces exactly the diff §5 predicts: 7 added Rust lines, zero py/ts bytes,
   unchanged `FILE_DESCRIPTOR_SET`.
7. The full CI graph passes, including `:deny`, `:machete`, `:affected-smoke`, and codegen-drift.

## 12. Proportionality — the strongest argument against this issue

The adversarial review's central objection deserves a direct answer rather than a rebuttal buried
in D1.

**The objection.** D2 rejects blanket injection in favour of a hand-maintained seven-entry list plus
a drift check. But a `macro_rules! impl_auditable!(…)` list in `audit.rs` is *also* a hand-maintained
list plus a drift check — identical drift characteristics, no new crate, no codegen coupling, no
`extern crate self` hack, ~10 lines. The derive's only unique benefit is `#[derive]` on hand-written
structs, and there are **zero** such structs; nothing reads the trait at all; and the Linear issue
itself says "best validated once a real auditable aggregate exists". That is a large cost multiple
for a feature with no consumers.

**The answer, and what changed because of it.** The objection was largely correct about *cost*, and
three of the four cost components have been removed or reduced in this revision: `trybuild` is gone
(D6), the new repo-level Moon gate is gone (§6 is now an ordinary crate test), and the dependency
cost was never real (§1.1 — `syn`/`quote`/`proc-macro2`, including `syn 3.0.3`, are already locked).
What remains is a ~60-line proc-macro crate, one Moon edge, seven `buf.gen.yaml` lines, and one
affected-graph case.

Against that, two things favour the derive over `macro_rules!`. First, a `macro_rules!` list must
name `crate::paigasus::iam::v1::Organization` inside `audit.rs`, coupling a `common`-scoped module
to the IAM module layout — exactly the coupling D5 exists to avoid — whereas injection puts the
impl next to the type it belongs to. Second, the issue's "validate against a real aggregate first"
caveat was written on the assumption that the derive would only reach hand-written types; injection
gives it seven real generated targets today, which is precisely the validation the caveat asked for.

**This remains a judgment call, and the cheaper paths are live options:** ship `macro_rules!` now
and open the derive as a follow-up landing with the first hand-written aggregate, or defer SMA-438
entirely per the issue's own note. Both are defensible. This spec recommends proceeding, at the
reduced scope above.
