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
        assert_eq!(quote::quote!(#got).to_string(), quote::quote!(#want).to_string(), "expansion drifted from the reference impl");
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
        let got = expand_str("struct S<'a, T: Clone, U = ()> where U: Default, { audit: Option<AuditMetadata>, borrowed: &'a T, other: U }").unwrap();
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
