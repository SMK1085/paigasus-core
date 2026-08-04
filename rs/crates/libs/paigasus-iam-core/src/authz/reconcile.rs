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
//!
//! **The classification ORDER is the security boundary**, not just presentation. It is, in full:
//!
//! 1. no row → `Absent`
//! 2. `revision > code_revision` → `StaleBinary { provenance_ok }` (defer, but say whether the
//!    row we are deferring to actually looks like a newer release's work)
//! 3. `!system` → `ExternallyModified`
//! 4. no fingerprint AND no revision → `Adopted` (a genuine pre-m0010 row)
//! 5. no fingerprint BUT a revision → `ExternallyModified` (the column was CLEARED)
//! 6. fingerprint mismatch → `ExternallyModified`
//! 7. content matches code → `Unchanged`
//! 8. otherwise → `Reconciled`
//!
//! Steps 3–5 exist because each is a one-`UPDATE` way to downgrade the tamper signal, and the
//! naive order (NULL-fingerprint first, `!system` last) makes every one of them *cheaper* than
//! the edit they hide. `content_fingerprint` and `starter_revision` are only ever written
//! TOGETHER (`converged_model` sets both, `doc_to_model` sets neither, m0010 back-fills
//! neither), which is exactly what makes step 5 decidable: a revision without a fingerprint
//! cannot be a pre-m0010 row.

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
    /// The stored row claims a revision NEWER than this binary's. Defer entirely (SMA-477 D11):
    /// there is one `policy` table for the whole fleet, so an older replica booting mid-deploy
    /// would otherwise push its own policy set onto every running newer replica via the
    /// `policy_gen` bump.
    ///
    /// `provenance_ok` reports whether the row actually looks like a newer release's work
    /// (`system = true` plus a fingerprint over its own content). A genuine newer release always
    /// stamps both, so `provenance_ok == false` is unambiguous: somebody forged a high revision,
    /// and since the revision check runs FIRST, that one `UPDATE` would otherwise exempt the row
    /// from convergence permanently and silently. Deferring is still the only safe action — this
    /// binary cannot know whether a newer release also wrote the content — but the caller warns
    /// and counts it as an external modification rather than a routine deploy artifact.
    StaleBinary { provenance_ok: bool },
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
            // A deferral whose provenance does not check out is NOT a routine deploy artifact —
            // it is a diverged row this binary refuses to repair, so it carries the label the
            // tamper alert watches rather than the one operators are told to expect mid-deploy.
            StarterPolicyOutcome::StaleBinary { provenance_ok: true } => "stale_binary",
            StarterPolicyOutcome::StaleBinary { provenance_ok: false } => "externally_modified",
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
            // `StaleBinary` writes NOTHING in either provenance state, so it can never justify a
            // fleet-wide cache invalidation — including the bad-provenance case, where a bump
            // every boot of every replica would be pure churn over a row nobody touched.
            StarterPolicyOutcome::Unchanged | StarterPolicyOutcome::StaleBinary { .. } => false,
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

/// Whether the stored row looks like something THIS service wrote: `system = true`, plus a
/// fingerprint over the row's own current content. Both are written together on every write
/// path this service has (`converged_model`), so a row failing either test was written by
/// something else — which is all the fingerprint ever claims to tell you (see the module docs
/// on why it is a provenance hint and not tamper evidence).
fn provenance_ok(stored: &StoredPolicyRow<'_>) -> bool {
    stored.system && stored.fingerprint.is_some_and(|fp| fp == content_fingerprint(stored.kind, stored.source, stored.description))
}

