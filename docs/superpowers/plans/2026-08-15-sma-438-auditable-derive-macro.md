# SMA-438 — `#[derive(Auditable)]` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a `#[derive(Auditable)]` proc macro and apply it, through the buf codegen pipeline, to the seven prost-generated messages that embed `AuditMetadata`.

**Architecture:** A new `proc-macro = true` crate `paigasus-proto-derive` exports the derive; `paigasus-proto` depends on it, re-exports it from `src/audit.rs`, and uses `extern crate self as paigasus_proto;` so the derive's absolute paths resolve against generated code inside that same crate. `contracts/buf.gen.yaml` injects the derive with seven `message_attribute=` opts. A `syn`-parsed test inside `paigasus-proto` enforces the biconditional "has an `Option<…AuditMetadata>` field named `audit` ⟺ carries the derive".

**Tech Stack:** Rust (edition 2024, rust-version 1.95), `syn` 3 / `quote` 1 / `proc-macro2` 1, `prost` 0.14 via `buf.build/community/neoeinstein-prost:v0.5.0`, Moon 2.3.2, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-08-15-sma-438-auditable-derive-macro-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for TOML/Python).
- Rust crates use **edition 2024 + rust-version 1.95**, inherited via `edition.workspace = true`.
- `rs/rustfmt.toml` sets `max_width = 200`. Run `cargo fmt` before every commit; `:fmt` is a CI gate.
- `[workspace.lints.rust] warnings = "deny"` — **dead code is a hard compile error**, so never add an item in one task and wire it up in a later one.
- Workspace dependency features are a **minimal baseline that unions across the workspace**; crates ADD features they need rather than the table enabling everything. Every new entry gets a comment explaining why it exists.
- Conventional commits with a workspace scope: `feat(rs):`, `feat(contracts):`, `docs(rs):`. Subject must **start lowercase** and be **≤100 chars**. Never write `#NNN` in a commit body (commitlint reads it as a footer) — write "owner/repo PR NNN".
- Do **not** bypass git hooks with `--no-verify`.
- The Bash tool's PATH lacks proto-managed CLIs. Prefix commands with:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`
- Work in the worktree `/Users/smaschek/dev/paigasus/paigasus-core-sma438` on branch `feature/sma-438-auditable-derive-macro`. Never `cd` to the main checkout.
- `.github/CODEOWNERS` is Moon-generated — never hand-edit.

## File Structure

| File | Responsibility |
|---|---|
| `rs/crates/libs/paigasus-proto-derive/Cargo.toml` | proc-macro crate manifest |
| `rs/crates/libs/paigasus-proto-derive/moon.yml` | Moon project `paigasus-proto-derive-rs` |
| `rs/crates/libs/paigasus-proto-derive/src/lib.rs` | `#[proc_macro_derive]` bridge only |
| `rs/crates/libs/paigasus-proto-derive/src/auditable.rs` | `expand()` — all logic + unit tests |
| `rs/Cargo.toml` | workspace dep entries |
| `rs/crates/libs/paigasus-proto/Cargo.toml` | dep on the derive crate; `syn` dev-dep |
| `rs/crates/libs/paigasus-proto/moon.yml` | `dependsOn` + task-level `^:build` |
| `rs/crates/libs/paigasus-proto/src/lib.rs` | `extern crate self as paigasus_proto;` |
| `rs/crates/libs/paigasus-proto/src/audit.rs` | re-export derive + `AuditMetadata`; drop the manual impl |
| `contracts/buf.gen.yaml` | seven `message_attribute=` opts |
| `rs/crates/libs/paigasus-proto/src/generated/**` | regenerated, committed |
| `rs/crates/libs/paigasus-proto/tests/auditable_derive.rs` | per-type read-through behavior |
| `rs/crates/libs/paigasus-proto/tests/auditable_derive_drift.rs` | the biconditional + negative fixtures |
| `ci/affected-graph/run.sh` | new strict-equality case |

---

### Task 1: The `paigasus-proto-derive` crate

Self-contained: the crate compiles and its tests pass without `paigasus-proto` existing as a dependent. `expand()` only builds tokens — it never compiles them — so the `::paigasus_proto::…` paths it emits do not need to resolve yet.

**Files:**
- Create: `rs/crates/libs/paigasus-proto-derive/Cargo.toml`
- Create: `rs/crates/libs/paigasus-proto-derive/moon.yml`
- Create: `rs/crates/libs/paigasus-proto-derive/src/lib.rs`
- Create: `rs/crates/libs/paigasus-proto-derive/src/auditable.rs`
- Modify: `rs/Cargo.toml` (`[workspace.dependencies]`)

**Interfaces:**
- Produces: `paigasus_proto_derive::Auditable` (derive macro). Internal:
  `pub(crate) fn auditable::expand(input: &syn::DeriveInput) -> syn::Result<proc_macro2::TokenStream>`,
  and the two error constants `E_SHAPE` / `E_FIELD`.

- [ ] **Step 1: Add the workspace dependency entries**

In `rs/Cargo.toml`, inside `[workspace.dependencies]`, add after the `proptest` entry:

```toml
# syn / quote / proc-macro2 — the proc-macro toolchain for `paigasus-proto-derive` (SMA-438's
# #[derive(Auditable)]). All three are ALREADY in Cargo.lock as transitive deps of async-trait
# and thiserror-impl (proc-macro2 1.0.106, quote 1.0.45, syn 2.0.117 AND syn 3.0.3), so this
# pins what is already resolved rather than adding to the build. `full` is needed by
# paigasus-proto's drift test, which calls `syn::parse_file` over the generated sources; the
# derive crate itself needs only the default features.
syn = { version = "3", features = ["full"] }
quote = "1"
proc-macro2 = "1"
# In-tree path dep: the derive crate that generates `impl Auditable` for DTOs embedding
# AuditMetadata. `paigasus-proto` re-exports it, and buf injects it onto the generated
# messages — same in-tree-freshness rationale as paigasus-kernel/paigasus-proto above.
# PUBLISH ORDER (SMA-388): this crate must publish BEFORE paigasus-proto, which depends on it.
paigasus-proto-derive = { path = "crates/libs/paigasus-proto-derive", version = "0.0.0" }
```

