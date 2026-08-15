# SMA-438 — `#[derive(Auditable)]` for DTOs embedding `AuditMetadata`

Linear: [SMA-438](https://linear.app/smaschek/issue/SMA-438/rust-derive-macro-to-auto-impl-auditable-for-dtos-embedding)
Follow-up from: [SMA-425](https://linear.app/smaschek/issue/SMA-425) (shipped the `Auditable` trait; derive
was explicitly out of scope there — "manual impls are fine for the first consumers")
ADR context: [ADR-0012 — per-language thin wrappers](https://app.notion.com/p/386830e8fbaa816d8a43e941ef5b4c4f),
[ADR-0004 — contracts are the codegen source of truth](../../../CONTRIBUTING.md)

## 1. Context

### 1.1 What exists today

SMA-425 landed `Auditable` in `rs/crates/libs/paigasus-proto/src/audit.rs`: one required method
plus four defaulted accessors.

```rust
pub trait Auditable {
    fn audit(&self) -> Option<&AuditMetadata>;
    fn created_by(&self) -> Option<&str> { … }
    fn modified_by(&self) -> Option<&str> { … }
    fn created_at(&self) -> Option<&::prost_types::Timestamp> { … }
    fn modified_at(&self) -> Option<&::prost_types::Timestamp> { … }
}
```

**Seven** generated messages embed `AuditMetadata audit = N;`:

| Proto file | Message |
|---|---|
| `contracts/proto/paigasus/common/v1/auditable_example.proto` | `AuditableExample` |
| `contracts/proto/paigasus/iam/v1/iam.proto` | `Organization`, `Team`, `Project`, `Membership`, `ServiceAccount`, `ApiKey` |

The only `impl Auditable` in the repository is a `cfg(test)` one on `AuditableExample`, inside
`audit.rs` itself. SMA-425 put it there because the orphan rule blocks it from an integration-test
crate, and recorded that reasoning in a comment.

`paigasus-iam`'s `adapters/grpc/convert.rs` *writes* `audit: Some(audit(..))` on all six IAM DTOs.
**Nothing in the repository reads through the trait.** That is the honest value baseline: this issue
removes boilerplate that has not yet been written, for readers that do not yet exist. It is worth
doing now because the boilerplate is about to be written six times over, and because the derive is
the mechanism ADR-0012 assumes on the Rust side.

No `syn`, `quote`, or `proc-macro2` exists anywhere in the Rust workspace today. This introduces a
new dependency family.

### 1.2 What this issue delivers

A `#[derive(Auditable)]` proc macro in a new companion crate, applied to all seven generated
messages through the codegen pipeline, plus a drift gate that keeps the applied set honest.

## 2. Findings established experimentally

Every load-bearing assumption below was probed in the worktree before the design was written,
because four of them decide whether the approach is possible at all.

### 2.1 The pinned prost plugin supports `type_attribute` (F1)

Generated derives are the only way to reach prost output — the structs are not hand-editable and
`clean: true` wipes them on every regeneration. Probed with a scratch template against
`buf.build/community/neoeinstein-prost:v0.5.0`:

```
type_attribute=.paigasus.iam.v1.Organization=#[derive(::probe_crate::Auditable)]
```

`buf generate` exits 0 and emits the attribute **above** the prost derives:

```rust
#[derive(::probe_crate::Auditable)]
#[derive(Clone, PartialEq, Eq, Hash, ::prost::Message)]
pub struct Organization {
```

The probe wrote only to a scratch out-dir; `git status` confirmed the committed trees were
untouched.

### 2.2 `extern crate self as …` resolves the derive's absolute paths in-crate (F2)

A derive crate cannot depend on `paigasus-proto` (that is the cycle), so its output must name the
trait by absolute path `::paigasus_proto::audit::Auditable`. But the derive is applied to code
*inside* `paigasus-proto`, where `::paigasus_proto` does not normally resolve.

`extern crate self as paigasus_proto;` fixes it. Probed in a throwaway two-crate workspace under
edition 2024 with `warnings = "deny"`: the derived impl compiles and its accessors return the
expected values. This is the same trick `serde` uses.

### 2.3 A re-exported derive and a trait may share one name (F3)

`pub use paigasus_proto_derive::Auditable;` next to `pub trait Auditable` in the same module
compiles — macros live in the macro namespace, traits in the type namespace. Probed; this is the
`serde::Serialize` pattern. One `use paigasus_proto::audit::Auditable;` therefore imports both.

### 2.4 The missing-field error renders correctly (F4)

`syn::Error::new_spanned(&input.ident, …).to_compile_error()` produces exactly the diagnostic the
issue asks for, anchored at the type name:

```
error: #[derive(Auditable)] requires a field `audit: Option<AuditMetadata>`
 --> hostcrate/src/bad.rs:2:12
  |
2 | pub struct NoAuditField {
  |            ^^^^^^^^^^^^
```

### 2.5 `syn` 3 works for this usage (F5)

Cargo resolves `syn = "2"` to 2.0.119 while reporting 3.0.3 available. The spike compiles and
passes on `syn = "3"` unchanged, so the workspace pins the current major and skips an immediate
Dependabot bump.

### 2.6 The derive resolves through the re-export at the derive position (F6)

F2 and F3 together imply, but do not prove, the spelling `buf.gen.yaml` will actually inject.
Probed directly by rewriting the spike's generated module to name the derive exactly as codegen
will — through the `audit` re-export, via the `extern crate self` path, from inside a nested
module carrying `#![allow(warnings)]`:

```rust
#[derive(::hostcrate::audit::Auditable)]
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Organization { … }
```

Compiles and passes. This is the exact shape §5 specifies, so no step of the injection path
remains unverified.

## 3. Decisions

**D1 — Ship the derive *and* apply it to the generated DTOs.** A derive with no consumers is
inert, and manual impls for the seven generated types are exactly the boilerplate this issue
exists to delete. Rejected: derive-only (issue-literal, zero consumers); a `macro_rules!`
`impl_auditable!(…)` list in `audit.rs` (cheaper, no new crate, but gives hand-written DTOs no
`#[derive]` and abandons the ergonomics ADR-0012 assumes).

**D2 — Explicit per-message `type_attribute` list, guarded by a drift gate.** The alternative,
`type_attribute=.=`, has no drift risk but puts the attribute on every message *and enum* in the
tree, bloats the committed codegen diff, makes `syn` parse ~100 types, and forces the derive to be
silent on a missing field — which contradicts the issue's stated error requirement. Explicit + gated
keeps the derive strict.

**D3 — Validate the field name, not its type.** The derive requires a field literally named
`audit` and hard-errors when it is absent. It does **not** inspect the field's type: a proc macro
sees tokens, so a syntactic `Option<…AuditMetadata>` check would reject a legitimate type alias or
re-export while still not being a real type check. A wrongly-typed `audit` field is left to rustc,
whose `E0308` is better-spanned and more accurate than anything the macro could emit.

**D4 — Name it `paigasus-proto-derive`.** A `serde_derive`-style companion crate whose charter is
"derives implementing `paigasus-proto`'s traits". A future `Capability` or `ErrorReason` derive
lands here without a rename. Rejected: `paigasus-macros` (a grab-bag crate that a crates.io-bound
crate would have to depend on) and `paigasus-audit-derive` (needs a second crate for the second
derive).

**D5 — Anchor the emitted paths on `paigasus_proto::audit`.** `audit.rs` re-exports
`AuditMetadata`, so the derive emits `::paigasus_proto::audit::AuditMetadata` rather than the
generated module path. The macro then has no knowledge of the codegen layout, and a future
regeneration that moves modules cannot break it.

**D6 — UI tests live in `paigasus-proto`, not in the derive crate.** `trybuild` fixtures must
reference `paigasus_proto`, which would make the derive crate dev-depend on its own dependent.
Cargo permits dev-dependency cycles, but `paigasus-proto` is crates.io-bound (`TODO(SMA-388)`) and
a cycle complicates publishing for no benefit.

**D7 — No new ADR.** This implements ADR-0012's Rust-side ergonomics inside ADR-0004's existing
codegen pipeline; it introduces no new architectural boundary. *(Flagged for confirmation — if
"first proc-macro crate in the workspace" is judged ADR-worthy, that ADR is a prerequisite.)*

## 4. Architecture

### 4.1 `paigasus-proto-derive`

```
rs/crates/libs/paigasus-proto-derive/
  Cargo.toml     # [lib] proc-macro = true; publish = false (mirrors paigasus-proto)
  moon.yml       # id: paigasus-proto-derive-rs, layer: library, language: rust
  src/lib.rs     # #[proc_macro_derive(Auditable)] — bridge only
  src/auditable.rs
```

`src/lib.rs` holds nothing but the `proc_macro` bridge:

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
(the `proc_macro` crate's API is unavailable outside a real macro expansion, `proc-macro2`'s is
not), so the macro's logic is unit-tested directly rather than through compilation.

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

| Input | Result |
|---|---|
| Named struct with an `audit` field | impl emitted |
| Named struct, generic, with an `audit` field | impl emitted, generics forwarded |
| Named struct without an `audit` field | `compile_error!` at the type name |
| Tuple struct / unit struct | `compile_error!` at the type name |
| Enum / union | `compile_error!` at the type name |
| `audit` field of the wrong type | rustc `E0308` at the generated impl |

All five `compile_error!` cases share one message, which names the required field.

### 4.4 Changes to `paigasus-proto`

- `src/lib.rs` gains `extern crate self as paigasus_proto;` with a comment explaining F2 — this
  line looks removable and is not.
- `src/audit.rs` gains `pub use paigasus_proto_derive::Auditable;` and promotes its existing
  `use crate::paigasus::common::v1::AuditMetadata;` to `pub use`.
- `src/audit.rs` **loses** its `cfg(test)` `impl Auditable for AuditableExample` — once the derive
  is injected into `AuditableExample`, the manual impl is a conflicting implementation and the crate
  will not compile with both. Its two existing tests stay, now exercising the derived impl. This is
  the cheapest available proof that the macro works against real codegen.
- `Cargo.toml` gains `paigasus-proto-derive` (dependency) and `trybuild` (dev-dependency).

## 5. Codegen injection

`contracts/buf.gen.yaml`, on the `neoeinstein-prost` plugin only, gains seven opts of the form:

```yaml
      - type_attribute=.paigasus.common.v1.AuditableExample=#[derive(::paigasus_proto::audit::Auditable)]
      - type_attribute=.paigasus.iam.v1.Organization=#[derive(::paigasus_proto::audit::Auditable)]
      # … Team, Project, Membership, ServiceAccount, ApiKey
```

The derive is named through the `paigasus_proto::audit` re-export (D5), so the generated code
depends on one stable path rather than on the derive crate's own name.

The Python and TypeScript plugins are untouched — ADR-0012 records that both languages satisfy the
contract structurally.

`src/generated/**` is regenerated and committed in the same change. The codegen-drift gate covers
this, but note the CLAUDE.md caveat that `contracts:generate` declares no `outputs:` and can serve
stale cached output — regeneration must be done with `buf generate` directly and the diff eyeballed.

## 6. Drift gate — `repo:auditable-derive-drift`

**Why it is needed.** The seven-entry list is hand-maintained. A new message embedding
`AuditMetadata` gets no derive, no error, and no impl — a silent regression that surfaces only when
someone eventually calls `.created_by()` and finds the trait unimplemented.

**What it asserts.** The set of messages embedding `AuditMetadata` in `contracts/proto/**` equals
the set named in `buf.gen.yaml`'s `type_attribute` opts. Both directions fail: a proto message
missing from the list, and a stale list entry naming a message that no longer embeds audit.

**Where it lives.** A `repo`-scoped Moon task with narrow `inputs:`, following the
`observability-drift` / `nats-permissions` precedent — `contracts/` has its own project, but the
gate reads both `contracts/**` and `rs/`, and the repo's convention for cross-cutting guards is to
hang them off `repo` with explicit inputs so they run on exactly the changes that can break them.

```yaml
  auditable-derive-drift:
    description: 'Assert every proto message embedding AuditMetadata carries #[derive(Auditable)] via buf.gen.yaml (SMA-438).'
    script: 'bash ci/auditable-derive-drift/run.sh'
    toolchain: 'system'
    inputs:
      - 'contracts/proto/**/*.proto'
      - 'contracts/buf.gen.yaml'
      - 'ci/auditable-derive-drift/**/*'
```

**It must be able to fail.** `ci/auditable-derive-drift/run.sh --negative-control` runs the same
comparison against a doctored list and asserts it reds, mirroring `ci/affected-graph/run.sh`. This
repo has shipped vacuously-passing assertions twice (SMA-489's `# TYPE` line, SMA-466's `promtool
check config`); a gate that has never been observed failing is not evidence.

Implementation: `python3` parses the `.proto` files for `message <Name> {` blocks containing a field
whose type resolves to `AuditMetadata` (matching both the same-package `AuditMetadata audit = N;`
and the cross-package `paigasus.common.v1.AuditMetadata audit = N;` spellings), builds the
fully-qualified name from each file's `package` declaration, and diffs that set against the
`type_attribute=` entries parsed out of `buf.gen.yaml`.

## 7. Testing

| Level | Location | Covers |
|---|---|---|
| Unit | `paigasus-proto-derive/src/auditable.rs` `#[cfg(test)]` | `expand()` over parsed fixtures: happy path, generics forwarded, missing field, tuple struct, unit struct, enum. Asserts on the returned `Err`'s message, and that the happy-path expansion contains the impl header and `self.audit.as_ref()`. |
| UI | `paigasus-proto/tests/auditable_derive_ui.rs` + `tests/ui/*.rs\|stderr` | The rendered compile error, via `trybuild`. |
| Behavior | `paigasus-proto/tests/auditable_derive.rs` | All seven generated types satisfy `Auditable` (a generic `fn assert_auditable<T: Auditable>()` over each), and accessors read through / return `None` on absent audit for a representative type. |
| Regression | `paigasus-proto/src/audit.rs` `#[cfg(test)]` | The two SMA-425 tests, unchanged, now running against the derived impl. |
| Gate | `ci/auditable-derive-drift/run.sh --negative-control` | That the drift gate reds on a doctored list. |

**`trybuild` fixtures are restricted to macro-emitted diagnostics.** A fixture asserting on a rustc
type error would make the `.stderr` file rustc-version-sensitive and turn a toolchain bump into a
red gate. Our `compile_error!` text is emitted by our own code and is stable across rustc versions.

## 8. Repo plumbing

- `rs/Cargo.toml` `[workspace.dependencies]`: `syn = "3"`, `quote = "1"`, `proc-macro2 = "1"`,
  `trybuild = "1"`, and the in-tree `paigasus-proto-derive = { path = …, version = "0.0.0" }`,
  each with the comment-the-rationale style the table already uses.
- `ci/affected-graph/run.sh`: a new strict-equality case asserting a derive-crate edit reaches
  `paigasus-proto-derive-rs,paigasus-proto-rs,paigasus-gateway-rs,paigasus-iam-rs`. The existing
  `contracts->proto` case is expected to be unchanged — the derive crate is *upstream* of
  `paigasus-proto`, so a proto edit does not reach it. **Verify rather than assume**; strict
  equality reds either way.
- `.github/workflows/ci.yml`: add `:auditable-derive-drift` to the `moon ci` target list.
- `CLAUDE.md`: add the same gate to the documented full-graph command.
- `.github/CODEOWNERS` is Moon-generated — regenerate, never hand-edit.

Expected to pass unaided, to be confirmed rather than assumed: `:deny` (all new deps are
MIT/Apache-2.0), `:machete` (every new dep is directly consumed), and `:wasm-getrandom-free` (whose
inputs include `rs/Cargo.toml`/`Cargo.lock`, so it *will* run — proc-macro crates compile for the
host and never enter the wasm target's tree).

## 9. Risks

| Risk | Mitigation |
|---|---|
| `extern crate self as paigasus_proto;` looks like dead code and gets deleted | Comment at the line explaining F2; deleting it fails the build immediately, so the failure is loud. |
| The seven-entry list drifts | §6's gate, with a proven-failing negative control. |
| `trybuild` `.stderr` churns on toolchain bumps | Fixtures restricted to macro-emitted diagnostics (§7). |
| First proc-macro crate slows the build graph | `syn`/`quote`/`proc-macro2` are host-only and already ubiquitous in Rust builds; they enter this workspace's tree for the first time, so the cost is a one-time compile of three small crates. |
| Derive applied to a message whose `audit` field is later renamed in the proto | The derive hard-errors at compile time; the failure is loud, not silent. |

## 10. Out of scope

- Server-side audit **stamping** (`created_by`/`modified_by` are left empty by
  `convert.rs::audit()` today) — a separate follow-up.
- TypeScript and Python equivalents — ADR-0012: both satisfy the contract structurally.
- Any change to the `Auditable` trait's shape or its defaulted accessors.
- Making any consumer actually *call* the accessors.
- Flipping `publish = true` on `paigasus-proto` / `paigasus-proto-derive` (`TODO(SMA-388)`).

## 11. Acceptance criteria

1. `#[derive(Auditable)]` exists in `paigasus-proto-derive` and is re-exported as
   `paigasus_proto::audit::Auditable`.
2. It generates `impl Auditable` for a named struct with an `audit` field, forwarding generics.
3. It emits a clear, correctly-spanned `compile_error!` for a struct with no `audit` field, and for
   tuple structs, unit structs, enums, and unions.
4. All seven generated messages embedding `AuditMetadata` implement `Auditable` with no
   hand-written impl anywhere in the repository.
5. `repo:auditable-derive-drift` passes on the committed tree, and its `--negative-control` mode
   demonstrates it reds on a doctored list.
6. The full CI graph passes, including `:deny`, `:machete`, `:affected-smoke`, and the
   codegen-drift gate.