/// Decide what boot should do with one starter policy. See the truth table in the SMA-477
/// design §3.1; the ORDER of these checks is load-bearing (module docs) and documented
/// per-branch below.
#[must_use]
pub fn classify_starter_policy(stored: Option<StoredPolicyRow<'_>>, code: &PolicyDocument, code_revision: u32) -> StarterPolicyOutcome {
    // (1) Nothing persisted — seed it.
    let Some(stored) = stored else {
        return StarterPolicyOutcome::Absent;
    };

    // (2) The row claims a newer release wrote it. Defer unconditionally — an older binary has
    // no authority over it, and deferring is what keeps fleet-wide convergence monotonic (D11).
    // A NULL revision reads as 0, so every pre-m0010 row falls through to normal handling.
    //
    // Because this check runs FIRST and writes nothing, a forged high revision is the cheapest
    // possible exemption from convergence. We cannot repair it here without breaking D11, so we
    // instead report whether the row's provenance holds up: a genuine newer release always
    // stamps a fingerprint over its own content, so a mismatch here is tampering with no false
    // positives, and the caller escalates the log level and the metric label accordingly.
    if stored.revision.unwrap_or(0) > code_revision {
        return StarterPolicyOutcome::StaleBinary {
            provenance_ok: provenance_ok(&stored),
        };
    }

    let content_changed = stored.kind != code.kind || stored.source != code.source || stored.description != code.description;
    let previous = || PolicyContent {
        kind: stored.kind,
        source: stored.source.to_string(),
        description: stored.description.to_string(),
    };
    let externally_modified = || StarterPolicyOutcome::ExternallyModified {
        content_changed,
        previous_content: previous(),
    };

    // (3) `!system` is treated as broken provenance, not as an operator's own policy: we only
    // ever write these rows with `system = true`, so a cleared flag means something else wrote
    // it — and without this, one `UPDATE policy SET system = false` would exempt a starter
    // policy from convergence forever (D6). This precedes the fingerprint branches below
    // BECAUSE it must: clearing the flag and the fingerprint in the same statement would
    // otherwise land in (4)'s adoption path, which is INFO and (when content is untouched) not
    // even audited.
    if !stored.system {
        return externally_modified();
    }

    let Some(fingerprint) = stored.fingerprint else {
        return if stored.revision.is_some() {
            // (5) A revision WITHOUT a fingerprint cannot be a pre-m0010 row: this service only
            // ever writes the two together, `doc_to_model` writes neither, and m0010 back-fills
            // neither. So the column was deliberately CLEARED — the cheapest way to downgrade a
            // WARN-plus-audit into a routine `adopted` INFO — and it is reported as the edit it
            // is. Note the asymmetry with (4) is provable, not heuristic.
            externally_modified()
        } else {
            // (4) A genuine pre-fingerprint row: provenance is unknowable, so adopt rather than
            // cry wolf at every environment on the first boot after the upgrade (D3).
            StarterPolicyOutcome::Adopted {
                content_changed,
                previous_content: content_changed.then(previous),
            }
        };
    };

    // (6) The fingerprint does not describe the stored content: something rewrote the row
    // without recomputing it.
    if fingerprint != content_fingerprint(stored.kind, stored.source, stored.description) {
        return externally_modified();
    }

    // (7)/(8) Provenance holds, so the only question left is whether the CODE moved.
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

/// The wire string the `policy.kind` column stores. Lives here so the audit entry recording an
/// overwritten row's `kind` (SMA-477 D8) and the adapter that persists it cannot drift apart —
/// `pg_policies.rs::kind_to_str` delegates to this.
#[must_use]
pub fn policy_kind_str(kind: PolicyKind) -> &'static str {
    match kind {
        PolicyKind::Static => "static",
        PolicyKind::Template => "template",
    }
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
        // Deferral is unconditional (D11) — but the outcome must NOT claim the provenance is
        // fine, or a forged revision buys a permanent, silent, INFO-level exemption.
        assert_eq!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::StaleBinary { provenance_ok: false });
    }

    /// The forged-revision bypass, isolated. One `UPDATE policy SET source = <weakened>,
    /// starter_revision = 2147483647` leaves the row diverged forever, because the revision check
    /// runs first and writes nothing. It cannot be repaired here without breaking monotonicity —
    /// so the outcome has to at least SAY the row's provenance is broken, which is what turns the
    /// boot line into a WARN and the metric into `externally_modified`.
    #[test]
    fn a_forged_revision_over_weakened_content_is_deferred_but_flagged_as_bad_provenance() {
        let code = code_doc();
        let weakened = "permit(principal, action, resource);";
        // Fingerprinted for the ORIGINAL content, as a real forger's row would be: they rewrote
        // `source` and `starter_revision`, and left the stamp we wrote in place.
        let stale_fp: &'static str = content_fingerprint(code.kind, &code.source, &code.description).leak();
        let row = StoredPolicyRow {
            kind: code.kind,
            source: weakened,
            description: &code.description,
            system: true,
            fingerprint: Some(stale_fp),
            // The literal from the finding: the largest value the `INTEGER` column can hold, so
            // recovery would need a `STARTER_POLICY_REVISION` past 2^31.
            revision: Some(2_147_483_647),
        };
        assert_eq!(
            classify_starter_policy(Some(row), &code, 1),
            StarterPolicyOutcome::StaleBinary { provenance_ok: false },
            "a high revision over content the stored fingerprint does not describe is tampering, not a newer release"
        );
    }

    /// The no-false-positives half of the pair above: a genuinely newer release stamps BOTH the
    /// revision and a fingerprint over its own content, so the ordinary mixed-version deploy —
    /// the case operators are told to expect — must stay quiet.
    #[test]
    fn a_newer_release_row_with_intact_provenance_defers_quietly() {
        let code = code_doc();
        let newer_source = "permit(principal == ?principal, action, resource in ?resource) unless { false };";
        let fp: &'static str = content_fingerprint(PolicyKind::Template, newer_source, "desc").leak();
        let row = StoredPolicyRow {
            kind: PolicyKind::Template,
            source: newer_source,
            description: "desc",
            system: true,
            fingerprint: Some(fp),
            revision: Some(2),
        };
        assert_eq!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::StaleBinary { provenance_ok: true });
    }

    /// `system = false` at a newer revision is the same forgery with a second lever pulled.
    #[test]
    fn a_newer_revision_with_a_cleared_system_flag_is_bad_provenance() {
        let code = code_doc();
        let (_, mut row) = matching_row(&code);
        row.system = false;
        row.revision = Some(99);
        assert_eq!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::StaleBinary { provenance_ok: false });
    }

    #[test]
    fn a_null_revision_reads_as_zero_and_does_not_defer() {
        let code = code_doc();
        let (_, mut row) = matching_row(&code);
        row.revision = None;
        assert_eq!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::Unchanged);
    }

    /// The deferral boundary is strictly `>`, not `>=`: a row this very release stamped must
    /// still converge when the code moves under it, or the first release after any revision bump
    /// would never converge anything again.
    #[test]
    fn an_equal_revision_still_converges() {
        let code = code_doc();
        let old_source = "permit(principal, action, resource);";
        let fp: &'static str = content_fingerprint(PolicyKind::Template, old_source, "desc").leak();
        let row = StoredPolicyRow {
            kind: PolicyKind::Template,
            source: old_source,
            description: "desc",
            system: true,
            fingerprint: Some(fp),
            revision: Some(1),
        };
        assert_eq!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::Reconciled);
    }

    #[test]
    fn a_null_fingerprint_with_matching_content_is_a_pure_stamp() {
        let code = code_doc();
        let (_, mut row) = matching_row(&code);
        row.fingerprint = None;
        // A genuine pre-m0010 row has NEITHER column: m0010 adds them nullable and back-fills
        // nothing. Leaving the revision stamped would make this the CLEARED-fingerprint case.
        row.revision = None;
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
        // "Regardless of fingerprint" means all three states have to be exercised, not just the
        // `Some(valid)` one: with the `!system` check placed AFTER the NULL-fingerprint branch
        // (the order this file shipped with), the `None` case fell through to `Adopted` — INFO,
        // and with untouched content not even audited — which is a cheaper bypass than the one
        // D6 closed.
        let code = code_doc();
        let (_, base) = matching_row(&code);

        let valid_fp = StoredPolicyRow { system: false, ..base };
        assert!(
            matches!(
                classify_starter_policy(Some(valid_fp), &code, 1),
                StarterPolicyOutcome::ExternallyModified { content_changed: false, .. }
            ),
            "system = false with a valid fingerprint"
        );

        let stale_fp: &'static str = "0".repeat(64).leak();
        let stale = StoredPolicyRow {
            system: false,
            fingerprint: Some(stale_fp),
            ..base
        };
        assert!(
            matches!(classify_starter_policy(Some(stale), &code, 1), StarterPolicyOutcome::ExternallyModified { content_changed: false, .. }),
            "system = false with a stale fingerprint"
        );

        // The case the old ordering got wrong. `revision` is left NULL too, so this is the
        // WEAKEST possible fixture: nothing but the cleared `system` flag can produce the
        // expected outcome.
        let no_fp = StoredPolicyRow {
            system: false,
            fingerprint: None,
            revision: None,
            ..base
        };
        assert!(
            matches!(classify_starter_policy(Some(no_fp), &code, 1), StarterPolicyOutcome::ExternallyModified { content_changed: false, .. }),
            "system = false with NO fingerprint must not classify as a routine adoption"
        );
    }

    /// `UPDATE policy SET source = <weakened>, content_fingerprint = NULL` — one statement that
    /// turned a WARN-plus-audit into an `adopted` INFO, which the runbook describes as routine.
    /// It is decidable because this service writes the fingerprint and the revision TOGETHER, so
    /// a revision without a fingerprint is provably a cleared column and not a pre-m0010 row.
    #[test]
    fn a_cleared_fingerprint_on_a_stamped_row_is_an_external_modification() {
        let code = code_doc();
        let (_, mut row) = matching_row(&code);
        row.fingerprint = None;
        row.source = "permit(principal, action, resource);";
        assert!(
            row.revision.is_some(),
            "the fixture must keep the revision this service stamped, or it proves nothing about the discriminator"
        );

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

    /// The same lever pulled without touching the content: still an edit, and still audited —
    /// where `Adopted { content_changed: false }` would have been DEBUG with no audit row at all.
    #[test]
    fn a_cleared_fingerprint_alone_is_still_reported() {
        let code = code_doc();
        let (_, mut row) = matching_row(&code);
        row.fingerprint = None;
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
        assert_eq!(StarterPolicyOutcome::StaleBinary { provenance_ok: true }.metric_label(), "stale_binary");
        // A deferral this binary will not repair, over a row whose provenance is broken, is what
        // the tamper alert exists for — `stale_binary` is documented as an expected deploy
        // artifact, so labelling it that way would route the one case that matters to a row
        // operators are told to ignore.
        assert_eq!(
            StarterPolicyOutcome::StaleBinary { provenance_ok: false }.metric_label(),
            "externally_modified",
            "bad provenance must reach the alert, not the ignore-me bucket"
        );
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

    /// AC6/D10: `content_changed()` is the ONLY input to the `policy_gen` bump, so a wrong answer
    /// either invalidates every replica's compiled policy set on every boot (a `true` where the
    /// outcome wrote nothing) or lets a real convergence go unobserved until the snapshot's TTL
    /// backstop (a `false` where it did). Nothing else pins the non-writing variants: they are
    /// the ones a mutation can flip while the whole suite stays green.
    #[test]
    fn content_changed_is_true_exactly_for_the_outcomes_that_wrote_content() {
        let previous = PolicyContent {
            kind: PolicyKind::Static,
            source: "old".to_string(),
            description: String::new(),
        };
        let cases = [
            (StarterPolicyOutcome::Absent, true, "a seed writes the whole row"),
            (StarterPolicyOutcome::Reconciled, true, "a code change rewrites the content"),
            (StarterPolicyOutcome::Unchanged, false, "nothing was written at all"),
            (StarterPolicyOutcome::StaleBinary { provenance_ok: true }, false, "a deferral writes nothing"),
            (
                StarterPolicyOutcome::StaleBinary { provenance_ok: false },
                false,
                "a deferral writes nothing even when the row is diverged — bumping here would churn every boot",
            ),
            (
                StarterPolicyOutcome::Adopted {
                    content_changed: false,
                    previous_content: None,
                },
                false,
                "a pure provenance stamp changes nothing a decision can observe",
            ),
            (
                StarterPolicyOutcome::Adopted {
                    content_changed: true,
                    previous_content: Some(previous.clone()),
                },
                true,
                "adoption that also converged content",
            ),
            (
                StarterPolicyOutcome::ExternallyModified {
                    content_changed: false,
                    previous_content: previous.clone(),
                },
                false,
                "provenance-only repair on content that already matched",
            ),
            (
                StarterPolicyOutcome::ExternallyModified {
                    content_changed: true,
                    previous_content: previous,
                },
                true,
                "an edit that was converged away",
            ),
        ];
        for (outcome, want, why) in cases {
            assert_eq!(outcome.content_changed(), want, "{outcome:?}: {why}");
        }
    }

    #[test]
    fn policy_kind_str_covers_every_variant() {
        // The persisted encoding: `pg_policies.rs::kind_to_str` delegates here, and the audit
        // entry's `previous_content.kind` uses it, so both sides read the same strings the
        // `ck_policy_kind` CHECK constraint allows.
        assert_eq!(policy_kind_str(PolicyKind::Static), "static");
        assert_eq!(policy_kind_str(PolicyKind::Template), "template");
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
        // `PgSystemRoleReconciler::reconcile_role` stores an empty description as NULL; the
        // comparison must agree, or an empty-description role would "differ" on every boot.
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
