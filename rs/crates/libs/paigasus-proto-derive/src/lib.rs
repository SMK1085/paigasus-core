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