- [ ] **Step 2: Create the crate manifest**

`rs/crates/libs/paigasus-proto-derive/Cargo.toml`:

```toml
# SPDX-License-Identifier: Apache-2.0

[package]
name = "paigasus-proto-derive"
version = "0.0.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
# TODO(SMA-388): flip publish = true in lockstep with paigasus-proto. This crate must be
# published FIRST — paigasus-proto depends on it.
publish = false

[lib]
proc-macro = true

[dependencies]
syn.workspace = true
quote.workspace = true
proc-macro2.workspace = true

[lints]
workspace = true
```

- [ ] **Step 3: Create the Moon project file**

`rs/crates/libs/paigasus-proto-derive/moon.yml`:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-proto-derive-rs'
layer: 'library'
language: 'rust'
```

No `dependsOn` — this crate is a graph *root*. The edge is declared on its dependent
(`paigasus-proto`) in Task 2.

- [ ] **Step 4: Write `expand()` with its failing tests**

`rs/crates/libs/paigasus-proto-derive/src/auditable.rs` — write the tests **and** the
implementation together; a proc-macro crate with a `mod` that has no callers will not compile
under `warnings = "deny"`, so a test-only intermediate commit is not possible here.

```rust
// SPDX-License-Identifier: Apache-2.0

use proc_macro2::TokenStream;
use quote::quote;
use syn::ext::IdentExt;
use syn::{Data, DeriveInput, Fields};

/// Emitted when the annotated item is not a struct with named fields.
pub(crate) const E_SHAPE: &str = "#[derive(Auditable)] can only be applied to a struct with named fields";
/// Emitted when a named-field struct carries no field called `audit`.
///
/// Deliberately does NOT promise a type: the derive validates the field NAME only (spec D3),
/// and a wrongly-typed `audit` field is left to rustc, whose error is better spanned.
pub(crate) const E_FIELD: &str = "#[derive(Auditable)] requires a field named `audit`";

