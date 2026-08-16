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
            let rendered = meta.path.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::");
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
    let mut findings = Vec::new();
    audit_items(&file.items, &mut findings);
    findings
}

/// Walks `items` looking for audit-bearing/derive-carrying structs, recursing into `pub mod`
/// blocks. prost emits a message's oneof companion types inside `pub mod <snake_case_name>`
/// (see `list_memberships_request` in the generated iam sources) — a nested MESSAGE embedding
/// AuditMetadata would take that same shape, so a top-level-only walk would silently miss it.
fn audit_items(items: &[syn::Item], findings: &mut Vec<Finding>) {
    for item in items {
        match item {
            syn::Item::Struct(s) => {
                let syn::Fields::Named(named) = &s.fields else { continue };
                let has_audit_field = named.named.iter().any(|f| f.ident.as_ref().is_some_and(|i| i == "audit") && is_option_of_audit_metadata(&f.ty));
                let has_derive = carries_expected_derive(&s.attrs);
                // Only structs that are interesting in EITHER direction.
                if has_audit_field || has_derive {
                    findings.push(Finding {
                        ty: s.ident.to_string(),
                        has_audit_field,
                        has_derive,
                    });
                }
            }
            syn::Item::Mod(m) => {
                if let Some((_, nested_items)) = &m.content {
                    audit_items(nested_items, findings);
                }
            }
            _ => {}
        }
    }
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
        "committed generated code has structs where the `audit` field and the derive disagree.\n\
         This usually means contracts/buf.gen.yaml's message_attribute= lines are out of sync:\n\
         an `audit` field without the derive means a line is MISSING;\n\
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

// prost nests a message's oneof companion types inside a `pub mod <snake_case_name>` block
// (see `list_memberships_request` in the generated iam sources) — that shape is already in the
// tree, so a message embedded the same way is not hypothetical. `audit()` must look inside it.
const NESTED_FIELD_WITHOUT_DERIVE: &str = r#"
    pub mod nested {
        #[derive(Clone, ::prost::Message)]
        pub struct MissingLine { pub audit: ::core::option::Option<super::AuditMetadata> }
    }
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
fn control_rejects_an_audit_field_with_no_derive_when_nested_in_a_module() {
    // A top-level-only walk would silently miss this struct: `total` would stay unchanged and
    // the missing message_attribute= line would never be flagged. This is the exact failure the
    // guard exists to catch — proven by running this test BEFORE `audit()` recurses into
    // `syn::Item::Mod` and confirming it fails.
    let found = audit(NESTED_FIELD_WITHOUT_DERIVE);
    assert_eq!(found.len(), 1, "a struct nested inside `pub mod` must still be visited");
    assert!(!found[0].is_consistent(), "a missing message_attribute= line must be detected even when nested");
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
