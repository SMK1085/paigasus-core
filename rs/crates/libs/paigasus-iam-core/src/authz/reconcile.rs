// SPDX-License-Identifier: Apache-2.0

//! Boot-time reconciliation of the code-defined starter set against what is persisted
//! (SMA-477). Pure: this module decides *what should happen*, the `SystemPolicyReconciler` /
//! `SystemRoleReconciler` adapters do the I/O, and `application::bootstrap` maps the outcome
//! onto a log level, a metric label, and (for an out-of-band edit) an audit entry.
//!
//! **The fingerprint is a provenance hint, not tamper evidence.** It answers "did this
//! service write this row, or did something else?" — which is enough to tell a routine code
//! change (silent) from a hand edit (WARN + audit), and is the whole reason the boot warning
//! stops being a permanent false positive. It is NOT a security control: the only actor who
//! can modify a `system = true` row is one with direct SQL access (the API refuses — see
//! `pg_policies.rs`'s `SystemImmutable` guard), and that same access recomputes the
//! fingerprint trivially. An adversary at that level could equally grant themselves
//! `platform_admin` in `role_grant`.
//!
//! **Provenance is checked before content** so a row hand-edited to exactly the code-defined
//! value still reports the edit once, carrying `content_changed: false` so the message stays
//! honest.

use super::model::{NodeKind, PolicyDocument, PolicyKind, Role};

/// The content-bearing triple of a `policy` row, snapshotted before it is overwritten so the
/// audit entry can record what was destroyed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyContent {
    pub kind: PolicyKind,
    pub source: String,
    pub description: String,
}

/// A borrowed view of the persisted `policy` row's decision-relevant columns, as read by the
/// reconciler adapter. Deliberately NOT `PolicyDocument`: the fingerprint and revision are
/// port-DTO concerns and must not enter the domain model every other layer passes around.
#[derive(Debug, Clone, Copy)]
pub struct StoredPolicyRow<'a> {
    pub kind: PolicyKind,
    pub source: &'a str,
    pub description: &'a str,
    pub system: bool,
    pub fingerprint: Option<&'a str>,
    pub revision: Option<u32>,
}

/// What boot should do with one starter policy. Every variant except `Unchanged` and
/// `StaleBinary` writes; only the two that carry `previous_content` audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StarterPolicyOutcome {
    /// No row — seed it.
    Absent,
    /// Content matches and the fingerprint proves we wrote it. Nothing to do.
    Unchanged,
    /// The stored row was written by a NEWER release than this binary. Defer entirely
    /// (SMA-477 D11): there is one `policy` table for the whole fleet, so an older replica
    /// booting mid-deploy would otherwise push its own policy set onto every running newer
    /// replica via the `policy_gen` bump.
    StaleBinary,
    /// The row predates the fingerprint column, so its provenance is unknowable. Converge and
    /// stamp. Audited only when the content actually changed — that is the one boot on which a
    /// pre-existing hand edit is destroyed, and it must not vanish silently.
    Adopted { content_changed: bool, previous_content: Option<PolicyContent> },
    /// We wrote the stored row and the code has since changed. The routine case; silent.
    Reconciled,
    /// Something other than this service wrote the row (stale fingerprint, or `system` cleared
    /// to dodge convergence). Converge, warn, and audit.
    ExternallyModified { content_changed: bool, previous_content: PolicyContent },
}

impl StarterPolicyOutcome {
    /// The `outcome` label on `iam_starter_policy_reconciles_total`. A closed set — never
    /// derived from anything caller-supplied.
    #[must_use]
    pub fn metric_label(&self) -> &'static str {
        match self {
            StarterPolicyOutcome::Absent => "seeded",
            StarterPolicyOutcome::Unchanged => "unchanged",
            StarterPolicyOutcome::StaleBinary => "stale_binary",
            StarterPolicyOutcome::Adopted { .. } => "adopted",
            StarterPolicyOutcome::Reconciled => "reconciled",
            StarterPolicyOutcome::ExternallyModified { .. } => "externally_modified",
        }
    }

    /// Whether policy CONTENT changed — the only thing that justifies a `policy_gen` bump
    /// (SMA-477 D10). A pure fingerprint stamp changes nothing a decision can observe.
    #[must_use]
    pub fn content_changed(&self) -> bool {
        match self {
            StarterPolicyOutcome::Absent | StarterPolicyOutcome::Reconciled => true,
            StarterPolicyOutcome::Unchanged | StarterPolicyOutcome::StaleBinary => false,
            StarterPolicyOutcome::Adopted { content_changed, .. } | StarterPolicyOutcome::ExternallyModified { content_changed, .. } => *content_changed,
        }
    }
}