/// Builds `impl Auditable for #ident` for a struct carrying an `audit` field.
///
/// Split out of the `#[proc_macro_derive]` bridge in `lib.rs` on purpose: this signature takes
/// and returns plain `syn`/`proc-macro2` types, so it is callable from an ordinary `#[cfg(test)]`
/// module. The `proc_macro` crate's API only exists during a real macro expansion; `proc-macro2`'s
/// does not have that restriction.
pub(crate) fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => return Err(syn::Error::new_spanned(&input.ident, E_SHAPE)),
        },
        _ => return Err(syn::Error::new_spanned(&input.ident, E_SHAPE)),
    };

    // `unraw()` matters: a raw ident stringifies as "r#audit", so a bare `i == "audit"` would
    // reject `r#audit`. Unreachable from prost today (`audit` is not a Rust keyword) but cheap.
    let has_audit = fields.iter().any(|f| f.ident.as_ref().is_some_and(|i| i.unraw() == "audit"));
    if !has_audit {
        return Err(syn::Error::new_spanned(&input.ident, E_FIELD));
    }

    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Every path is fully qualified — including ::core::option::Option — so the impl compiles in
    // a module that has shadowed any of these names. `::paigasus_proto` resolves inside
    // paigasus-proto itself thanks to its `extern crate self as paigasus_proto;` (spec F2).
    Ok(quote! {
        impl #impl_generics ::paigasus_proto::audit::Auditable for #ident #ty_generics #where_clause {
            fn audit(&self) -> ::core::option::Option<&::paigasus_proto::audit::AuditMetadata> {
                self.audit.as_ref()
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{E_FIELD, E_SHAPE, expand};
    use syn::{DeriveInput, ItemImpl};

    fn expand_str(src: &str) -> syn::Result<ItemImpl> {
        let input: DeriveInput = syn::parse_str(src).expect("fixture must parse");
        // Parse the expansion STRUCTURALLY. Never assert with `contains(..)` on
        // `TokenStream::to_string()`: it renders as `self . audit . as_ref ()`, and the obvious
        // "fix" of loosening the needle to "audit" is then satisfied by the emitted path
        // `::paigasus_proto::audit::Auditable` alone — i.e. it would pass with the method body
        // deleted. This repo has shipped that exact class of vacuous assertion twice.
        expand(&input).map(|ts| syn::parse2::<ItemImpl>(ts).expect("expansion must be an impl block"))
    }

    fn err_message(src: &str) -> String {
        expand_str(src).expect_err("expected a rejection").to_string()
    }

    /// The full expansion, compared against a reference token stream.
    #[test]
    fn emits_the_expected_impl() {
        let got = expand_str("struct Organization { prn: String, audit: Option<AuditMetadata> }").unwrap();
        let want: ItemImpl = syn::parse_quote! {
            impl ::paigasus_proto::audit::Auditable for Organization {
                fn audit(&self) -> ::core::option::Option<&::paigasus_proto::audit::AuditMetadata> {
                    self.audit.as_ref()
                }
            }
        };
        assert_eq!(
            quote::quote!(#got).to_string(),
            quote::quote!(#want).to_string(),
            "expansion drifted from the reference impl"
        );
    }

    /// The body must read the field. Pinned separately so a regression to `{ None }` — which
    /// would still parse as a valid ItemImpl — cannot slip through.
    #[test]
    fn body_reads_the_audit_field() {
        let got = expand_str("struct S { audit: Option<AuditMetadata> }").unwrap();
        let method = got.items.first().expect("impl must contain the audit method");
        let syn::ImplItem::Fn(f) = method else { panic!("expected a method, got {method:?}") };
        assert_eq!(f.sig.ident, "audit");
        let body: syn::Expr = syn::parse_quote!(self.audit.as_ref());
        let actual = f.block.stmts.first().expect("method body must not be empty");
        assert_eq!(
            quote::quote!(#actual).to_string(),
            quote::quote!(#body).to_string(),
            "method body must be exactly `self.audit.as_ref()`"
        );
    }

    #[test]
    fn forwards_generics_defaults_lifetimes_and_where_clauses() {
        let got = expand_str(
            "struct S<'a, T: Clone, U = ()> where U: Default { audit: Option<AuditMetadata>, borrowed: &'a T, other: U }",
        )
        .unwrap();
        let rendered = quote::quote!(#got).to_string();
        // Defaults must NOT appear in the impl header — `impl<U = ()>` is not valid Rust.
        assert!(!rendered.contains('='), "generic defaults leaked into the impl header: {rendered}");
        assert!(rendered.contains("where"), "where clause was dropped: {rendered}");
        let want: syn::ItemImpl = syn::parse_quote! {
            impl<'a, T: Clone, U> ::paigasus_proto::audit::Auditable for S<'a, T, U>
            where
                U: Default,
            {
                fn audit(&self) -> ::core::option::Option<&::paigasus_proto::audit::AuditMetadata> {
                    self.audit.as_ref()
                }
            }
        };
        assert_eq!(rendered, quote::quote!(#want).to_string());
    }

    #[test]
    fn accepts_a_raw_ident_audit_field() {
        // `r#audit` stringifies as "r#audit"; only the `unraw()` comparison accepts it.
        expand_str("struct S { r#audit: Option<AuditMetadata> }").expect("r#audit must be accepted");
    }

    #[test]
    fn rejects_struct_without_an_audit_field() {
        assert_eq!(err_message("struct S { prn: String }"), E_FIELD);
    }

    #[test]
    fn rejects_struct_with_zero_fields() {
        assert_eq!(err_message("struct S {}"), E_FIELD);
    }

    #[test]
    fn rejects_a_similarly_named_field() {
        // Guards the name check against being loosened to a substring match.
        assert_eq!(err_message("struct S { audit_log: Option<AuditMetadata> }"), E_FIELD);
    }

    #[test]
    fn rejects_tuple_struct() {
        assert_eq!(err_message("struct S(Option<AuditMetadata>);"), E_SHAPE);
    }

    #[test]
    fn rejects_unit_struct() {
        assert_eq!(err_message("struct S;"), E_SHAPE);
    }

    #[test]
    fn rejects_enum() {
        assert_eq!(err_message("enum E { A { audit: Option<AuditMetadata> } }"), E_SHAPE);
    }

    #[test]
    fn rejects_union() {
        assert_eq!(err_message("union U { audit: u8 }"), E_SHAPE);
    }
}
```

- [ ] **Step 5: Write the proc-macro bridge**

`rs/crates/libs/paigasus-proto-derive/src/lib.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Derive macros for the traits in `paigasus-proto`.
//!
//! Re-exported from `paigasus_proto::audit`, which is how consumers should reach them — a
//! direct dependency on this crate is never needed. The generated code names
//! `::paigasus_proto::…` by absolute path, so any crate applying a derive from here must
//! depend on `paigasus-proto` (otherwise: `E0433: could not find paigasus_proto`).

mod auditable;

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

/// Implements `paigasus_proto::audit::Auditable` for a struct carrying an `audit` field.
///
/// ```ignore
/// #[derive(Auditable)]
/// struct Organization {
///     prn: String,
///     audit: Option<AuditMetadata>,
/// }
/// ```
///
/// Errors at compile time when the annotated type is not a struct with named fields, or has no
/// field named `audit`. The field's *type* is not checked — a wrong type surfaces as an ordinary
/// rustc type error at the generated impl.
#[proc_macro_derive(Auditable)]
pub fn derive_auditable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    auditable::expand(&input).unwrap_or_else(syn::Error::into_compile_error).into()
}
```

- [ ] **Step 6: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-proto-derive
```

Expected: **11 tests pass**. If `emits_the_expected_impl` fails on whitespace, the assertion is
comparing two `quote!` renderings — both sides go through the same renderer, so a mismatch is a
real difference, not formatting. Do not "fix" it by relaxing to `contains`.

- [ ] **Step 7: Lint and format**

```bash
cd rs && cargo fmt && cargo clippy -p paigasus-proto-derive --all-targets -- -D warnings
```

Expected: clean. `cargo fmt` may reflow the long `E_SHAPE` line — that is fine (`max_width = 200`).

- [ ] **Step 8: Commit**

```bash
git add rs/Cargo.toml rs/Cargo.lock rs/crates/libs/paigasus-proto-derive
git commit -m "feat(rs): add the paigasus-proto-derive crate with #[derive(Auditable)] (SMA-438)"
```

---

### Task 2: Wire the derive into `paigasus-proto`

Proves F2/F3/F6 inside the real crate, against a hand-written struct — before any generated code
depends on it. If this task passes, the injection in Task 3 is mechanical.

**Files:**
- Modify: `rs/crates/libs/paigasus-proto/Cargo.toml`
- Modify: `rs/crates/libs/paigasus-proto/moon.yml`
- Modify: `rs/crates/libs/paigasus-proto/src/lib.rs`
- Modify: `rs/crates/libs/paigasus-proto/src/audit.rs`
- Create: `rs/crates/libs/paigasus-proto/tests/auditable_derive.rs`

**Interfaces:**
- Consumes: `paigasus_proto_derive::Auditable` from Task 1.
- Produces: `paigasus_proto::audit::Auditable` (trait **and** derive, same name, different
  namespaces) and `paigasus_proto::audit::AuditMetadata` (re-export). These two paths are exactly
  what Task 3's `buf.gen.yaml` opts and Task 4's drift test refer to.

- [ ] **Step 1: Add the dependency**

In `rs/crates/libs/paigasus-proto/Cargo.toml`, under `[dependencies]`:

```toml
# paigasus-proto-derive: #[derive(Auditable)], re-exported from `audit` and injected onto the
# generated audit-bearing messages by contracts/buf.gen.yaml (SMA-438). NOT optional — the
# generated code names it unconditionally, so a feature gate would break codegen.
paigasus-proto-derive.workspace = true
```

- [ ] **Step 2: Declare the Moon edge**

Replace the `tasks:` block in `rs/crates/libs/paigasus-proto/moon.yml` so the file reads:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-proto-rs'
layer: 'library'
language: 'rust'

# Cargo path deps are NOT auto-synced into Moon's graph — every in-tree edge is hand-declared.
# The task-level `^:build` is what propagates `affected` under `moon ci --include-relations`;
# a project `dependsOn` alone does not (SMA-389 D3). Without both, a paigasus-proto-derive edit
# would not rebuild this crate and `:affected-smoke` would red on the SMA-438 case.
dependsOn:
  - 'paigasus-proto-derive-rs'

tasks:
  build:
    deps: ['contracts:generate', '^:build']
  test:
    deps: ['contracts:generate', '^:build']
```

- [ ] **Step 3: Add `extern crate self`**

In `rs/crates/libs/paigasus-proto/src/lib.rs`, immediately after the `//!` doc block and before
`pub mod paigasus {`:

```rust
// Lets the ABSOLUTE path `::paigasus_proto::…` resolve INSIDE this crate. The Auditable derive
// is injected onto generated messages here (contracts/buf.gen.yaml), and a derive crate cannot
// depend on paigasus-proto — that is the cycle — so its output must name the trait absolutely.
// Same trick serde uses. THIS LINE LOOKS REMOVABLE AND IS NOT: deleting it breaks every
// generated `#[derive(::paigasus_proto::audit::Auditable)]` (SMA-438 F2).
extern crate self as paigasus_proto;
```

- [ ] **Step 4: Re-export the derive and `AuditMetadata`**

In `rs/crates/libs/paigasus-proto/src/audit.rs`, replace line 2
(`use crate::paigasus::common::v1::AuditMetadata;`) with:

```rust
// Re-exported so the derive's generated code has ONE stable anchor to name
// (`::paigasus_proto::audit::AuditMetadata`) instead of the codegen module layout, which
// `clean: true` regenerates (SMA-438 D5).
pub use crate::paigasus::common::v1::AuditMetadata;
// The derive and the trait below deliberately share a name: macros live in the macro namespace,
// traits in the type namespace, so `use paigasus_proto::audit::Auditable;` imports BOTH. This is
// the `serde::Serialize` pattern (SMA-438 F3).
pub use paigasus_proto_derive::Auditable;
```

- [ ] **Step 5: Write the end-to-end test**

`rs/crates/libs/paigasus-proto/tests/auditable_derive.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! End-to-end coverage for `#[derive(Auditable)]` (SMA-438).
//!
//! Task 2 covers a hand-written struct — proving the re-export, the absolute paths, and
//! `extern crate self` all line up. Task 3 extends this file to the generated messages.

use paigasus_proto::audit::{AuditMetadata, Auditable};

#[derive(Auditable, Default)]
struct HandWritten {
    #[allow(dead_code)]
    prn: String,
    audit: Option<AuditMetadata>,
}

#[test]
fn derived_impl_reads_through_to_embedded_metadata() {
    let dto = HandWritten {
        prn: "p".to_string(),
        audit: Some(AuditMetadata { created_by: "svc".to_string(), ..Default::default() }),
    };
    // A sentinel value, not just `is_some()` — a derive emitting `{ None }` must fail here.
    assert_eq!(dto.created_by(), Some("svc"));
    // Empty actor is a meaningful value (unknown/system), distinct from absent audit.
    assert_eq!(dto.modified_by(), Some(""));
    assert_eq!(dto.created_at(), None);
}

#[test]
fn absent_audit_yields_none_accessors() {
    let dto = HandWritten::default();
    assert_eq!(dto.audit(), None);
    assert_eq!(dto.created_by(), None);
    assert_eq!(dto.modified_at(), None);
}
```

- [ ] **Step 6: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-proto
```

Expected: PASS, including the two pre-existing `audit::tests` cases.

If you see `error[E0433]: failed to resolve: use of undeclared crate or module 'paigasus_proto'`,
Step 3 is missing or misplaced — `extern crate self as …` must be at the crate root, not inside a
module.

- [ ] **Step 7: Verify the Moon edge resolves**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects --affected --downstream deep <<< "rs/crates/libs/paigasus-proto-derive/src/lib.rs"
```

Expected: the JSON includes `paigasus-proto-derive-rs`, `paigasus-proto-rs`, `paigasus-gateway-rs`,
`paigasus-iam-rs` (plus `repo`). If `paigasus-proto-rs` is absent, Step 2 did not take effect.

- [ ] **Step 8: Lint, format, commit**

```bash
cd rs && cargo fmt && cargo clippy -p paigasus-proto --all-targets -- -D warnings
cd .. && git add rs/ && git commit -m "feat(rs): re-export #[derive(Auditable)] from paigasus-proto::audit (SMA-438)"
```

---

### Task 3: Inject the derive into the generated messages

**Files:**
- Modify: `contracts/buf.gen.yaml`
- Modify: `rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs` (regenerated)
- Modify: `rs/crates/libs/paigasus-proto/src/generated/paigasus/iam/v1/paigasus.iam.v1.rs` (regenerated)
- Modify: `rs/crates/libs/paigasus-proto/src/audit.rs` (remove the manual impl)
- Modify: `rs/crates/libs/paigasus-proto/tests/auditable_derive.rs` (add the seven-type coverage)

**Interfaces:**
- Consumes: `paigasus_proto::audit::{Auditable, AuditMetadata}` from Task 2.
- Produces: `impl Auditable` on `common::v1::AuditableExample` and
  `iam::v1::{Organization, Team, Project, Membership, ServiceAccount, ApiKey}`.

- [ ] **Step 1: Add the seven injection opts**

In `contracts/buf.gen.yaml`, extend the **first** plugin block (`neoeinstein-prost`) only. Its
`opt:` list becomes:

```yaml
  - remote: buf.build/community/neoeinstein-prost:v0.5.0
    out: ../rs/crates/libs/paigasus-proto/src/generated
    opt:
      - bytes=.
      - file_descriptor_set
      # SMA-438 — inject #[derive(Auditable)] onto every message embedding AuditMetadata, so
      # paigasus-proto ships the impl instead of each consumer hand-writing it.
      #
      # `message_attribute`, NOT `type_attribute`: prost's path matching is prefix-based over
      # SUB-paths, so a type_attribute on a message also hits types nested inside it — including
      # the enum prost synthesizes for a `oneof`. Probed on ListMembershipsRequest: type_attribute
      # landed the derive on both the struct and its `Filter` oneof enum, which would then fail
      # the derive's own shape check inside generated code. message_attribute hits structs only.
      #
      # Each entry MUST be quoted: unquoted, `#[` starts a YAML comment unless `#` is immediately
      # preceded by `=`, so one stray space silently truncates the opt. And the attribute must
      # contain NO comma — buf joins these entries into one comma-separated plugin parameter, so
      # a future `#[derive(A, B)]` needs two separate lines for the same message.
      #
      # Keep in sync with the biconditional asserted by paigasus-proto's
      # tests/auditable_derive_drift.rs, which fails if a message gains an `audit` field without
      # a line here (or keeps a line here after losing the field).
      - 'message_attribute=.paigasus.common.v1.AuditableExample=#[derive(::paigasus_proto::audit::Auditable)]'
      - 'message_attribute=.paigasus.iam.v1.Organization=#[derive(::paigasus_proto::audit::Auditable)]'
      - 'message_attribute=.paigasus.iam.v1.Team=#[derive(::paigasus_proto::audit::Auditable)]'
      - 'message_attribute=.paigasus.iam.v1.Project=#[derive(::paigasus_proto::audit::Auditable)]'
      - 'message_attribute=.paigasus.iam.v1.Membership=#[derive(::paigasus_proto::audit::Auditable)]'
      - 'message_attribute=.paigasus.iam.v1.ServiceAccount=#[derive(::paigasus_proto::audit::Auditable)]'
      - 'message_attribute=.paigasus.iam.v1.ApiKey=#[derive(::paigasus_proto::audit::Auditable)]'
```

- [ ] **Step 2: Record the pre-regeneration baseline**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
grep -c . rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs
git rev-parse HEAD:py/packages/paigasus-proto/src/paigasus_proto/generated
git rev-parse HEAD:ts/packages/paigasus-proto/src/generated
```

Note the two tree hashes — Step 4 asserts they are unchanged.

- [ ] **Step 3: Regenerate**

Run `buf generate` **directly**, not `moon run contracts:generate`: that task declares no
`outputs:`, so Moon can serve stale cached output (CLAUDE.md gotcha).

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf generate && cd ..
```

- [ ] **Step 4: Verify the diff is exactly what the spec predicts**

```bash
git diff --stat
git diff -- rs/crates/libs/paigasus-proto/src/generated | grep '^[+-]' | grep -v '^[+-][+-]'
git rev-parse HEAD:py/packages/paigasus-proto/src/paigasus_proto/generated
git rev-parse HEAD:ts/packages/paigasus-proto/src/generated
```

Expected, and all four are acceptance criteria — **stop and investigate if any differs**:
1. Exactly **7 added lines**, every one `#[derive(::paigasus_proto::audit::Auditable)]`, and **zero deleted lines**.
2. No change under `py/packages/paigasus-proto/` or `ts/packages/paigasus-proto/` (`opt` is per-plugin). The two tree hashes match Step 2's.
3. `FILE_DESCRIPTOR_SET` unchanged in all three generated Rust files — it is a function of the `.proto` sources, which this change does not touch. Confirm with:
   `git diff -- rs/crates/libs/paigasus-proto/src/generated | grep -c FILE_DESCRIPTOR_SET` → expect `0`.
4. The 7 lines land on `AuditableExample`, `Organization`, `Team`, `Project`, `Membership`, `ServiceAccount`, `ApiKey` — and on **no `pub enum`**. Confirm with:
   `grep -A2 'derive(::paigasus_proto' rs/crates/libs/paigasus-proto/src/generated/paigasus/*/v1/*.rs | grep 'pub enum'` → expect **no output**.

- [ ] **Step 5: Remove the now-conflicting manual impl**

The build is broken at this point with `error[E0119]: conflicting implementations of trait
'Auditable' for type 'AuditableExample'` — expected, and the reason this step shares a commit with
Step 1. In `rs/crates/libs/paigasus-proto/src/audit.rs`, delete the manual impl and its comment
from the `mod tests` block:

```rust
    // Conformance impl on the *generated* embedding fixture — proves the trait works
    // over `AuditableExample.audit: Option<AuditMetadata>` produced by codegen. The
    // orphan rule blocks this from an integration test crate (neither item is local
    // there), so it lives in-crate under cfg(test).
    impl Auditable for AuditableExample {
        fn audit(&self) -> Option<&AuditMetadata> {
            self.audit.as_ref()
        }
    }
```

Replace it with:

```rust
    // No manual impl here any more: `AuditableExample` now carries #[derive(Auditable)] via
    // codegen (SMA-438), so the two tests below exercise the DERIVED impl. Re-adding a manual
    // one is an E0119 conflict. Note this makes the fixture's impl public API, reversing
    // SMA-425's decision to keep it test-only — deliberate, see SMA-438 spec D8.
```

Keep both `#[test]` functions and the `use` lines exactly as they are.

- [ ] **Step 6: Extend the behavior test to all seven types**

Append to `rs/crates/libs/paigasus-proto/tests/auditable_derive.rs`:

```rust
// ─── Generated messages (SMA-438) ────────────────────────────────────────────────────────────
//
// Each type is built with a DISTINCT sentinel in `created_by` and asserted to return exactly
// that. A bare `fn assert_auditable<T: Auditable>()` bound would prove only that an impl
// EXISTS — a derive emitting `{ None }` would satisfy it for six of the seven types.

use paigasus_proto::paigasus::common::v1::AuditableExample;
use paigasus_proto::paigasus::iam::v1::{ApiKey, Membership, Organization, Project, ServiceAccount, Team};

fn stamped(actor: &str) -> Option<AuditMetadata> {
    Some(AuditMetadata { created_by: actor.to_string(), ..Default::default() })
}

macro_rules! generated_type_reads_through {
    ($($name:ident => $ty:ty),+ $(,)?) => {$(
        #[test]
        fn $name() {
            let sentinel = stringify!($ty);
            let dto = <$ty> { audit: stamped(sentinel), ..Default::default() };
            assert_eq!(dto.created_by(), Some(sentinel), "derived impl did not read the audit field");
            assert_eq!(dto.audit().map(|a| a.created_by.as_str()), Some(sentinel));

            let empty = <$ty>::default();
            assert_eq!(empty.audit(), None, "absent audit must yield None");
            assert_eq!(empty.created_by(), None);
            assert_eq!(empty.modified_at(), None);
        }
    )+};
}

generated_type_reads_through! {
    auditable_example_reads_through => AuditableExample,
    organization_reads_through      => Organization,
    team_reads_through              => Team,
    project_reads_through           => Project,
    membership_reads_through        => Membership,
    service_account_reads_through   => ServiceAccount,
    api_key_reads_through           => ApiKey,
}
```

- [ ] **Step 7: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-proto
```

Expected: the 2 hand-written tests, the 7 generated-type tests, and the 2 pre-existing
`audit::tests` cases all pass.

If `<$ty> { audit: …, ..Default::default() }` fails to compile because a type does not derive
`Default`, check the generated struct — prost derives `Default` on every message, so a failure
here means the wrong type name.

- [ ] **Step 8: Lint, format, commit**

```bash
cd rs && cargo fmt && cargo clippy -p paigasus-proto --all-targets -- -D warnings
cd .. && git add contracts/buf.gen.yaml rs/
git commit -m "feat(contracts): derive Auditable on the seven messages embedding AuditMetadata (SMA-438)"
```

---

### Task 4: The drift test

**Files:**
- Modify: `rs/crates/libs/paigasus-proto/Cargo.toml` (`syn` dev-dep)
- Create: `rs/crates/libs/paigasus-proto/tests/auditable_derive_drift.rs`

**Interfaces:**
- Consumes: the committed `src/generated/**` produced by Task 3.
- Produces: nothing other crates use.

- [ ] **Step 1: Add the dev-dependency**

In `rs/crates/libs/paigasus-proto/Cargo.toml`, add after `[dependencies]`:

```toml
[dev-dependencies]
# syn: used ONLY by tests/auditable_derive_drift.rs, which parses the committed generated
# sources to assert that every message embedding AuditMetadata carries #[derive(Auditable)].
# Parsing (not regex) is what makes that check immune to comments, oneofs, nested messages and
# the several legal spellings of a cross-package type reference.
syn.workspace = true
```

**Do not add `quote` here.** The drift test never calls `quote!` — it compares path segments as
strings — and an unused dev-dep reds `:machete` in Task 5.

**Do not add `syn`'s `extra-traits` feature here either.** It gates `Debug`/`PartialEq`/`Hash` for
the whole `syn` AST, and this test only ever formats its own `Finding` struct (which derives
`Debug`). Task 1 needed that feature solely because one test's panic message formatted a
`syn::ImplItem` with `{:?}`; nothing below does.

- [ ] **Step 2: Write the drift test**

`rs/crates/libs/paigasus-proto/tests/auditable_derive_drift.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Drift guard for the `#[derive(Auditable)]` injection list (SMA-438).
//!
//! Asserts a biconditional over every struct in the committed generated sources:
//!
//!     has a field `audit: Option<…AuditMetadata>`  ⟺  carries #[derive(…audit::Auditable)]
//!
//! Both directions matter. Left-to-right catches a new audit-bearing proto message that nobody
//! added a `message_attribute=` line for in contracts/buf.gen.yaml — it would silently ship with
//! no impl. Right-to-left catches a stale line left behind after a field was removed.
//!
//! This is an ordinary crate test rather than a repo-level Moon gate on purpose: the files it
//! reads live inside this crate's own project directory, so a regeneration makes the crate
//! affected directly. (The `ops/`-reading guards — observability-drift, nats-permissions —
//! needed a `repo`-scoped task precisely BECAUSE their inputs sit outside any crate.)
//!
//! It parses with `syn` rather than matching text: `audit.proto` contains the literal string
//! "AuditMetadata audit = N;" inside a COMMENT three lines above `message AuditMetadata {`, and
//! a text scan flags AuditMetadata as embedding itself.

use std::path::{Path, PathBuf};

/// The derive the generated code must carry, as `syn` renders its path segments (the leading
/// `::` is not part of any segment).
const EXPECTED_DERIVE: &str = "paigasus_proto::audit::Auditable";

#[derive(Debug, PartialEq, Eq)]
struct Finding {
    ty: String,
    has_audit_field: bool,
    has_derive: bool,
}

impl Finding {
    fn is_consistent(&self) -> bool {
        self.has_audit_field == self.has_derive
    }
}

/// True when `ty` is `Option<…>` whose innermost argument path ends in `AuditMetadata`.
fn is_option_of_audit_metadata(ty: &syn::Type) -> bool {
    let syn::Type::Path(tp) = ty else { return false };
    let Some(last) = tp.path.segments.last() else { return false };
    if last.ident != "Option" {
        return false;
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else { return false };
    args.args.iter().any(|arg| {
        let syn::GenericArgument::Type(syn::Type::Path(inner)) = arg else { return false };
        inner.path.segments.last().is_some_and(|s| s.ident == "AuditMetadata")
    })
}

fn carries_expected_derive(attrs: &[syn::Attribute]) -> bool {
    let mut found = false;
    for attr in attrs {
        if !attr.path().is_ident("derive") {
            continue;
        }
        // parse_nested_meta walks `#[derive(A, b::C)]` one path at a time.
        let _ = attr.parse_nested_meta(|meta| {
            let rendered =
                meta.path.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::");
            if rendered == EXPECTED_DERIVE {
                found = true;
            }
            Ok(())
        });
    }
    found
}

fn audit(src: &str) -> Vec<Finding> {
    let file = syn::parse_file(src).expect("generated source must parse");
    file.items
        .iter()
        .filter_map(|item| {
            let syn::Item::Struct(s) = item else { return None };
            let syn::Fields::Named(named) = &s.fields else { return None };
            let has_audit_field = named
                .named
                .iter()
                .any(|f| f.ident.as_ref().is_some_and(|i| i == "audit") && is_option_of_audit_metadata(&f.ty));
            let has_derive = carries_expected_derive(&s.attrs);
            // Only structs that are interesting in EITHER direction.
            (has_audit_field || has_derive).then(|| Finding {
                ty: s.ident.to_string(),
                has_audit_field,
                has_derive,
            })
        })
        .collect()
}

fn generated_sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("generated dir must be readable") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/generated");
    let mut out = Vec::new();
    walk(&root, &mut out);
    out.sort();
    assert!(!out.is_empty(), "found no generated sources under {}", root.display());
    out
}

#[test]
fn every_audit_bearing_generated_struct_carries_the_derive() {
    let mut inconsistent = Vec::new();
    let mut total = 0usize;
    for path in generated_sources() {
        let src = std::fs::read_to_string(&path).expect("generated source must be readable");
        for finding in audit(&src) {
            total += 1;
            if !finding.is_consistent() {
                inconsistent.push(format!("{}: {finding:?}", path.display()));
            }
        }
    }
    assert!(
        inconsistent.is_empty(),
        "generated code and contracts/buf.gen.yaml disagree.\n\
         An `audit` field without the derive means a message_attribute= line is MISSING;\n\
         the derive without an `audit` field means a line is STALE.\n{}",
        inconsistent.join("\n")
    );
    // Guards against the whole check passing vacuously if the walk or the parse silently
    // yields nothing. Seven messages embed AuditMetadata as of SMA-438.
    assert_eq!(total, 7, "expected exactly 7 audit-bearing generated structs, found {total}");
}

// ─── Negative controls ───────────────────────────────────────────────────────────────────────
//
// These run on EVERY CI run, not behind an opt-in flag. `affected-smoke` invokes
// ci/affected-graph/run.sh WITHOUT --negative-control, which makes its control a manual
// affordance that proves nothing in CI; this repo has shipped vacuously-passing assertions
// twice (SMA-489's `# TYPE` line, SMA-466's `promtool check config`).

const WITH_BOTH: &str = r#"
    #[derive(::paigasus_proto::audit::Auditable)]
    #[derive(Clone, ::prost::Message)]
    pub struct Good { pub audit: ::core::option::Option<AuditMetadata> }
"#;

const FIELD_WITHOUT_DERIVE: &str = r#"
    #[derive(Clone, ::prost::Message)]
    pub struct MissingLine { pub audit: ::core::option::Option<AuditMetadata> }
"#;

const DERIVE_WITHOUT_FIELD: &str = r#"
    #[derive(::paigasus_proto::audit::Auditable)]
    #[derive(Clone, ::prost::Message)]
    pub struct StaleLine { pub prn: ::prost::alloc::string::String }
"#;

#[test]
fn control_accepts_a_correctly_injected_struct() {
    let found = audit(WITH_BOTH);
    assert_eq!(found.len(), 1);
    assert!(found[0].is_consistent(), "{found:?}");
}

#[test]
fn control_rejects_an_audit_field_with_no_derive() {
    let found = audit(FIELD_WITHOUT_DERIVE);
    assert_eq!(found.len(), 1);
    assert!(!found[0].is_consistent(), "a missing message_attribute= line must be detected");
}

#[test]
fn control_rejects_a_derive_with_no_audit_field() {
    let found = audit(DERIVE_WITHOUT_FIELD);
    assert_eq!(found.len(), 1);
    assert!(!found[0].is_consistent(), "a stale message_attribute= line must be detected");
}

#[test]
fn control_ignores_unrelated_structs_and_comment_text() {
    // The `audit.proto` hazard, in Rust form: doc comments carrying the literal field text, and
    // a struct whose `audit`-ish field is the WRONG type or shape.
    let src = r#"
        /// Carried by embedding this message as a field (`AuditMetadata audit = N;`).
        #[derive(Clone, ::prost::Message)]
        pub struct AuditMetadata { pub created_by: ::prost::alloc::string::String }

        #[derive(Clone, ::prost::Message)]
        pub struct Unrelated { pub audit_log: ::core::option::Option<AuditMetadata> }

        #[derive(Clone, ::prost::Message)]
        pub struct Repeated { pub audit: ::prost::alloc::vec::Vec<AuditMetadata> }
    "#;
    assert_eq!(audit(src), Vec::new(), "none of these should be treated as audit-bearing");
}
```

- [ ] **Step 3: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-proto
```

Expected: all pass, including the 4 negative controls and the `total == 7` count.

- [ ] **Step 4: Prove the guard actually bites**

Temporarily delete one injection line and confirm the test reds — a guard never observed failing
against the real tree is not evidence.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 - <<'PY'
import re, pathlib
p = pathlib.Path("contracts/buf.gen.yaml")
s = p.read_text()
p.write_text(re.sub(r"^.*message_attribute=\.paigasus\.iam\.v1\.Team=.*\n", "", s, flags=re.M))
PY
cd contracts && buf generate && cd ..
cd rs && cargo nextest run --no-tests=pass -p paigasus-proto --test auditable_derive_drift
```

Expected: **FAIL**, naming `Team` with `has_audit_field: true, has_derive: false`.

Then restore and re-verify:

```bash
git checkout -- contracts/buf.gen.yaml
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf generate && cd ..
git status --short   # expect: only the new/modified files from this task
cd rs && cargo nextest run --no-tests=pass -p paigasus-proto
```

Expected: PASS, and `git status` shows no leftover change under `src/generated/`.

**Note:** `cargo` decides freshness by mtime. If the restored generated file keeps an older mtime
than the build, cargo may reuse the binary built from the doctored source and the test will keep
failing against correct code. If that happens, `touch rs/crates/libs/paigasus-proto/src/generated/paigasus/iam/v1/paigasus.iam.v1.rs`
and re-run.

- [ ] **Step 5: Lint, format, commit**

```bash
cd rs && cargo fmt && cargo clippy -p paigasus-proto --all-targets -- -D warnings
cd .. && git add rs/ && git commit -m "test(rs): guard the Auditable injection list against drift (SMA-438)"
```

---

### Task 5: Affected-graph case and full-graph verification

**Files:**
- Modify: `ci/affected-graph/run.sh`
- Modify: `.github/CODEOWNERS` (regenerated, not hand-edited)

**Interfaces:**
- Consumes: the Moon project `paigasus-proto-derive-rs` and the edge declared in Task 2.

- [ ] **Step 1: Add the strict-equality case**

In `ci/affected-graph/run.sh`, inside `run_suite()`, immediately after the `contracts->proto`
case (around line 107), add:

```bash
  # derive-crate edit -> the derive crate + paigasus-proto and everything downstream of it
  # (SMA-438). One-directional w.r.t. contracts: the derive crate is strictly UPSTREAM of
  # paigasus-proto, so a proto edit must NOT reach it — enforced implicitly by the strict
  # equality of the contracts->proto case above, which lists no derive crate.
  run_case "proto-derive->proto" "rs/crates/libs/paigasus-proto-derive/src/lib.rs" \
    "paigasus-proto-derive-rs,paigasus-proto-rs,paigasus-gateway-rs,paigasus-iam-rs"
```

- [ ] **Step 2: Run the guard, including its negative control**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/affected-graph/run.sh
bash ci/affected-graph/run.sh --negative-control
```

Expected: every case PASSes — including the six pre-existing ones, whose expected sets must stay
byte-identical (the derive crate is upstream, so `--downstream deep` never reaches it). The
negative control reports OK on both wrong expectations.

If `contracts->proto` now reds with `unexpected: paigasus-proto-derive-rs`, the Task 2 edge was
declared in the wrong direction — `dependsOn` belongs on `paigasus-proto`, naming the derive crate.

- [ ] **Step 3: Regenerate CODEOWNERS**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon sync codeowners
git diff --stat .github/CODEOWNERS
```

Expected: either no change, or a single added line for the new project. Never hand-edit this file.

- [ ] **Step 4: Run the full CI graph**

Per-project tasks do not run the repo-level gates. Run the whole thing exactly as CI does:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: all green. Moon reports failures without attributing them — diagnose any "N failed" with:

```bash
jq '.actions[] | select(.status=="failed") | .label' .moon/cache/ciReport.json
```

Known things to watch, each with its remedy:
- **`:deny`** — new transitive licenses. If it reds, add a scoped `[licenses] exceptions` entry to `rs/deny.toml` with a comment, not a blanket `allow`.
- **`:machete`** — `proc-macro2` may be reachable only through `quote!`'s inferred return type and get flagged as unused. If so, add a `[package.metadata.cargo-machete] ignored = ["proc-macro2"]` block to `paigasus-proto-derive/Cargo.toml` with a comment; three crates already carry such waivers (`paigasus-wasm/Cargo.toml:24-29` and the two binding crates).
- **`:fmt`** — `cargo fmt --check` is whole-workspace. Run `cd rs && cargo fmt` and re-run.
- **`contracts:fmt`** — only fires if a `.proto` changed; this task changes none.
- **`:breaking`** — `buf breaking` compares proto sources only; unaffected by plugin opts.

- [ ] **Step 5: Commit**

```bash
git add ci/affected-graph/run.sh .github/CODEOWNERS
git commit -m "test(ci): assert the paigasus-proto-derive affected-graph edge (SMA-438)"
```

- [ ] **Step 6: Final sanity check**

```bash
git log --oneline origin/main..HEAD
git diff origin/main --stat
grep -rn "impl .*Auditable for" rs/ --include="*.rs" --exclude-dir=paigasus-proto-derive
```

Expected: the diff touches only the files this plan lists, and the `grep` returns **no results** —
every impl now comes from the derive (spec AC4).

`--exclude-dir=paigasus-proto-derive` is required, not a convenience: that crate necessarily
contains `impl … Auditable for …` as literal text — once in the `quote!` template `expand()`
emits, once in its test's `parse_quote!` reference, and once in a rustdoc line. Anchoring the
pattern does not help, because the two macro-template occurrences sit at the start of their
lines. Excluding the crate still covers the rest of the workspace, which is where a hand-written
impl would actually be the defect this check is looking for.

---

## Self-Review

**Spec coverage.** Every spec section maps to a task: §4.1/4.2/4.3 → Task 1; §4.4 → Tasks 2–3; §5 → Task 3; §6 → Task 4; §7 → Tasks 1/3/4; §8 → Tasks 1/2/5; §11 AC1–7 → Tasks 1–5 (AC6's diff assertions are Task 3 Step 4; AC4's "no hand-written impl" is Task 5 Step 6).

**Deliberately not in this plan**, per spec §10: server-side stamping, TS/Python equivalents, trait-shape changes, making a consumer call the accessors, and flipping `publish = true`.

**Ordering constraint.** Task 3 Steps 1 and 5 must land in **one commit** — between them the crate does not compile (`E0119`). This is the `warnings = "deny"` staging trap in a different guise, and the plan's step ordering is what avoids leaving a broken commit on the branch.