/// What `reconcile_role` did to one `role` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleOutcome {
    Inserted,
    Unchanged,
    Updated,
}

/// A borrowed view of the persisted `role` row's comparable columns.
#[derive(Debug, Clone, Copy)]
pub struct StoredRoleRow<'a> {
    pub template_id: &'a str,
    /// The JSON-array encoding the column stores, exactly as read back.
    pub scope_kinds: &'a str,
    /// `None` when the column is NULL — which is how an empty description is stored.
    pub description: Option<&'a str>,
    pub system: bool,
}

/// blake3 of a length-prefixed encoding of the content-bearing triple. Length-prefixed so no
/// field value can forge a field boundary (`("ab", "c")` must not collide with `("a", "bc")`).
/// Lowercase hex, 64 chars — pinned by the `ck_policy_fingerprint` CHECK constraint.
#[must_use]
pub fn content_fingerprint(kind: PolicyKind, source: &str, description: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    // A discriminant byte rather than the wire string: this is an internal hash, so it owes
    // nothing to the `static`/`template` encoding `pg_policies.rs` persists.
    hasher.update(&[match kind {
        PolicyKind::Static => 0u8,
        PolicyKind::Template => 1u8,
    }]);
    for field in [source, description] {
        hasher.update(&(field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

/// Decide what boot should do with one starter policy. See the truth table in the SMA-477
/// design §3.1; the ORDER of these checks is load-bearing and documented per-branch.
#[must_use]
pub fn classify_starter_policy(stored: Option<StoredPolicyRow<'_>>, code: &PolicyDocument, code_revision: u32) -> StarterPolicyOutcome {
    let Some(stored) = stored else {
        return StarterPolicyOutcome::Absent;
    };

    // (1) A newer release wrote this row. Defer unconditionally — an older binary has no
    // authority over it, and deferring is what keeps fleet-wide convergence monotonic (D11).
    // A NULL revision reads as 0, so every pre-m0010 row falls through to normal handling.
    if stored.revision.unwrap_or(0) > code_revision {
        return StarterPolicyOutcome::StaleBinary;
    }

    let content_changed = stored.kind != code.kind || stored.source != code.source || stored.description != code.description;
    let previous = || PolicyContent {
        kind: stored.kind,
        source: stored.source.to_string(),
        description: stored.description.to_string(),
    };

    // (2) Pre-fingerprint row: provenance unknowable, so adopt rather than cry wolf (D3).
    let Some(fingerprint) = stored.fingerprint else {
        return StarterPolicyOutcome::Adopted {
            content_changed,
            previous_content: content_changed.then(previous),
        };
    };

    // (3) `!system` is treated as broken provenance, not as an operator's own policy: we only
    // ever write these rows with `system = true`, so a cleared flag means something else wrote
    // it — and without this, one `UPDATE policy SET system = false` would exempt a starter
    // policy from convergence forever (D6).
    if !stored.system || fingerprint != content_fingerprint(stored.kind, stored.source, stored.description) {
        return StarterPolicyOutcome::ExternallyModified {
            content_changed,
            previous_content: previous(),
        };
    }

    if content_changed { StarterPolicyOutcome::Reconciled } else { StarterPolicyOutcome::Unchanged }
}

/// Renders a `Role::scope_kinds` list as the JSON-array-of-strings the `role.scope_kinds`
/// column stores (e.g. `["organization"]`). Lives here rather than in the adapter because it
/// is the pinned canonical encoding (design §6.1) that both the writer and `role_row_matches`
/// must agree on.
#[must_use]
pub fn scope_kinds_json(kinds: &[NodeKind]) -> String {
    let items: Vec<String> = kinds.iter().map(|k| format!("\"{}\"", node_kind_str(*k))).collect();
    format!("[{}]", items.join(","))
}

#[must_use]
pub fn node_kind_str(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Root => "root",
        NodeKind::Organization => "organization",
        NodeKind::Team => "team",
        NodeKind::Project => "project",
    }
}

/// Whether a persisted `role` row already matches its code-defined `Role`. An empty code-side
/// description matches a NULL column, mirroring how the row is written.
#[must_use]
pub fn role_row_matches(stored: &StoredRoleRow<'_>, code: &Role) -> bool {
    let code_description = if code.description.is_empty() { None } else { Some(code.description.as_str()) };
    stored.template_id == code.template_id && stored.scope_kinds == scope_kinds_json(&code.scope_kinds) && stored.description == code_description && stored.system == code.system
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::model::{NodeKind, PolicyDocument, PolicyKind, Role};
    use chrono::Utc;

    fn code_doc() -> PolicyDocument {
        let now = Utc::now();
        PolicyDocument {
            policy_id: "org_admin".to_string(),
            kind: PolicyKind::Template,
            source: "permit(principal == ?principal, action, resource in ?resource);".to_string(),
            description: "desc".to_string(),
            system: true,
            created_at: now,
            updated_at: now,
        }
    }

    /// A stored row that is byte-identical to `code_doc()` and correctly fingerprinted.
    fn matching_row(code: &PolicyDocument) -> (String, StoredPolicyRow<'static>) {
        let fp = content_fingerprint(code.kind, &code.source, &code.description);
        // leak is fine in a test: gives the row a 'static borrow without fighting lifetimes
        let src: &'static str = Box::leak(code.source.clone().into_boxed_str());
        let desc: &'static str = Box::leak(code.description.clone().into_boxed_str());
        let fp_static: &'static str = Box::leak(fp.clone().into_boxed_str());
        (
            fp,
            StoredPolicyRow {
                kind: code.kind,
                source: src,
                description: desc,
                system: true,
                fingerprint: Some(fp_static),
                revision: Some(1),
            },
        )
    }

    #[test]
    fn absent_row_is_seeded() {
        assert_eq!(classify_starter_policy(None, &code_doc(), 1), StarterPolicyOutcome::Absent);
    }

    #[test]
    fn matching_row_with_good_fingerprint_is_unchanged() {
        let code = code_doc();
        let (_, row) = matching_row(&code);
        assert_eq!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::Unchanged);
    }

    #[test]
    fn a_newer_stored_revision_defers_even_when_content_differs_and_provenance_is_broken() {
        let code = code_doc();
        let row = StoredPolicyRow {
            kind: PolicyKind::Template,
            source: "permit(principal, action, resource);",
            description: "tampered",
            system: false,
            fingerprint: Some("deadbeef"),
            revision: Some(9),
        };
        assert_eq!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::StaleBinary);
    }

    #[test]
    fn a_null_revision_reads_as_zero_and_does_not_defer() {
        let code = code_doc();
        let (_, mut row) = matching_row(&code);
        row.revision = None;
        assert_eq!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::Unchanged);
    }

    #[test]
    fn an_equal_revision_still_converges() {
        let code = code_doc();
        let row = StoredPolicyRow {
            kind: PolicyKind::Template,
            source: "permit(principal, action, resource);",
            description: "desc",
            system: true,
            fingerprint: None,
            revision: Some(1),
        };
        assert!(matches!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::Adopted { content_changed: true, .. }));
    }

    #[test]
    fn a_null_fingerprint_with_matching_content_is_a_pure_stamp() {
        let code = code_doc();
        let (_, mut row) = matching_row(&code);
        row.fingerprint = None;
        assert_eq!(
            classify_starter_policy(Some(row), &code, 1),
            StarterPolicyOutcome::Adopted {
                content_changed: false,
                previous_content: None
            }
        );
    }

    #[test]
    fn a_null_fingerprint_with_changed_content_is_adopted_and_carries_the_previous_content() {
        let code = code_doc();
        let row = StoredPolicyRow {
            kind: PolicyKind::Template,
            source: "permit(principal, action, resource);",
            description: "old",
            system: true,
            fingerprint: None,
            revision: None,
        };
        let out = classify_starter_policy(Some(row), &code, 1);
        let StarterPolicyOutcome::Adopted {
            content_changed: true,
            previous_content: Some(prev),
        } = out
        else {
            panic!("expected Adopted with previous content, got {out:?}")
        };
        assert_eq!(prev.source, "permit(principal, action, resource);");
        assert_eq!(prev.description, "old");
    }

    #[test]
    fn a_stale_fingerprint_is_an_external_modification() {
        let code = code_doc();
        let stale_fp: &'static str = "0".repeat(64).leak();
        let row = StoredPolicyRow {
            kind: PolicyKind::Template,
            source: "permit(principal, action, resource);",
            description: "desc",
            system: true,
            fingerprint: Some(stale_fp),
            revision: Some(1),
        };
        let out = classify_starter_policy(Some(row), &code, 1);
        let StarterPolicyOutcome::ExternallyModified {
            content_changed: true,
            previous_content,
        } = out
        else {
            panic!("expected ExternallyModified, got {out:?}")
        };
        assert_eq!(previous_content.source, "permit(principal, action, resource);");
    }

    #[test]
    fn a_stale_fingerprint_on_content_that_already_matches_still_reports_the_edit() {
        // D4: provenance is checked before content. Somebody hand-edited the row to exactly
        // the code value (today's runbook remediation) — still worth saying once.
        let code = code_doc();
        let (_, mut row) = matching_row(&code);
        let stale_fp: &'static str = "0".repeat(64).leak();
        row.fingerprint = Some(stale_fp);
        assert!(matches!(
            classify_starter_policy(Some(row), &code, 1),
            StarterPolicyOutcome::ExternallyModified { content_changed: false, .. }
        ));
    }

    #[test]
    fn a_non_system_row_is_an_external_modification_regardless_of_fingerprint() {
        // D6: `UPDATE policy SET system = false` must not buy an exemption from convergence.
        let code = code_doc();
        let (_, mut row) = matching_row(&code);
        row.system = false;
        assert!(matches!(
            classify_starter_policy(Some(row), &code, 1),
            StarterPolicyOutcome::ExternallyModified { content_changed: false, .. }
        ));
    }

    #[test]
    fn a_good_fingerprint_with_changed_source_is_a_code_change() {
        let code = code_doc();
        let old_source = "permit(principal, action, resource);";
        let fp = content_fingerprint(PolicyKind::Template, old_source, "desc");
        let fp_static: &'static str = fp.leak();
        let row = StoredPolicyRow {
            kind: PolicyKind::Template,
            source: old_source,
            description: "desc",
            system: true,
            fingerprint: Some(fp_static),
            revision: Some(1),
        };
        assert_eq!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::Reconciled);
    }

    #[test]
    fn kind_alone_differing_is_a_code_change() {
        let code = code_doc();
        let fp = content_fingerprint(PolicyKind::Static, &code.source, &code.description);
        let src: &'static str = Box::leak(code.source.clone().into_boxed_str());
        let fp_static: &'static str = fp.leak();
        let row = StoredPolicyRow {
            kind: PolicyKind::Static,
            source: src,
            description: "desc",
            system: true,
            fingerprint: Some(fp_static),
            revision: Some(1),
        };
        assert_eq!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::Reconciled);
    }

    #[test]
    fn description_alone_differing_is_a_code_change() {
        let code = code_doc();
        let src: &'static str = Box::leak(code.source.clone().into_boxed_str());
        let fp = content_fingerprint(PolicyKind::Template, &code.source, "stale description");
        let fp_static: &'static str = fp.leak();
        let row = StoredPolicyRow {
            kind: PolicyKind::Template,
            source: src,
            description: "stale description",
            system: true,
            fingerprint: Some(fp_static),
            revision: Some(1),
        };
        assert_eq!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::Reconciled);
    }

    #[test]
    fn the_fingerprint_is_lowercase_hex_and_field_boundaries_cannot_be_forged() {
        let a = content_fingerprint(PolicyKind::Static, "ab", "c");
        let b = content_fingerprint(PolicyKind::Static, "a", "bc");
        assert_ne!(a, b, "length prefixing must stop a value straddling the field boundary");
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(
            content_fingerprint(PolicyKind::Static, "s", "d"),
            content_fingerprint(PolicyKind::Template, "s", "d"),
            "kind must be part of the fingerprint"
        );
    }

    #[test]
    fn metric_labels_are_stable_and_distinct() {
        assert_eq!(StarterPolicyOutcome::Absent.metric_label(), "seeded");
        assert_eq!(StarterPolicyOutcome::Unchanged.metric_label(), "unchanged");
        assert_eq!(StarterPolicyOutcome::StaleBinary.metric_label(), "stale_binary");
        assert_eq!(StarterPolicyOutcome::Reconciled.metric_label(), "reconciled");
        assert_eq!(
            StarterPolicyOutcome::Adopted {
                content_changed: false,
                previous_content: None
            }
            .metric_label(),
            "adopted"
        );
        assert_eq!(
            StarterPolicyOutcome::ExternallyModified {
                content_changed: true,
                previous_content: PolicyContent {
                    kind: PolicyKind::Static,
                    source: String::new(),
                    description: String::new()
                }
            }
            .metric_label(),
            "externally_modified"
        );
    }

    #[test]
    fn scope_kinds_json_renders_a_json_string_array() {
        assert_eq!(scope_kinds_json(&[NodeKind::Root]), r#"["root"]"#);
        assert_eq!(scope_kinds_json(&[NodeKind::Organization, NodeKind::Team]), r#"["organization","team"]"#);
    }

    #[test]
    fn node_kind_str_covers_every_variant() {
        assert_eq!(node_kind_str(NodeKind::Root), "root");
        assert_eq!(node_kind_str(NodeKind::Organization), "organization");
        assert_eq!(node_kind_str(NodeKind::Team), "team");
        assert_eq!(node_kind_str(NodeKind::Project), "project");
    }

    fn code_role() -> Role {
        Role {
            key: "org_admin".to_string(),
            template_id: "org_admin".to_string(),
            scope_kinds: vec![NodeKind::Organization],
            description: "Manage an organization.".to_string(),
            system: true,
        }
    }

    #[test]
    fn an_equal_role_row_matches() {
        let code = code_role();
        let row = StoredRoleRow {
            template_id: "org_admin",
            scope_kinds: r#"["organization"]"#,
            description: Some("Manage an organization."),
            system: true,
        };
        assert!(role_row_matches(&row, &code));
    }

    #[test]
    fn each_role_field_differing_is_detected() {
        let code = code_role();
        let base = StoredRoleRow {
            template_id: "org_admin",
            scope_kinds: r#"["organization"]"#,
            description: Some("Manage an organization."),
            system: true,
        };

        let r = StoredRoleRow { template_id: "other", ..base };
        assert!(!role_row_matches(&r, &code), "template_id");
        let r = StoredRoleRow { scope_kinds: r#"["team"]"#, ..base };
        assert!(!role_row_matches(&r, &code), "scope_kinds");
        let r = StoredRoleRow { description: Some("stale"), ..base };
        assert!(!role_row_matches(&r, &code), "description");
        let r = StoredRoleRow { system: false, ..base };
        assert!(!role_row_matches(&r, &code), "system");
    }

    #[test]
    fn an_empty_code_description_matches_a_null_column() {
        // `seed_role_row` stores an empty description as NULL; the comparison must agree,
        // or an empty-description role would "differ" on every single boot.
        let code = Role {
            description: String::new(),
            ..code_role()
        };
        let row = StoredRoleRow {
            template_id: "org_admin",
            scope_kinds: r#"["organization"]"#,
            description: None,
            system: true,
        };
        assert!(role_row_matches(&row, &code));
    }
}
