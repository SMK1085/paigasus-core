# SMA-477 Starter Policy Reconciliation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make boot converge system-owned starter Cedar policies and system role rows to the code-defined content, silently for a routine code change and loudly (WARN + audit + metric) for an out-of-band edit, so the drift warning stops being a permanent false positive.

**Architecture:** A pure classifier in `paigasus-iam-core` compares the persisted row against the code-defined document plus a stored blake3 content fingerprint, and returns one of six outcomes. Two narrow boot-only ports (`SystemPolicyReconciler`, `SystemRoleReconciler`) carry out the write; `application::bootstrap` maps outcome → log level → metric → audit entry. A monotonic `STARTER_POLICY_REVISION` stops an older binary rewriting a newer release's policy set through the shared row.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), SeaORM + Postgres, Cedar, blake3, `metrics`, tokio, `cargo nextest`, Moon.

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Rust crates use **edition 2024 + rust-version 1.95**.
- `cargo clippy --workspace -- -D warnings` must pass; `cargo fmt --check` must pass.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `moon`/`nextest` resolve to the repo-pinned versions.
- All Rust commands run from `rs/`. Use `cargo nextest run --no-tests=pass`.
- Docker integration tests skip on a Docker-less machine (`support::start_migrated_postgres` returns `None`); they are still required to compile.
- Conventional commits with a workspace scope, e.g. `feat(rs): …`. Commit bodies must **not** contain a bare `#NNN`, and the subject must start lowercase and be ≤100 chars.
- blake3 is already a workspace dependency of both crates — **do not add any new dependency**.
- Never hand-edit `.github/CODEOWNERS`.

**Names fixed across tasks** (use these exactly):

| Item | Where |
|---|---|
| `authz::reconcile` module | `paigasus-iam-core/src/authz/reconcile.rs` |
| `StoredPolicyRow<'a>`, `StoredRoleRow<'a>`, `PolicyContent` | `authz::reconcile` |
| `StarterPolicyOutcome`, `RoleOutcome` | `authz::reconcile` |
| `content_fingerprint`, `classify_starter_policy`, `role_row_matches`, `scope_kinds_json`, `node_kind_str` | `authz::reconcile` |
| `STARTER_POLICY_REVISION`, `STARTER_POLICY_IDS`, `is_starter_policy_id` | `authz::roles` |
| `SystemPolicyReconciler`, `SystemRoleReconciler` | `authz::ports` |
| `PgSystemRoleReconciler` | `adapters/persistence/pg_system_roles.rs` |
| `IAM_STARTER_POLICY_RECONCILES_TOTAL` | `paigasus-observability/src/names.rs` |
| `ReconcileStarterDeps`, `reconcile_policies`, `reconcile_roles`, `reconcile_starter` | `application/bootstrap.rs` |
| `m0010_policy_reconcile_columns` | `adapters/persistence/migration/` |
| Columns `content_fingerprint TEXT`, `starter_revision INTEGER` | table `policy` |

---

### Task 1: Pure reconcile classifier in the core

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/src/authz/reconcile.rs`
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/mod.rs`
- Test: inline `#[cfg(test)] mod tests` in `reconcile.rs`

**Interfaces:**
- Consumes: `authz::model::{PolicyDocument, PolicyKind, Role, NodeKind}` (existing).
- Produces: everything in the names table above that lives in `authz::reconcile`. Task 4 calls `classify_starter_policy` + `content_fingerprint`; Task 5 calls `role_row_matches` + `scope_kinds_json`; Task 7 matches on `StarterPolicyOutcome`.

- [ ] **Step 1: Write the failing test**

Create `rs/crates/libs/paigasus-iam-core/src/authz/reconcile.rs` with only the test module for now:

```rust
// SPDX-License-Identifier: Apache-2.0

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
            StoredPolicyRow { kind: code.kind, source: src, description: desc, system: true, fingerprint: Some(fp_static), revision: Some(1) },
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
        let row = StoredPolicyRow { kind: PolicyKind::Template, source: "permit(principal, action, resource);", description: "tampered", system: false, fingerprint: Some("deadbeef"), revision: Some(9) };
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
        let row = StoredPolicyRow { kind: PolicyKind::Template, source: "permit(principal, action, resource);", description: "desc", system: true, fingerprint: None, revision: Some(1) };
        assert!(matches!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::Adopted { content_changed: true, .. }));
    }

    #[test]
    fn a_null_fingerprint_with_matching_content_is_a_pure_stamp() {
        let code = code_doc();
        let (_, mut row) = matching_row(&code);
        row.fingerprint = None;
        assert_eq!(
            classify_starter_policy(Some(row), &code, 1),
            StarterPolicyOutcome::Adopted { content_changed: false, previous_content: None }
        );
    }

    #[test]
    fn a_null_fingerprint_with_changed_content_is_adopted_and_carries_the_previous_content() {
        let code = code_doc();
        let row = StoredPolicyRow { kind: PolicyKind::Template, source: "permit(principal, action, resource);", description: "old", system: true, fingerprint: None, revision: None };
        let out = classify_starter_policy(Some(row), &code, 1);
        let StarterPolicyOutcome::Adopted { content_changed: true, previous_content: Some(prev) } = out else {
            panic!("expected Adopted with previous content, got {out:?}")
        };
        assert_eq!(prev.source, "permit(principal, action, resource);");
        assert_eq!(prev.description, "old");
    }

    #[test]
    fn a_stale_fingerprint_is_an_external_modification() {
        let code = code_doc();
        let row = StoredPolicyRow { kind: PolicyKind::Template, source: "permit(principal, action, resource);", description: "desc", system: true, fingerprint: Some("0".repeat(64).leak()), revision: Some(1) };
        let out = classify_starter_policy(Some(row), &code, 1);
        let StarterPolicyOutcome::ExternallyModified { content_changed: true, previous_content } = out else {
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
        row.fingerprint = Some("0".repeat(64).leak());
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
        let row = StoredPolicyRow { kind: PolicyKind::Template, source: old_source, description: "desc", system: true, fingerprint: Some(fp.leak()), revision: Some(1) };
        assert_eq!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::Reconciled);
    }

    #[test]
    fn kind_alone_differing_is_a_code_change() {
        let code = code_doc();
        let fp = content_fingerprint(PolicyKind::Static, &code.source, &code.description);
        let src: &'static str = Box::leak(code.source.clone().into_boxed_str());
        let row = StoredPolicyRow { kind: PolicyKind::Static, source: src, description: "desc", system: true, fingerprint: Some(fp.leak()), revision: Some(1) };
        assert_eq!(classify_starter_policy(Some(row), &code, 1), StarterPolicyOutcome::Reconciled);
    }

    #[test]
    fn description_alone_differing_is_a_code_change() {
        let code = code_doc();
        let src: &'static str = Box::leak(code.source.clone().into_boxed_str());
        let fp = content_fingerprint(PolicyKind::Template, &code.source, "stale description");
        let row = StoredPolicyRow { kind: PolicyKind::Template, source: src, description: "stale description", system: true, fingerprint: Some(fp.leak()), revision: Some(1) };
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
        assert_eq!(StarterPolicyOutcome::Adopted { content_changed: false, previous_content: None }.metric_label(), "adopted");
        assert_eq!(
            StarterPolicyOutcome::ExternallyModified { content_changed: true, previous_content: PolicyContent { kind: PolicyKind::Static, source: String::new(), description: String::new() } }.metric_label(),
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
        let row = StoredRoleRow { template_id: "org_admin", scope_kinds: r#"["organization"]"#, description: Some("Manage an organization."), system: true };
        assert!(role_row_matches(&row, &code));
    }

    #[test]
    fn each_role_field_differing_is_detected() {
        let code = code_role();
        let base = StoredRoleRow { template_id: "org_admin", scope_kinds: r#"["organization"]"#, description: Some("Manage an organization."), system: true };

        let mut r = StoredRoleRow { template_id: "other", ..base };
        assert!(!role_row_matches(&r, &code), "template_id");
        r = StoredRoleRow { scope_kinds: r#"["team"]"#, ..base };
        assert!(!role_row_matches(&r, &code), "scope_kinds");
        r = StoredRoleRow { description: Some("stale"), ..base };
        assert!(!role_row_matches(&r, &code), "description");
        r = StoredRoleRow { system: false, ..base };
        assert!(!role_row_matches(&r, &code), "system");
    }

    #[test]
    fn an_empty_code_description_matches_a_null_column() {
        // `seed_role_row` stores an empty description as NULL; the comparison must agree,
        // or an empty-description role would "differ" on every single boot.
        let code = Role { description: String::new(), ..code_role() };
        let row = StoredRoleRow { template_id: "org_admin", scope_kinds: r#"["organization"]"#, description: None, system: true };
        assert!(role_row_matches(&row, &code));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam-core reconcile --no-tests=pass
```

Expected: FAIL — the module is not declared in `authz/mod.rs`, and none of the types exist.

- [ ] **Step 3: Declare the module**

In `rs/crates/libs/paigasus-iam-core/src/authz/mod.rs`, add `pub mod reconcile;` after `pub mod ports;` (keep the list alphabetical), and extend the re-export line:

```rust
pub use reconcile::{PolicyContent, RoleOutcome, StarterPolicyOutcome, StoredPolicyRow, StoredRoleRow};
```

- [ ] **Step 4: Write the implementation**

Prepend to `reconcile.rs`, above the test module:

```rust
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
        return StarterPolicyOutcome::ExternallyModified { content_changed, previous_content: previous() };
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
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam-core reconcile --no-tests=pass && cargo clippy -p paigasus-iam-core -- -D warnings && cargo fmt --check
```

Expected: all reconcile tests PASS, clippy and fmt clean.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src/authz/reconcile.rs rs/crates/libs/paigasus-iam-core/src/authz/mod.rs
git commit -m "feat(rs): pure starter-policy reconciliation classifier (SMA-477)"
```

---

### Task 2: Starter revision + reserved id namespace in `authz::roles`

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/roles.rs`
- Test: inline `#[cfg(test)] mod tests` in `roles.rs`

**Interfaces:**
- Consumes: `authz::reconcile::content_fingerprint` (Task 1).
- Produces: `pub const STARTER_POLICY_REVISION: u32`, `pub const STARTER_POLICY_IDS: &[&str]`, `pub fn is_starter_policy_id(id: &str) -> bool`. Task 4 passes the revision to `reconcile_system`; Task 9 calls `is_starter_policy_id`.

- [ ] **Step 1: Write the failing tests**

Add to the existing `mod tests` in `roles.rs`:

```rust
    #[test]
    fn starter_policy_ids_matches_what_starter_policies_actually_produces() {
        // The const exists so `put_in`'s reserved-namespace check is a slice scan rather than
        // nine `PolicyDocument` allocations per call. This test is what stops it drifting.
        let actual: Vec<String> = starter_policies().into_iter().map(|d| d.policy_id).collect();
        let declared: Vec<String> = STARTER_POLICY_IDS.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(declared, actual, "STARTER_POLICY_IDS must list exactly the ids starter_policies() produces, in order");
    }

    #[test]
    fn is_starter_policy_id_recognizes_every_starter_id_and_nothing_else() {
        for id in STARTER_POLICY_IDS {
            assert!(is_starter_policy_id(id), "{id} must be recognized");
        }
        assert!(!is_starter_policy_id("some-operator-policy"));
        assert!(!is_starter_policy_id(""));
    }

    /// SMA-477 D11: `STARTER_POLICY_REVISION` is hand-maintained, so this pin is what stops it
    /// being forgotten. It hashes the canonical content of every starter policy; any change to
    /// a generated source, a role's action list, a description, or a kind reds it.
    #[test]
    fn starter_policy_content_is_pinned_to_the_declared_revision() {
        let mut hasher = blake3::Hasher::new();
        for doc in starter_policies() {
            hasher.update(doc.policy_id.as_bytes());
            hasher.update(crate::authz::reconcile::content_fingerprint(doc.kind, &doc.source, &doc.description).as_bytes());
        }
        let actual = hasher.finalize().to_hex().to_string();

        assert_eq!(
            actual, EXPECTED_STARTER_CONTENT_HASH,
            "\n\nThe starter policy set's content changed.\n\
             This is expected when you add an Action, edit a role's action list, or reword a \
             description — but it means every deployed database now holds an older set.\n\n\
             Do BOTH of these, in this order:\n\
             1. Bump `STARTER_POLICY_REVISION` (currently {STARTER_POLICY_REVISION}) by one.\n\
             2. Replace `EXPECTED_STARTER_CONTENT_HASH` with:\n     {actual}\n\n\
             Skipping step 1 lets an older binary overwrite this release's policy set \
             fleet-wide (SMA-477 D11).\n"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam-core starter_policy --no-tests=pass
```

Expected: FAIL — `STARTER_POLICY_IDS`, `is_starter_policy_id`, `STARTER_POLICY_REVISION`, and `EXPECTED_STARTER_CONTENT_HASH` do not exist.

- [ ] **Step 3: Write the implementation**

In `roles.rs`, immediately after the `FORBID_ARCHIVED_WRITES_ID` const:

```rust
/// Bumped by hand whenever any starter policy's content changes — guarded by
/// `starter_policy_content_is_pinned_to_the_declared_revision` below, which reds until it is.
///
/// Persisted per row as `policy.starter_revision`, and compared on every boot: a replica whose
/// `STARTER_POLICY_REVISION` is LOWER than a stored row's leaves that row alone (SMA-477 D11).
/// There is one `policy` table for the whole fleet, so without this an older replica booting
/// mid-deploy — a rollback, a crashloop restart, an HPA scale-up, a held canary — would
/// rewrite the shared row to its own older policy set and, via the `policy_gen` bump, push it
/// onto every already-serving newer replica.
///
/// `CARGO_PKG_VERSION` cannot serve here: the crate is version `0.0.0`.
pub const STARTER_POLICY_REVISION: u32 = 1;

/// Every `policy_id` [`starter_policies`] produces, in the order it produces them. A `const`
/// so the reserved-namespace check in `PolicyStore::put_in` is a slice scan rather than nine
/// `PolicyDocument` allocations per call; kept honest by
/// `starter_policy_ids_matches_what_starter_policies_actually_produces`.
pub const STARTER_POLICY_IDS: &[&str] = &[
    FORBID_ARCHIVED_WRITES_ID,
    PLATFORM_ADMIN_KEY,
    "org_admin",
    "org_member",
    "team_admin",
    "team_member",
    "project_admin",
    "project_member",
    "gateway_user",
];

/// Whether `id` is one of the code-owned starter policy ids. The public `PutPolicy` API
/// rejects these outright (SMA-477 D6): the ids are reserved even before they are seeded, so
/// an operator cannot occupy one and thereby exempt it from boot-time convergence.
#[must_use]
pub fn is_starter_policy_id(id: &str) -> bool {
    STARTER_POLICY_IDS.contains(&id)
}

/// The pinned content hash guarding [`STARTER_POLICY_REVISION`] — see the test that reads it.
#[cfg(test)]
const EXPECTED_STARTER_CONTENT_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
```

- [ ] **Step 4: Run the tests and record the real hash**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam-core starter_policy --no-tests=pass
```

Expected: the two id tests PASS; `starter_policy_content_is_pinned_to_the_declared_revision` FAILS and prints the actual hash. Copy that hash into `EXPECTED_STARTER_CONTENT_HASH`, replacing the zeros. (Do **not** bump `STARTER_POLICY_REVISION` — this is the initial pin, not a content change.)

- [ ] **Step 5: Re-run to verify green**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam-core --no-tests=pass && cargo clippy -p paigasus-iam-core -- -D warnings && cargo fmt --check
```

Expected: PASS. If any *other* `roles.rs` test broke, that is a real regression — fix it, do not adjust the pin.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src/authz/roles.rs
git commit -m "feat(rs): pin the starter policy set to a monotonic revision (SMA-477)"
```

---

### Task 3: Migration m0010 + policy entity columns

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0010_policy_reconcile_columns.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/mod.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/entities/policy.rs`

**Interfaces:**
- Produces: columns `policy.content_fingerprint TEXT NULL` and `policy.starter_revision INTEGER NULL`, plus entity fields `content_fingerprint: Option<String>` and `starter_revision: Option<i32>`. Task 4 reads and writes both.

- [ ] **Step 1: Write the migration**

Create `m0010_policy_reconcile_columns.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! m0010 — `policy` gains the boot-reconciliation columns (SMA-477).
//!
//! - `content_fingerprint` is a blake3 hex of the `(kind, source, description)` triple this
//!   service last wrote for the row. It exists to tell a routine code change (silent) from an
//!   out-of-band edit (WARN + audit) — see `authz::reconcile`'s module docs for why it is a
//!   provenance hint and not a security control.
//! - `starter_revision` is the `authz::roles::STARTER_POLICY_REVISION` of the binary that last
//!   wrote the row. Reconcile refuses to write when the stored revision is HIGHER than its own,
//!   which is what stops an older replica pushing its policy set onto the fleet through this
//!   shared table.
//!
//! **No backfill.** blake3 is not computable in Postgres (`pgcrypto` does not offer it), so
//! both columns start NULL and the first `reconcile_starter` after this migration stamps every
//! system row. A NULL fingerprint reads as "provenance unknown" (adopt, do not warn) and a NULL
//! revision reads as `0`.
//!
//! **Every statement is idempotent, deliberately** — m0007/m0008/m0009 record that SeaORM's
//! migrator does not serialize concurrent `up()` across replicas, so a bare `ADD COLUMN` would
//! fail the loser of a simultaneous first boot. `SET LOCAL lock_timeout` mirrors m0008/m0009 so
//! the `ACCESS EXCLUSIVE` request backs off rather than queueing ahead of in-flight
//! `PolicyService::put` writes during a rolling deploy.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(
            r#"ALTER TABLE "policy"
                 ADD COLUMN IF NOT EXISTS content_fingerprint TEXT NULL,
                 ADD COLUMN IF NOT EXISTS starter_revision INTEGER NULL;"#,
        )
        .await?;
        // Pins the encoding `authz::reconcile::content_fingerprint` promises: lowercase hex,
        // 64 chars. Dropped first so a re-run replaces rather than errors.
        conn.execute_unprepared(r#"ALTER TABLE "policy" DROP CONSTRAINT IF EXISTS ck_policy_fingerprint;"#).await?;
        conn.execute_unprepared(
            r#"ALTER TABLE "policy" ADD CONSTRAINT ck_policy_fingerprint
                 CHECK (content_fingerprint IS NULL OR content_fingerprint ~ '^[0-9a-f]{64}$');"#,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(r#"ALTER TABLE "policy" DROP CONSTRAINT IF EXISTS ck_policy_fingerprint;"#).await?;
        conn.execute_unprepared(r#"ALTER TABLE "policy" DROP COLUMN IF EXISTS starter_revision, DROP COLUMN IF EXISTS content_fingerprint;"#)
            .await?;
        Ok(())
    }
}
```

- [ ] **Step 2: Register it**

In `migration/mod.rs`, add `mod m0010_policy_reconcile_columns;` after the m0009 line, and `Box::new(m0010_policy_reconcile_columns::Migration),` as the last entry of the `migrations()` vec.

- [ ] **Step 3: Add the entity columns**

In `entities/policy.rs`, add to `Model`, after `pub updated_at: DateTimeUtc,`:

```rust
    /// SMA-477: blake3 of the `(kind, source, description)` this service last wrote. NULL for
    /// operator-authored policies (only `SystemPolicyReconciler` ever sets it) and for system
    /// rows seeded before m0010.
    pub content_fingerprint: Option<String>,
    /// SMA-477: `authz::roles::STARTER_POLICY_REVISION` of the binary that last wrote the row.
    /// NULL reads as `0`.
    pub starter_revision: Option<i32>,
```

- [ ] **Step 4: Verify the workspace still builds and the existing suites pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --workspace && cargo nextest run -p paigasus-iam --no-tests=pass && cargo clippy --workspace -- -D warnings && cargo fmt --check
```

Expected: PASS. `doc_to_model` (`pg_policies.rs:139`) is **not** changed — it is shared with `put_in`, and an operator policy written through `PutPolicy` must leave both new columns NULL. An unset `ActiveModel` field is simply not written, which is exactly the required behaviour.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/persistence/migration/ rs/crates/services/paigasus-iam/src/adapters/persistence/entities/policy.rs
git commit -m "feat(rs): add policy reconciliation columns (SMA-477)"
```

---

### Task 4: `SystemPolicyReconciler` port + Postgres implementation

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/ports.rs`
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/mod.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_policies.rs`
- Test: `rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs` (new cases; Docker)

**Interfaces:**
- Consumes: `authz::reconcile::{classify_starter_policy, content_fingerprint, StarterPolicyOutcome, StoredPolicyRow}` (Task 1); `policy` entity columns (Task 3).
- Produces: `pub trait SystemPolicyReconciler` with `reconcile_system(&self, doc: &PolicyDocument, revision: u32) -> Result<StarterPolicyOutcome, AuthzError>` and `orphaned_system_policy_ids(&self, known: &[&str]) -> Result<Vec<String>, AuthzError>`, implemented by `PgPolicyStore`. Task 7 calls both.

- [ ] **Step 1: Write the failing Docker tests**

Append to `rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs`:

```rust
/// Rewrites a stored policy row's content via raw SQL, optionally leaving the fingerprint
/// stale — the difference between "a release changed the code" and "somebody edited the row".
async fn tamper_policy(db: &DatabaseConnection, policy_id: &str, source: &str, fingerprint: Option<&str>) {
    let fp = fingerprint.map_or("NULL".to_string(), |f| format!("'{f}'"));
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        format!(r#"UPDATE "policy" SET source = '{source}', content_fingerprint = {fp} WHERE policy_id = '{policy_id}'"#),
    ))
    .await
    .unwrap();
}

async fn stored_source(db: &DatabaseConnection, policy_id: &str) -> String {
    use paigasus_iam::adapters::persistence::entities::policy;
    policy::Entity::find_by_id(policy_id.to_string()).one(db).await.unwrap().unwrap().source
}

#[tokio::test]
async fn reconcile_system_seeds_stamping_the_fingerprint_and_revision() {
    use paigasus_iam::adapters::persistence::entities::policy;
    use paigasus_iam_core::authz::reconcile::{StarterPolicyOutcome, content_fingerprint};
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;
    use paigasus_iam_core::SystemPolicyReconciler;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = starter_policies().into_iter().next().unwrap();

    assert_eq!(store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap(), StarterPolicyOutcome::Absent);

    let row = policy::Entity::find_by_id(doc.policy_id.clone()).one(&db).await.unwrap().unwrap();
    assert_eq!(row.content_fingerprint.as_deref(), Some(content_fingerprint(doc.kind, &doc.source, &doc.description).as_str()));
    assert_eq!(row.starter_revision, Some(i32::try_from(STARTER_POLICY_REVISION).unwrap()));
    assert!(row.system);

    // Immediately idempotent.
    assert_eq!(store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap(), StarterPolicyOutcome::Unchanged);
}

#[tokio::test]
async fn reconcile_system_converges_a_code_change_without_reporting_an_edit() {
    use paigasus_iam_core::authz::reconcile::{StarterPolicyOutcome, content_fingerprint};
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;
    use paigasus_iam_core::SystemPolicyReconciler;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = starter_policies().into_iter().next().unwrap();
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();

    // Simulate "the previous release wrote this": different source, CORRECTLY fingerprinted.
    let old = "forbid(principal, action, resource) when { resource has effective_status };";
    let old_fp = content_fingerprint(doc.kind, old, &doc.description);
    tamper_policy(&db, &doc.policy_id, old, Some(&old_fp)).await;

    assert_eq!(store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap(), StarterPolicyOutcome::Reconciled);
    assert_eq!(stored_source(&db, &doc.policy_id).await, doc.source);
}

#[tokio::test]
async fn reconcile_system_reports_and_reverts_an_out_of_band_edit() {
    use paigasus_iam_core::authz::reconcile::StarterPolicyOutcome;
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;
    use paigasus_iam_core::SystemPolicyReconciler;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = starter_policies().into_iter().next().unwrap();
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();

    // Content rewritten, fingerprint left stale — nobody but us writes that column.
    let edited = "forbid(principal, action, resource) when { resource has effective_status };";
    tamper_policy(&db, &doc.policy_id, edited, Some(&"0".repeat(64))).await;

    let out = store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();
    let StarterPolicyOutcome::ExternallyModified { content_changed: true, previous_content } = out else {
        panic!("expected ExternallyModified, got {out:?}")
    };
    assert_eq!(previous_content.source, edited, "the overwritten source must be handed back for the audit row");
    assert_eq!(stored_source(&db, &doc.policy_id).await, doc.source);
}

#[tokio::test]
async fn reconcile_system_restores_a_cleared_system_flag() {
    use paigasus_iam::adapters::persistence::entities::policy;
    use paigasus_iam_core::authz::reconcile::StarterPolicyOutcome;
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;
    use paigasus_iam_core::SystemPolicyReconciler;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = starter_policies().into_iter().next().unwrap();
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();

    // The bypass this guards: clearing `system` must not buy an exemption from convergence.
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        format!(r#"UPDATE "policy" SET system = false, source = 'permit(principal, action, resource);' WHERE policy_id = '{}'"#, doc.policy_id),
    ))
    .await
    .unwrap();

    assert!(matches!(
        store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap(),
        StarterPolicyOutcome::ExternallyModified { .. }
    ));
    let row = policy::Entity::find_by_id(doc.policy_id.clone()).one(&db).await.unwrap().unwrap();
    assert!(row.system, "system must be restored, not left cleared");
    assert_eq!(row.source, doc.source);
}

#[tokio::test]
async fn reconcile_system_defers_to_a_newer_revision() {
    use paigasus_iam_core::authz::reconcile::StarterPolicyOutcome;
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;
    use paigasus_iam_core::SystemPolicyReconciler;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = starter_policies().into_iter().next().unwrap();
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();

    let newer = "permit(principal, action, resource);";
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        format!(r#"UPDATE "policy" SET source = '{newer}', starter_revision = {} WHERE policy_id = '{}'"#, STARTER_POLICY_REVISION + 5, doc.policy_id),
    ))
    .await
    .unwrap();

    assert_eq!(store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap(), StarterPolicyOutcome::StaleBinary);
    assert_eq!(stored_source(&db, &doc.policy_id).await, newer, "an older binary must not rewrite a newer release's row");
}

#[tokio::test]
async fn reconcile_system_adopts_a_pre_m0010_row() {
    use paigasus_iam_core::authz::reconcile::StarterPolicyOutcome;
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;
    use paigasus_iam_core::SystemPolicyReconciler;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = starter_policies().into_iter().next().unwrap();
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();

    let old = "forbid(principal, action, resource) when { resource has effective_status };";
    tamper_policy(&db, &doc.policy_id, old, None).await;
    db.execute(Statement::from_string(DbBackend::Postgres, format!(r#"UPDATE "policy" SET starter_revision = NULL WHERE policy_id = '{}'"#, doc.policy_id)))
        .await
        .unwrap();

    assert!(matches!(
        store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap(),
        StarterPolicyOutcome::Adopted { content_changed: true, previous_content: Some(_) }
    ));
    assert_eq!(stored_source(&db, &doc.policy_id).await, doc.source);
}

#[tokio::test]
async fn a_fingerprint_only_stamp_does_not_bump_policy_gen_but_a_content_change_does() {
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;
    use paigasus_iam_core::{PolicyStore, SystemPolicyReconciler};

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = starter_policies().into_iter().next().unwrap();
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();

    // Clear the fingerprint but leave content correct: a pure stamp, invisible to any decision.
    tamper_policy(&db, &doc.policy_id, &doc.source, None).await;
    let before = store.policy_gen().await.unwrap();
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();
    assert_eq!(store.policy_gen().await.unwrap(), before, "a stamp changes nothing a decision can observe");

    // Now a real content change.
    tamper_policy(&db, &doc.policy_id, "permit(principal, action, resource);", None).await;
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();
    assert!(store.policy_gen().await.unwrap() > before, "a content change must invalidate");
}

#[tokio::test]
async fn concurrent_reconcile_of_the_same_absent_policy_yields_exactly_one_row() {
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;
    use paigasus_iam_core::{PolicyStore, SystemPolicyReconciler};

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let a = Arc::new(PgPolicyStore::new(db.clone(), Generations::memory()));
    let b = a.clone();
    let doc = Arc::new(starter_policies().into_iter().next().unwrap());
    let (d1, d2) = (doc.clone(), doc.clone());

    let (r1, r2) = tokio::join!(
        tokio::spawn(async move { a.reconcile_system(&d1, STARTER_POLICY_REVISION).await }),
        tokio::spawn(async move { b.reconcile_system(&d2, STARTER_POLICY_REVISION).await }),
    );
    r1.unwrap().expect("racer 1 must not error");
    r2.unwrap().expect("racer 2 must not error");

    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let rows = store.list_all().await.unwrap();
    assert_eq!(rows.iter().filter(|d| d.policy_id == doc.policy_id).count(), 1);
    assert_eq!(rows.iter().find(|d| d.policy_id == doc.policy_id).unwrap().source, doc.source);
}

#[tokio::test]
async fn orphaned_system_policy_ids_reports_retired_starter_policies_only() {
    use paigasus_iam_core::authz::roles::{STARTER_POLICY_IDS, STARTER_POLICY_REVISION};
    use paigasus_iam_core::SystemPolicyReconciler;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    for doc in starter_policies() {
        store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();
    }
    // A system row for a role this build no longer defines.
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        r#"INSERT INTO "policy" (policy_id, kind, source, description, system, created_at, updated_at)
           VALUES ('retired_role', 'template', 'permit(principal == ?principal, action, resource in ?resource);', NULL, true, now(), now())"#
            .to_string(),
    ))
    .await
    .unwrap();
    // An operator's own (non-system) policy must NOT be reported.
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        r#"INSERT INTO "policy" (policy_id, kind, source, description, system, created_at, updated_at)
           VALUES ('operator-policy', 'static', 'permit(principal, action, resource);', NULL, false, now(), now())"#
            .to_string(),
    ))
    .await
    .unwrap();

    let orphans = store.orphaned_system_policy_ids(STARTER_POLICY_IDS).await.unwrap();
    assert_eq!(orphans, vec!["retired_role".to_string()]);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test authz_bootstrap --no-tests=pass
```

Expected: FAIL to compile — `SystemPolicyReconciler` does not exist.

- [ ] **Step 3: Declare the port**

In `authz/ports.rs`, add the import `use super::reconcile::{RoleOutcome, StarterPolicyOutcome};` alongside the existing `use super::model::{...}`, and append:

```rust
/// Boot-only reconciliation of the code-owned starter policy set (SMA-477 D5). Deliberately
/// NOT a `PolicyStore` method: `PolicyStore` has seven implementations, six of which are test
/// fakes on the request path that would gain a method nothing calls.
#[async_trait]
pub trait SystemPolicyReconciler: Send + Sync {
    /// Converge the persisted row for `doc.policy_id` to `doc`, stamping `revision`, and report
    /// what happened. Writes nothing when the outcome is `Unchanged` or `StaleBinary`. Bumps
    /// `policy_gen` best-effort, and only when policy CONTENT changed.
    async fn reconcile_system(&self, doc: &PolicyDocument, revision: u32) -> Result<StarterPolicyOutcome, AuthzError>;
    /// Ids of persisted `system = true` rows NOT in `known` — retired starter policies that
    /// nothing can now delete (`DeletePolicy` refuses a system row). Reported, never removed:
    /// a safe retirement path has its own ordering constraints and is out of scope.
    async fn orphaned_system_policy_ids(&self, known: &[&str]) -> Result<Vec<String>, AuthzError>;
    /// Every persisted `policy_id`, captured once before reconciliation so boot can tell a
    /// SURVIVABLE convergence failure (the row exists and still governs) from a FATAL seeding
    /// failure (the row is missing, so the compiled snapshot would be incomplete) — SMA-477 D12.
    async fn existing_policy_ids(&self) -> Result<Vec<String>, AuthzError>;
}
```

Add the same names to the `assert_object_safe` compile-time proof at the bottom of the file:

```rust
    #[allow(dead_code)]
    fn assert_reconciler_object_safe(_: &dyn SystemPolicyReconciler) {}
```

In `authz/mod.rs`, extend the ports re-export to include `SystemPolicyReconciler`. In the crate root `lib.rs`, mirror however `PolicyStore` is re-exported so `paigasus_iam_core::SystemPolicyReconciler` resolves (check the existing `pub use authz::{...}` line and add it there).

- [ ] **Step 4: Implement it on `PgPolicyStore`**

In `pg_policies.rs`, add the imports:

```rust
use paigasus_iam_core::authz::reconcile::{StarterPolicyOutcome, StoredPolicyRow, content_fingerprint};
use paigasus_iam_core::SystemPolicyReconciler;
use sea_orm::{ColumnTrait, QueryFilter};
```

Add a helper next to `policy_content_matches`:

```rust
/// Borrows a stored row as the classifier's input view.
///
/// Two deliberate coercions, both of which this module elsewhere refuses to make — read the
/// reasoning before "fixing" either:
///
/// - `starter_revision` is `i32` in Postgres (there is no unsigned integer type). A negative
///   value can only come from a hand edit; clamping it to `0` makes it read as "oldest
///   possible", which CONVERGES the row rather than deferring to it. Deferring on a
///   hand-written negative would be the exploitable direction.
/// - An unparseable `kind` degrades to `Static` here, where `model_to_doc` (line ~123) rightly
///   surfaces it as `Backend`. The difference is what the value feeds: `model_to_doc` feeds the
///   decision path, where a wrong kind silently changes authorization, so it must fail loudly.
///   This value feeds only the CLASSIFIER, and a corrupt `kind` can only come from a hand edit
///   — whose fingerprint therefore cannot match, so the row classifies `ExternallyModified` and
///   gets converged (repairing the bad `kind`) no matter which variant is guessed here.
///   Returning an error instead would make a corrupt row permanently unrepairable, which is the
///   opposite of this function's purpose.
fn stored_row(model: &policy::Model) -> StoredPolicyRow<'_> {
    StoredPolicyRow {
        kind: kind_from_str(&model.kind).unwrap_or(PolicyKind::Static),
        source: &model.source,
        description: model.description.as_deref().unwrap_or(""),
        system: model.system,
        fingerprint: model.content_fingerprint.as_deref(),
        revision: model.starter_revision.map(|r| u32::try_from(r).unwrap_or(0)),
    }
}

/// The full column set a converge writes. `created_at` is deliberately preserved from the
/// stored row — the incoming `doc.created_at` is `starter_policies()`'s own `Utc::now()` and
/// must never rewrite history. `system` is always set back to `true`: clearing it is one of the
/// things a converge exists to undo.
fn converged_model(doc: &PolicyDocument, created_at: DateTime<Utc>, now: DateTime<Utc>, revision: u32) -> policy::ActiveModel {
    policy::ActiveModel {
        policy_id: Set(doc.policy_id.clone()),
        kind: Set(kind_to_str(doc.kind).to_string()),
        source: Set(doc.source.clone()),
        description: Set(if doc.description.is_empty() { None } else { Some(doc.description.clone()) }),
        system: Set(true),
        created_at: Set(created_at),
        updated_at: Set(now),
        content_fingerprint: Set(Some(content_fingerprint(doc.kind, &doc.source, &doc.description))),
        starter_revision: Set(Some(i32::try_from(revision).unwrap_or(i32::MAX))),
    }
}
```

Then the impl block. Note it needs `PolicyKind` in scope (already imported) and `Utc::now()` for `updated_at` — the reconciler is an adapter, so it uses the wall clock directly, exactly as `bootstrap.rs::seed_role_row` already does for `created_at`:

```rust
#[async_trait]
impl SystemPolicyReconciler for PgPolicyStore {
    async fn reconcile_system(&self, doc: &PolicyDocument, revision: u32) -> Result<StarterPolicyOutcome, AuthzError> {
        // Same tripwire `put_in` applies. A code-defined source always passes (`roles.rs`'s own
        // suite asserts it), so a failure here means a broken release, not operator input —
        // `reconcile_policies` logs and skips rather than refusing to boot.
        validate_policy(&doc.source)?;

        let txn = self.db.begin().await.map_err(map_err)?;
        // Bounds the worst case when a concurrent `PolicyService::put_in` holds the row lock:
        // this runs BEFORE the HTTP listener binds, so an unbounded wait is a startup hang.
        txn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await.map_err(map_err)?;

        let existing = policy::Entity::find_by_id(doc.policy_id.clone()).lock_exclusive().one(&txn).await.map_err(map_err)?;
        let outcome = classify_starter_policy(existing.as_ref().map(stored_row), doc, revision);
        let now = Utc::now();

        let outcome = match (&outcome, existing) {
            (StarterPolicyOutcome::Unchanged | StarterPolicyOutcome::StaleBinary, _) => outcome,
            (_, Some(row)) => {
                converged_model(doc, row.created_at, now, revision).update(&txn).await.map_err(map_err)?;
                outcome
            }
            (_, None) => {
                // Our existence check and this INSERT are not atomic: two replicas can both see
                // an absent row. Run it on a SAVEPOINT so a unique violation rolls back only the
                // insert, not the caller's transaction — the `put_in` pattern, except we re-read
                // WITH the row lock, because unlike `put_in` we may go on to UPDATE.
                let sp = txn.begin().await.map_err(map_err)?;
                match converged_model(doc, doc.created_at, now, revision).insert(&sp).await {
                    Ok(_) => {
                        sp.commit().await.map_err(map_err)?;
                        StarterPolicyOutcome::Absent
                    }
                    Err(e) if matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => {
                        sp.rollback().await.map_err(map_err)?;
                        let winner = policy::Entity::find_by_id(doc.policy_id.clone())
                            .lock_exclusive()
                            .one(&txn)
                            .await
                            .map_err(map_err)?
                            .ok_or_else(|| backend_err(format!("policy {}: unique-constraint violation on insert but no row found on re-read", doc.policy_id)))?;
                        // Re-classify against whoever won. This cannot recurse into the insert
                        // branch — the row provably exists now — so it terminates.
                        let re = classify_starter_policy(Some(stored_row(&winner)), doc, revision);
                        if !matches!(re, StarterPolicyOutcome::Unchanged | StarterPolicyOutcome::StaleBinary) {
                            converged_model(doc, winner.created_at, now, revision).update(&txn).await.map_err(map_err)?;
                        }
                        re
                    }
                    Err(e) => return Err(map_err(e)),
                }
            }
        };

        txn.commit().await.map_err(map_err)?;
        if outcome.content_changed() {
            self.bump_policy_gen_best_effort().await;
        }
        Ok(outcome)
    }

    async fn orphaned_system_policy_ids(&self, known: &[&str]) -> Result<Vec<String>, AuthzError> {
        let rows = policy::Entity::find().filter(policy::Column::System.eq(true)).all(&self.db).await.map_err(map_err)?;
        Ok(rows.into_iter().map(|r| r.policy_id).filter(|id| !known.contains(&id.as_str())).collect())
    }

    async fn existing_policy_ids(&self) -> Result<Vec<String>, AuthzError> {
        let rows = policy::Entity::find().all(&self.db).await.map_err(map_err)?;
        Ok(rows.into_iter().map(|r| r.policy_id).collect())
    }
}
```

Add `use sea_orm::ConnectionTrait;` if `execute_unprepared` is not already in scope.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test authz_bootstrap --no-tests=pass && cargo clippy --workspace -- -D warnings && cargo fmt --check
```

Expected: the new cases PASS (or skip cleanly with no Docker — in that case at minimum `cargo build --tests` must succeed).

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src/authz/ rs/crates/services/paigasus-iam/src/adapters/persistence/pg_policies.rs rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs
git commit -m "feat(rs): converge system-owned starter policies at boot (SMA-477)"
```

---

### Task 5: `SystemRoleReconciler` port + Postgres implementation

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/ports.rs`
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_system_roles.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs`
- Test: `rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs`

**Interfaces:**
- Consumes: `authz::reconcile::{role_row_matches, scope_kinds_json, RoleOutcome, StoredRoleRow}` (Task 1).
- Produces: `pub trait SystemRoleReconciler` with `reconcile_role(&self, role: &Role) -> Result<RoleOutcome, AuthzError>` and `orphaned_system_role_keys(&self, known: &[&str]) -> Result<Vec<String>, AuthzError>`; `pub struct PgSystemRoleReconciler`. Task 7 calls both.

- [ ] **Step 1: Write the failing Docker test**

Append to `tests/authz_bootstrap.rs`:

```rust
#[tokio::test]
async fn reconcile_role_inserts_then_converges_a_drifted_row() {
    use paigasus_iam::adapters::persistence::PgSystemRoleReconciler;
    use paigasus_iam_core::authz::reconcile::RoleOutcome;
    use paigasus_iam_core::SystemRoleReconciler;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    // The role row's `template_id` carries an FK to `policy.policy_id`, so the templates must
    // exist first — the same ordering `reconcile_starter` itself relies on.
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    for doc in starter_policies() {
        use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;
        use paigasus_iam_core::SystemPolicyReconciler;
        store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();
    }

    let roles = PgSystemRoleReconciler::new(db.clone());
    let code = system_roles().into_iter().find(|r| r.key == "org_admin").unwrap();

    assert_eq!(roles.reconcile_role(&code).await.unwrap(), RoleOutcome::Inserted);
    assert_eq!(roles.reconcile_role(&code).await.unwrap(), RoleOutcome::Unchanged);

    db.execute(Statement::from_string(
        DbBackend::Postgres,
        r#"UPDATE "role" SET description = 'stale wording', scope_kinds = '["team"]' WHERE key = 'org_admin'"#.to_string(),
    ))
    .await
    .unwrap();

    assert_eq!(roles.reconcile_role(&code).await.unwrap(), RoleOutcome::Updated);
    let row = role::Entity::find_by_id("org_admin".to_string()).one(&db).await.unwrap().unwrap();
    assert_eq!(row.description.as_deref(), Some(code.description.as_str()));
    assert_eq!(row.scope_kinds, r#"["organization"]"#);
}

#[tokio::test]
async fn orphaned_system_role_keys_reports_retired_roles_only() {
    use paigasus_iam::adapters::persistence::PgSystemRoleReconciler;
    use paigasus_iam_core::SystemRoleReconciler;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    for doc in starter_policies() {
        use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;
        use paigasus_iam_core::SystemPolicyReconciler;
        store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();
    }
    let roles = PgSystemRoleReconciler::new(db.clone());
    for r in system_roles() {
        roles.reconcile_role(&r).await.unwrap();
    }
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        r#"INSERT INTO "role" (key, template_id, scope_kinds, description, system, created_at)
           VALUES ('retired_role', 'org_admin', '["organization"]', NULL, true, now())"#
            .to_string(),
    ))
    .await
    .unwrap();

    let known: Vec<&str> = system_roles().iter().map(|r| r.key.as_str()).collect::<Vec<_>>().leak().to_vec();
    let orphans = roles.orphaned_system_role_keys(&known).await.unwrap();
    assert_eq!(orphans, vec!["retired_role".to_string()]);
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test authz_bootstrap reconcile_role --no-tests=pass
```

Expected: FAIL to compile — `PgSystemRoleReconciler` does not exist.

- [ ] **Step 3: Declare the port**

Append to `authz/ports.rs`:

```rust
/// Boot-only reconciliation of the code-defined system role catalog (SMA-477 D7). Symmetric to
/// [`SystemPolicyReconciler`] so `reconcile_starter` is fully fakeable without a database.
///
/// No fingerprint and no audit: these columns are introspectable-only — nothing parses them
/// back at runtime (the `role_key -> Role` lookup is always code-defined), so there is no
/// operator-edit story worth preserving and no security-relevant content to record.
#[async_trait]
pub trait SystemRoleReconciler: Send + Sync {
    async fn reconcile_role(&self, role: &Role) -> Result<RoleOutcome, AuthzError>;
    async fn orphaned_system_role_keys(&self, known: &[&str]) -> Result<Vec<String>, AuthzError>;
}
```

Import `Role` into `ports.rs` if it is not already there, re-export `SystemRoleReconciler` from `authz/mod.rs` and the crate root exactly as Task 4 did for `SystemPolicyReconciler`, and extend the object-safety assertion.

- [ ] **Step 4: Implement the adapter**

Create `pg_system_roles.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed [`SystemRoleReconciler`] (SMA-477). Boot converges the `role` table to
//! `authz::roles::system_roles()`, where it previously only inserted missing rows — so a code
//! change to a role's description or scope kinds drifted forever, and silently.
//!
//! There is no `RoleRepository` port and this is not one: roles are code-defined, and this
//! table is only their persisted/introspectable form plus the `role_grant.role_key` FK target.

use super::entities::role;
use crate::adapters::persistence::pg_policies::map_db_err;
use async_trait::async_trait;
use chrono::Utc;
use paigasus_iam_core::authz::reconcile::{RoleOutcome, StoredRoleRow, role_row_matches, scope_kinds_json};
use paigasus_iam_core::{AuthzError, Role, SystemRoleReconciler};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set, SqlErr};

#[derive(Clone)]
pub struct PgSystemRoleReconciler {
    db: DatabaseConnection,
}

impl PgSystemRoleReconciler {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgSystemRoleReconciler { db }
    }
}

#[async_trait]
impl SystemRoleReconciler for PgSystemRoleReconciler {
    async fn reconcile_role(&self, role_def: &Role) -> Result<RoleOutcome, AuthzError> {
        let description = if role_def.description.is_empty() { None } else { Some(role_def.description.clone()) };
        let existing = role::Entity::find_by_id(role_def.key.clone()).one(&self.db).await.map_err(map_db_err)?;

        let Some(existing) = existing else {
            let active = role::ActiveModel {
                key: Set(role_def.key.clone()),
                template_id: Set(role_def.template_id.clone()),
                scope_kinds: Set(scope_kinds_json(&role_def.scope_kinds)),
                description: Set(description),
                system: Set(role_def.system),
                created_at: Set(Utc::now()),
            };
            return match active.insert(&self.db).await {
                Ok(_) => Ok(RoleOutcome::Inserted),
                // A concurrent replica's boot won the race between our check and our insert.
                // The row exists either way, so this is an idempotent no-op, not an error.
                Err(e) if matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => Ok(RoleOutcome::Unchanged),
                Err(e) => Err(map_db_err(e)),
            };
        };

        let stored = StoredRoleRow {
            template_id: &existing.template_id,
            scope_kinds: &existing.scope_kinds,
            description: existing.description.as_deref(),
            system: existing.system,
        };
        if role_row_matches(&stored, role_def) {
            return Ok(RoleOutcome::Unchanged);
        }

        // `created_at` is preserved: only the code-defined columns converge.
        let active = role::ActiveModel {
            key: Set(role_def.key.clone()),
            template_id: Set(role_def.template_id.clone()),
            scope_kinds: Set(scope_kinds_json(&role_def.scope_kinds)),
            description: Set(description),
            system: Set(role_def.system),
            created_at: Set(existing.created_at),
        };
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(RoleOutcome::Updated)
    }

    async fn orphaned_system_role_keys(&self, known: &[&str]) -> Result<Vec<String>, AuthzError> {
        let rows = role::Entity::find().filter(role::Column::System.eq(true)).all(&self.db).await.map_err(map_db_err)?;
        Ok(rows.into_iter().map(|r| r.key).filter(|k| !known.contains(&k.as_str())).collect())
    }
}
```

In `pg_policies.rs`, rename the private `map_err` to a `pub(crate) fn map_db_err` (or add `pub(crate) use` alias) so this module can share it, updating its call sites in that file. In `persistence/mod.rs`, add `pub mod pg_system_roles;` and `pub use pg_system_roles::PgSystemRoleReconciler;`.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test authz_bootstrap --no-tests=pass && cargo clippy --workspace -- -D warnings && cargo fmt --check
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src/authz/ports.rs rs/crates/libs/paigasus-iam-core/src/authz/mod.rs rs/crates/libs/paigasus-iam-core/src/lib.rs rs/crates/services/paigasus-iam/src/adapters/persistence/ rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs
git commit -m "feat(rs): converge the system role catalog at boot (SMA-477)"
```

---

### Task 6: Register the reconciliation metric

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs`

**Interfaces:**
- Produces: `pub const IAM_STARTER_POLICY_RECONCILES_TOTAL: &str = "iam_starter_policy_reconciles_total"`. Task 7 emits it.

- [ ] **Step 1: Add the constant**

After `IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL` in `names.rs`:

```rust
/// SMA-477: one increment per starter policy per boot, labelled by what reconciliation did.
/// `outcome` is a closed set — `unchanged` | `seeded` | `adopted` | `reconciled` |
/// `externally_modified` | `stale_binary` | `orphaned` | `failed` — never derived from anything
/// caller-supplied, so it cannot mint cardinality.
///
/// `externally_modified` is the one worth alerting on: it means something other than this
/// service wrote a system-owned policy row, which boot has just reverted. `stale_binary` means
/// an older replica declined to overwrite a newer release's row — expected briefly during a
/// deploy, suspicious if it persists. `orphaned` counts system rows whose id is no longer
/// code-defined; nothing can delete those automatically.
pub const IAM_STARTER_POLICY_RECONCILES_TOTAL: &str = "iam_starter_policy_reconciles_total";
```

Add `IAM_STARTER_POLICY_RECONCILES_TOTAL,` to the `ALL` array, immediately after `IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL,`.

- [ ] **Step 2: Verify the drift gate still passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-observability --no-tests=pass
```

Expected: PASS. The gate asserts committed dashboards/rules reference only *known* metrics — adding a name to `ALL` without any ops artifact referencing it is fine in that direction.

- [ ] **Step 3: Commit**

```bash
git add rs/crates/libs/paigasus-observability/src/names.rs
git commit -m "feat(rs): register the starter-policy reconciliation counter (SMA-477)"
```

---

### Task 7: Rewrite `bootstrap.rs` — orchestration, logging, metric, audit

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/bootstrap.rs`
- Modify: `rs/crates/services/paigasus-iam/src/application/fakes.rs`
- Test: inline `#[cfg(test)] mod tests` in `bootstrap.rs`

**Interfaces:**
- Consumes: `SystemPolicyReconciler` (Task 4), `SystemRoleReconciler` (Task 5), `StarterPolicyOutcome`/`RoleOutcome` (Task 1), `STARTER_POLICY_REVISION`/`STARTER_POLICY_IDS` (Task 2), `IAM_STARTER_POLICY_RECONCILES_TOTAL` (Task 6).
- Produces: `pub struct ReconcileStarterDeps<I: IdGenerator, C: Clock>`, `pub async fn reconcile_policies(...)`, `pub async fn reconcile_roles(...)`, `pub async fn reconcile_starter(deps: &ReconcileStarterDeps<I, C>) -> Result<(), AuthzError>`. Task 8 wires it at the composition root.

- [ ] **Step 1: Extend the audit fakes**

In `fakes.rs`, replace `FakeAuditLog::record_out_of_band`'s `unimplemented!` body — boot's reconcile holds no transaction, so it is the method actually used:

```rust
    async fn record_out_of_band(&self, e: &AuditEntry) -> Result<(), RepositoryError> {
        // SMA-477: `bootstrap::reconcile_policies` holds no `Transaction`, so it records
        // out of band. Same buffer as `record` — assertions read one place.
        self.0.lock().unwrap().push(e.clone());
        Ok(())
    }
```

Add, next to it:

```rust
/// An `AuditLog` whose out-of-band writes always fail — proves boot survives a failed audit
/// write (SMA-477 D9). `record`/`query` are unreachable for this fake's only caller.
#[derive(Clone, Default)]
pub struct FailingAuditLog;

#[async_trait]
impl AuditLog for FailingAuditLog {
    async fn record_out_of_band(&self, _e: &AuditEntry) -> Result<(), RepositoryError> {
        Err(RepositoryError::Backend(Box::new(std::io::Error::other("audit sink down"))))
    }
    async fn record(&self, _tx: &dyn Transaction, _e: &AuditEntry) -> Result<(), RepositoryError> {
        unimplemented!("FailingAuditLog is only used for the out-of-band path")
    }
    async fn query(&self, _f: &AuditFilter) -> Result<Vec<AuditEntry>, RepositoryError> {
        unimplemented!("FailingAuditLog is only used for the out-of-band path")
    }
}
```

Match the existing file's import list; add `AuditFilter` if absent.

- [ ] **Step 2: Write the failing tests**

Replace `bootstrap.rs`'s `mod tests` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::clock::SystemClock;
    use crate::adapters::id::KernelIdGenerator;
    use crate::application::fakes::{FailingAuditLog, FakeAuditLog};
    use paigasus_iam_core::authz::reconcile::PolicyContent;
    use paigasus_iam_core::authz::roles::starter_policies;
    use paigasus_iam_core::PolicyKind;
    use std::sync::{Arc, Mutex};

    /// Returns a scripted outcome for every policy, recording what it was asked to reconcile.
    #[derive(Default)]
    struct ScriptedPolicies {
        outcome: Mutex<Option<StarterPolicyOutcome>>,
        fail: bool,
        seen: Mutex<Vec<String>>,
        orphans: Vec<String>,
    }

    impl ScriptedPolicies {
        fn with(outcome: StarterPolicyOutcome) -> Arc<Self> {
            Arc::new(ScriptedPolicies { outcome: Mutex::new(Some(outcome)), ..Default::default() })
        }
    }

    #[async_trait::async_trait]
    impl SystemPolicyReconciler for ScriptedPolicies {
        async fn reconcile_system(&self, doc: &PolicyDocument, _revision: u32) -> Result<StarterPolicyOutcome, AuthzError> {
            self.seen.lock().unwrap().push(doc.policy_id.clone());
            if self.fail {
                return Err(AuthzError::Backend(Box::new(std::io::Error::other("db down"))));
            }
            Ok(self.outcome.lock().unwrap().clone().unwrap_or(StarterPolicyOutcome::Unchanged))
        }
        async fn orphaned_system_policy_ids(&self, _known: &[&str]) -> Result<Vec<String>, AuthzError> {
            Ok(self.orphans.clone())
        }
    }

    #[derive(Default)]
    struct ScriptedRoles;

    #[async_trait::async_trait]
    impl SystemRoleReconciler for ScriptedRoles {
        async fn reconcile_role(&self, _role: &Role) -> Result<RoleOutcome, AuthzError> {
            Ok(RoleOutcome::Unchanged)
        }
        async fn orphaned_system_role_keys(&self, _known: &[&str]) -> Result<Vec<String>, AuthzError> {
            Ok(vec![])
        }
    }

    fn deps(policies: Arc<ScriptedPolicies>, audit: Arc<dyn AuditLog>) -> ReconcileStarterDeps<KernelIdGenerator, SystemClock> {
        ReconcileStarterDeps { policies, roles: Arc::new(ScriptedRoles), audit, ids: KernelIdGenerator, clock: SystemClock }
    }

    fn tampered() -> StarterPolicyOutcome {
        StarterPolicyOutcome::ExternallyModified {
            content_changed: true,
            previous_content: PolicyContent { kind: PolicyKind::Static, source: "permit(principal, action, resource);".to_string(), description: "old".to_string() },
        }
    }

    #[tokio::test]
    async fn an_external_modification_writes_one_audit_row_per_policy() {
        let entries = FakeAuditLog::default();
        let d = deps(ScriptedPolicies::with(tampered()), Arc::new(entries.clone()));
        reconcile_policies(&d).await.unwrap();

        let rows = entries.0.lock().unwrap();
        assert_eq!(rows.len(), starter_policies().len(), "one audit row per externally-modified policy");
        let e = &rows[0];
        assert_eq!(e.action, "PutPolicy", "reuse the standard action so the row appears in the standard query");
        assert_eq!(e.actor_prn, None, "no principal authorized this — a deployment did");
        assert_eq!(e.resource_prn.as_deref(), Some(root_prn().canonical().as_str()));
        assert_eq!(e.outcome, AuditOutcome::Committed);
        assert_eq!(e.detail["source"], serde_json::json!("starter_policy_reconcile"));
        assert_eq!(e.detail["reason"], serde_json::json!("external_modification"));
        assert_eq!(e.detail["previous_content"]["source"], serde_json::json!("permit(principal, action, resource);"));
    }

    #[tokio::test]
    async fn an_adopted_row_audits_only_when_its_content_actually_changed() {
        let changed = FakeAuditLog::default();
        let d = deps(
            ScriptedPolicies::with(StarterPolicyOutcome::Adopted {
                content_changed: true,
                previous_content: Some(PolicyContent { kind: PolicyKind::Static, source: "old".to_string(), description: String::new() }),
            }),
            Arc::new(changed.clone()),
        );
        reconcile_policies(&d).await.unwrap();
        assert_eq!(changed.0.lock().unwrap().len(), starter_policies().len());
        assert_eq!(changed.0.lock().unwrap()[0].detail["reason"], serde_json::json!("adopted_unfingerprinted"));

        let stamped = FakeAuditLog::default();
        let d = deps(
            ScriptedPolicies::with(StarterPolicyOutcome::Adopted { content_changed: false, previous_content: None }),
            Arc::new(stamped.clone()),
        );
        reconcile_policies(&d).await.unwrap();
        assert!(stamped.0.lock().unwrap().is_empty(), "a pure fingerprint stamp is not an event");
    }

    #[tokio::test]
    async fn routine_outcomes_write_no_audit_rows() {
        for outcome in [StarterPolicyOutcome::Unchanged, StarterPolicyOutcome::Reconciled, StarterPolicyOutcome::Absent, StarterPolicyOutcome::StaleBinary] {
            let entries = FakeAuditLog::default();
            let d = deps(ScriptedPolicies::with(outcome.clone()), Arc::new(entries.clone()));
            reconcile_policies(&d).await.unwrap();
            assert!(entries.0.lock().unwrap().is_empty(), "{outcome:?} must not audit");
        }
    }

    #[tokio::test]
    async fn an_oversized_previous_source_is_truncated_and_marked() {
        let entries = FakeAuditLog::default();
        let huge = "x".repeat(MAX_AUDITED_SOURCE_BYTES + 500);
        let d = deps(
            ScriptedPolicies::with(StarterPolicyOutcome::ExternallyModified {
                content_changed: true,
                previous_content: PolicyContent { kind: PolicyKind::Static, source: huge, description: String::new() },
            }),
            Arc::new(entries.clone()),
        );
        reconcile_policies(&d).await.unwrap();

        let rows = entries.0.lock().unwrap();
        let src = rows[0].detail["previous_content"]["source"].as_str().unwrap();
        assert_eq!(src.len(), MAX_AUDITED_SOURCE_BYTES);
        assert_eq!(rows[0].detail["previous_content"]["truncated"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn a_failing_audit_sink_does_not_fail_boot() {
        let d = deps(ScriptedPolicies::with(tampered()), Arc::new(FailingAuditLog));
        reconcile_policies(&d).await.expect("an audit-write failure must never stop a replica starting");
    }

    #[tokio::test]
    async fn a_convergence_failure_is_skipped_but_a_seeding_failure_is_fatal() {
        // Convergence failure: the stored row governed fine before this change, so keeping it
        // for one more boot beats refusing to start.
        let converge_fail = Arc::new(ScriptedPolicies {
            fail: true,
            existing: starter_policies().into_iter().map(|d| d.policy_id).collect(),
            ..Default::default()
        });
        let entries = FakeAuditLog::default();
        let d = deps(converge_fail.clone(), Arc::new(entries));
        reconcile_policies(&d).await.expect("a transient convergence failure must not stop boot");
        assert_eq!(converge_fail.seen.lock().unwrap().len(), starter_policies().len(), "every policy is still attempted");
    }

    #[tokio::test]
    async fn every_starter_policy_is_reconciled() {
        let scripted = ScriptedPolicies::with(StarterPolicyOutcome::Unchanged);
        let d = deps(scripted.clone(), Arc::new(FakeAuditLog::default()));
        reconcile_policies(&d).await.unwrap();
        let seen = scripted.seen.lock().unwrap().clone();
        let expected: Vec<String> = starter_policies().into_iter().map(|p| p.policy_id).collect();
        assert_eq!(seen, expected);
    }
}
```

- [ ] **Step 3: Run to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam bootstrap:: --no-tests=pass
```

Expected: FAIL to compile — `ReconcileStarterDeps`, `reconcile_policies`, `MAX_AUDITED_SOURCE_BYTES` do not exist.

- [ ] **Step 4: Write the implementation**

Replace everything in `bootstrap.rs` above the test module:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Boot-time reconciliation of the starter Cedar policy set + the eight system roles
//! (SMA-444 Task 17; converge-to-code since SMA-477). Runs once per `AppState::new`, AFTER the
//! policy store is constructed but BEFORE the initial `PolicySnapshot` compiles — so a fresh
//! database is seeded before the first request is decided.
//!
//! **These rows are code-owned.** Boot converges each starter policy and each system role row
//! to the code-defined content; the database was the one place that ownership was not enforced.
//! What used to be a compare-and-WARN that never wrote — and therefore warned forever, since a
//! generated policy source changes with every new write action — is now:
//!
//! - a routine code change converges silently (INFO);
//! - a row written by a NEWER release is left alone (`StaleBinary`) — see
//!   `authz::roles::STARTER_POLICY_REVISION` for why that matters with one shared table;
//! - an out-of-band edit converges LOUDLY: WARN, a metric, and one audit entry capturing what
//!   was overwritten, since converging destroys the evidence.
//!
//! **Failure posture (SMA-477 D12).** Converging an existing row is best-effort: an error is
//! logged, counted, and skipped, because that row governed decisions perfectly well before this
//! change and refusing to boot would turn a transient database blip into an outage. SEEDING an
//! absent row stays fatal — `AppState::new` documents that the initial snapshot always compiles
//! at least the starter set, and a replica that booted with a partial policy set would deny
//! everything.

use chrono::Utc;
use metrics::counter;
use paigasus_iam_core::authz::reconcile::{RoleOutcome, StarterPolicyOutcome};
use paigasus_iam_core::authz::roles::{self as authz_roles, STARTER_POLICY_IDS, STARTER_POLICY_REVISION};
use paigasus_iam_core::{AuditEntry, AuditLog, AuditOutcome, AuthzError, Clock, IdGenerator, PolicyDocument, Role, SystemPolicyReconciler, SystemRoleReconciler};
use paigasus_iam_core::authz::model::root_prn;
use paigasus_observability::names;
use std::sync::Arc;

/// Cap on the overwritten source copied into an audit entry. The value is attacker-influenced
/// text landing in an append-only table, so it is bounded rather than trusted.
pub const MAX_AUDITED_SOURCE_BYTES: usize = 8 * 1024;

/// Boot-time reconciliation dependencies. A named-field `*Deps` struct with generic
/// `IdGenerator`/`Clock`, mirroring `BootstrapAdminSeederDeps` and `RoleServiceDeps`.
pub struct ReconcileStarterDeps<I: IdGenerator, C: Clock> {
    pub policies: Arc<dyn SystemPolicyReconciler>,
    pub roles: Arc<dyn SystemRoleReconciler>,
    pub audit: Arc<dyn AuditLog>,
    pub ids: I,
    pub clock: C,
}

fn count(outcome_label: &'static str) {
    counter!(names::IAM_STARTER_POLICY_RECONCILES_TOTAL, "outcome" => outcome_label).increment(1);
}

/// Truncates on a char boundary so the result is always valid UTF-8 for `serde_json`.
fn truncate_source(source: &str) -> (String, bool) {
    if source.len() <= MAX_AUDITED_SOURCE_BYTES {
        return (source.to_string(), false);
    }
    let mut end = MAX_AUDITED_SOURCE_BYTES;
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    (source[..end].to_string(), true)
}

/// Reconciles every `authz::roles::starter_policies()` document. Only a failure to SEED an
/// absent policy propagates (see the module docs).
pub async fn reconcile_policies<I: IdGenerator, C: Clock>(deps: &ReconcileStarterDeps<I, C>) -> Result<(), AuthzError> {
    // Captured once, BEFORE any convergence: which ids already exist decides whether a failure
    // below is survivable. An unreadable list degrades to "assume nothing exists", i.e. treat
    // every failure as a fatal seed — the conservative direction, since the alternative is
    // booting a replica with an incomplete policy set that denies everything.
    let existing_ids = deps.policies.existing_policy_ids().await.unwrap_or_default();

    for doc in authz_roles::starter_policies() {
        let outcome = match deps.policies.reconcile_system(&doc, STARTER_POLICY_REVISION).await {
            Ok(outcome) => outcome,
            // The row exists: it governed decisions perfectly well before this change, so
            // keeping it for one more boot beats refusing to start (SMA-477 D12).
            Err(err) if existing_ids.iter().any(|id| id == &doc.policy_id) => {
                count("failed");
                tracing::error!(policy_id = %doc.policy_id, error = %err, "starter policy reconciliation failed; keeping the stored row for this boot");
                continue;
            }
            // The row is MISSING and could not be written. `AppState::new` guarantees the
            // initial snapshot compiles at least the starter set; booting past this would
            // compile a partial one, which denies everything.
            Err(err) => {
                count("failed");
                tracing::error!(policy_id = %doc.policy_id, error = %err, "failed to seed a missing starter policy; refusing to boot with an incomplete policy set");
                return Err(err);
            }
        };
        count(outcome.metric_label());

        match &outcome {
            StarterPolicyOutcome::Unchanged => {}
            StarterPolicyOutcome::Absent => tracing::info!(policy_id = %doc.policy_id, "seeded starter policy"),
            StarterPolicyOutcome::Reconciled => {
                tracing::info!(policy_id = %doc.policy_id, "converged starter policy to the code-defined content")
            }
            StarterPolicyOutcome::StaleBinary => tracing::info!(
                policy_id = %doc.policy_id,
                revision = STARTER_POLICY_REVISION,
                "starter policy was written by a newer release; leaving it in place"
            ),
            StarterPolicyOutcome::Adopted { content_changed, previous_content } => {
                if *content_changed {
                    tracing::info!(policy_id = %doc.policy_id, "adopted a starter policy with no recorded provenance and converged its content");
                    if let Some(previous) = previous_content {
                        record_reconcile_audit(deps, &doc, "adopted_unfingerprinted", true, previous).await;
                    }
                } else {
                    tracing::debug!(policy_id = %doc.policy_id, "stamped provenance on an already-matching starter policy");
                }
            }
            StarterPolicyOutcome::ExternallyModified { content_changed, previous_content } => {
                tracing::warn!(
                    policy_id = %doc.policy_id,
                    content_changed = *content_changed,
                    "a system-owned starter policy was modified outside this service; converging it back to the code-defined content"
                );
                record_reconcile_audit(deps, &doc, "external_modification", *content_changed, previous_content).await;
            }
        }
    }

    for orphan in deps.policies.orphaned_system_policy_ids(STARTER_POLICY_IDS).await.unwrap_or_default() {
        count("orphaned");
        tracing::warn!(
            policy_id = %orphan,
            "a system-owned policy row is no longer code-defined; it still compiles and still links grants, and DeletePolicy refuses to remove it"
        );
    }
    Ok(())
}

/// One audit entry recording content this boot is about to destroy. Best-effort by design
/// (SMA-477 D9): the convergence already committed, so a failed audit write is logged, never
/// propagated — refusing to start over a bookkeeping failure would be a self-inflicted outage.
async fn record_reconcile_audit<I: IdGenerator, C: Clock>(
    deps: &ReconcileStarterDeps<I, C>,
    doc: &PolicyDocument,
    reason: &str,
    content_changed: bool,
    previous: &paigasus_iam_core::authz::reconcile::PolicyContent,
) {
    let (source, truncated) = truncate_source(&previous.source);
    let entry = AuditEntry {
        id: deps.ids.new_audit_id(),
        occurred_at: deps.clock.now(),
        // No principal authorized this — a code deployment did (the SMA-468 posture).
        actor_prn: None,
        // The same action + Root resource every PutPolicy audit row uses, so these rows are
        // reachable by the standard `AuditFilter::resource_prn` query rather than a private one.
        action: "PutPolicy".to_string(),
        resource_prn: Some(root_prn().canonical()),
        outcome: AuditOutcome::Committed,
        determining_policies: vec![],
        detail: serde_json::json!({
            "policy_id": doc.policy_id,
            "source": "starter_policy_reconcile",
            "reason": reason,
            "content_changed": content_changed,
            "previous_content": {
                "source": source,
                "description": previous.description,
                "truncated": truncated,
            },
        }),
        correlation_id: Some(deps.ids.new_correlation_id()),
    };
    if let Err(err) = deps.audit.record_out_of_band(&entry).await {
        tracing::error!(policy_id = %doc.policy_id, error = %err, "failed to record the starter-policy reconciliation audit entry");
    }
}

/// Reconciles every `authz::roles::system_roles()` row. These columns are introspectable-only,
/// so drift here is cosmetic — logged, never audited.
pub async fn reconcile_roles<I: IdGenerator, C: Clock>(deps: &ReconcileStarterDeps<I, C>) -> Result<(), AuthzError> {
    for role_def in authz_roles::system_roles() {
        match deps.roles.reconcile_role(&role_def).await {
            Ok(RoleOutcome::Inserted) => tracing::info!(role_key = %role_def.key, "seeded system role"),
            Ok(RoleOutcome::Updated) => tracing::info!(role_key = %role_def.key, "converged system role row to the code-defined catalog"),
            Ok(RoleOutcome::Unchanged) => {}
            Err(err) => tracing::error!(role_key = %role_def.key, error = %err, "system role reconciliation failed; keeping the stored row for this boot"),
        }
    }

    let known: Vec<String> = authz_roles::system_roles().into_iter().map(|r| r.key).collect();
    let known_refs: Vec<&str> = known.iter().map(String::as_str).collect();
    for orphan in deps.roles.orphaned_system_role_keys(&known_refs).await.unwrap_or_default() {
        tracing::warn!(role_key = %orphan, "a system role row is no longer code-defined; existing grants of it still resolve");
    }
    Ok(())
}

/// Policies first, then roles: every role template's `policy_id == Role::key ==
/// Role::template_id`, and `role.template_id` carries an FK to `policy.policy_id`
/// (`fk_role_template`), so the referenced policy row must exist before the role row can be
/// inserted.
pub async fn reconcile_starter<I: IdGenerator, C: Clock>(deps: &ReconcileStarterDeps<I, C>) -> Result<(), AuthzError> {
    reconcile_policies(deps).await?;
    reconcile_roles(deps).await
}
```

Note `Utc` may become unused once `seed_role_row` moves out — remove the import if clippy flags it. Delete the old `map_err`, `node_kind_str`, `scope_kinds_json`, and `seed_role_row` from this file (their behaviour now lives in `authz::reconcile` and `PgSystemRoleReconciler`).

The test fake needs the matching method — add to `ScriptedPolicies` an `existing: Vec<String>` field and:

```rust
        async fn existing_policy_ids(&self) -> Result<Vec<String>, AuthzError> {
            Ok(self.existing.clone())
        }
```

`ScriptedPolicies::with` sets `existing` to every starter id, so the scripted cases exercise the
survivable path by default:

```rust
        fn with(outcome: StarterPolicyOutcome) -> Arc<Self> {
            Arc::new(ScriptedPolicies {
                outcome: Mutex::new(Some(outcome)),
                existing: starter_policies().into_iter().map(|d| d.policy_id).collect(),
                ..Default::default()
            })
        }
```

and extend the fatal-seed assertion in `a_convergence_failure_is_skipped_but_a_seeding_failure_is_fatal`:

```rust
        // Seeding failure: an absent policy that cannot be written must stop the replica.
        let seed_fail = Arc::new(ScriptedPolicies { fail: true, existing: vec![], ..Default::default() });
        let d = deps(seed_fail, Arc::new(FakeAuditLog::default()));
        reconcile_policies(&d).await.expect_err("a failure to seed a missing starter policy must fail boot");
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam bootstrap:: --no-tests=pass && cargo clippy --workspace -- -D warnings && cargo fmt --check
```

Expected: PASS. (The workspace will not fully build until Task 8 fixes the call sites — if `cargo clippy --workspace` fails only on `http/mod.rs` and the integration tests calling the old `reconcile_starter(&store, &db)` signature, that is expected; run `cargo nextest run -p paigasus-iam --lib bootstrap:: --no-tests=pass` to confirm the unit tests themselves pass, and finish the build in Task 8.)

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/application/bootstrap.rs rs/crates/services/paigasus-iam/src/application/fakes.rs rs/crates/libs/paigasus-iam-core/src/authz/ports.rs rs/crates/services/paigasus-iam/src/adapters/persistence/pg_policies.rs
git commit -m "feat(rs): converge and report starter policy drift at boot (SMA-477)"
```

---

### Task 8: Wire it at the composition root

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs` (existing call sites)
- Modify: any other `reconcile_starter` caller found by ripgrep

**Interfaces:**
- Consumes: `ReconcileStarterDeps`/`reconcile_starter` (Task 7), `PgSystemRoleReconciler` (Task 5).

- [ ] **Step 1: Find every call site**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
rg -n 'reconcile_starter' rs/
```

Expected: `adapters/http/mod.rs:338` plus several in `tests/authz_bootstrap.rs`.

- [ ] **Step 2: Move `PgAuditLog` construction above the reconcile call**

In `http/mod.rs`, cut the `let audit_log: Arc<dyn AuditLog> = Arc::new(PgAuditLog::new(db.clone()).with_query_window(...));` line (currently below, ~line 400) and paste it immediately **above** the `bootstrap::reconcile_starter(...)` call, keeping its existing doc comment with it.

Build ONE instance, not two — `with_query_window` takes `mut self` and returns `Self`, so constructing a plain instance for reconcile and a windowed one for the query API would create two, breaking the "one store instance, not several" invariant that comment documents. Reconcile simply does not use the window.

- [ ] **Step 3: Replace the call**

```rust
        let policy_store: Arc<dyn PolicyStore> = Arc::new(PgPolicyStore::new(db.clone(), gens.clone()));
        let role_grant_store: Arc<dyn RoleGrantStore> = Arc::new(PgRoleGrantStore::new(db.clone(), gens.clone()));

        // SMA-477: the reconciler shares the SAME `PgPolicyStore` the snapshot reads from, so a
        // converged policy's `policy_gen` bump is observed by this replica's own snapshot and by
        // every other replica's.
        let policy_reconciler: Arc<dyn SystemPolicyReconciler> = Arc::new(PgPolicyStore::new(db.clone(), gens.clone()));
        let role_reconciler: Arc<dyn SystemRoleReconciler> = Arc::new(PgSystemRoleReconciler::new(db.clone()));
        let reconcile_deps = bootstrap::ReconcileStarterDeps {
            policies: policy_reconciler,
            roles: role_reconciler,
            audit: audit_log.clone(),
            ids: KernelIdGenerator,
            clock: SystemClock,
        };
        bootstrap::reconcile_starter(&reconcile_deps).await.map_err(|e| AuthnError::Backend(Box::new(e)))?;
```

Add `SystemPolicyReconciler`, `SystemRoleReconciler` to the `paigasus_iam_core` import list and `PgSystemRoleReconciler` to the persistence import list.

Update the `AppState::new` doc comment (the paragraph at ~line 290-294): the initial snapshot still always compiles at least the starter set, and now that set is also **converged** to the code-defined content on every boot.

- [ ] **Step 4: Update the test call sites**

In `tests/authz_bootstrap.rs`, add a helper and use it everywhere the old two-argument form appeared:

```rust
/// Builds the boot-reconciliation deps over one database, mirroring `AppState::new`'s wiring.
fn reconcile_deps(db: &DatabaseConnection, gens: &Generations) -> paigasus_iam::application::bootstrap::ReconcileStarterDeps<KernelIdGenerator, SystemClock> {
    use paigasus_iam::adapters::persistence::PgSystemRoleReconciler;
    paigasus_iam::application::bootstrap::ReconcileStarterDeps {
        policies: Arc::new(PgPolicyStore::new(db.clone(), gens.clone())),
        roles: Arc::new(PgSystemRoleReconciler::new(db.clone())),
        audit: Arc::new(PgAuditLog::new(db.clone())),
        ids: KernelIdGenerator,
        clock: SystemClock,
    }
}
```

Replace `reconcile_starter(&policy_store, &db).await.unwrap()` with
`reconcile_starter(&reconcile_deps(&db, &gens)).await.unwrap()`, introducing a
`let gens = Generations::memory();` where a test does not already have one and passing that same
handle to `PgPolicyStore::new` so the counter stays shared.

- [ ] **Step 5: Build and run the full IAM suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --workspace && cargo nextest run -p paigasus-iam -p paigasus-iam-core --no-tests=pass && cargo clippy --workspace -- -D warnings && cargo fmt --check
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/
git commit -m "feat(rs): wire boot reconciliation at the composition root (SMA-477)"
```

---

### Task 9: Reserve the starter id namespace in `put_in`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_policies.rs`
- Test: `rs/crates/services/paigasus-iam/tests/authz_policy_store.rs`

**Interfaces:**
- Consumes: `authz::roles::is_starter_policy_id` (Task 2).

**This task MUST come after Task 8.** Until boot stops routing seeding through `PolicyStore::put`, this guard would reject the seed itself and no replica would start.

- [ ] **Step 1: Write the failing test**

Append to `tests/authz_policy_store.rs` (match its existing imports and Docker-skip helper):

```rust
/// SMA-477 D6: the starter ids are reserved even before they are seeded. Without this an
/// operator could occupy one, and a row that is not `system = true` would then be exempt from
/// boot-time convergence forever.
#[tokio::test]
async fn put_rejects_a_reserved_starter_policy_id() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let now = Utc::now();

    let doc = PolicyDocument {
        policy_id: "org_admin".to_string(),
        kind: PolicyKind::Static,
        source: "permit(principal, action, resource);".to_string(),
        description: String::new(),
        system: false,
        created_at: now,
        updated_at: now,
    };

    // Rejected on a FRESH database, i.e. before the id is seeded — the check is on the id, not
    // on any stored row's `system` flag.
    let err = store.put(&doc).await.expect_err("a reserved starter id must be rejected");
    assert!(matches!(err, AuthzError::SystemImmutable(id) if id == "org_admin"), "got {err:?}");

    let forbid = PolicyDocument { policy_id: "forbid-archived-writes".to_string(), ..doc.clone() };
    assert!(matches!(store.put(&forbid).await, Err(AuthzError::SystemImmutable(_))));

    // An operator's own id is unaffected.
    let ok = PolicyDocument { policy_id: "operator-policy".to_string(), ..doc };
    store.put(&ok).await.expect("a non-reserved id must still be accepted");
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test authz_policy_store reserved --no-tests=pass
```

Expected: FAIL — the put succeeds.

- [ ] **Step 3: Add the guard**

In `pg_policies.rs::put_in`, immediately after `validate_policy(&doc.source)?;`:

```rust
        // SMA-477 D6: the starter policy ids are code-owned, reserved whether or not they are
        // seeded yet. Rejecting on the ID (not on a stored row's `system` flag) is what closes
        // the `UPDATE policy SET system = false` bypass — an operator can never create a
        // non-system row at one of these ids for a later release to trip over. Reuses
        // `SystemImmutable` so the existing `TenancyError` and API mappings are unchanged.
        if authz_roles::is_starter_policy_id(&doc.policy_id) {
            return Err(AuthzError::SystemImmutable(doc.policy_id.clone()));
        }
```

Add `use paigasus_iam_core::authz::roles as authz_roles;`. Note `reconcile_system` does **not** call `put_in`, so seeding is unaffected.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam -p paigasus-iam-core --no-tests=pass && cargo clippy --workspace -- -D warnings && cargo fmt --check
```

Expected: PASS. If another test used a starter id for an operator policy, change that test's id — the rejection is the intended behaviour.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/persistence/pg_policies.rs rs/crates/services/paigasus-iam/tests/authz_policy_store.rs
git commit -m "feat(rs): reserve the starter policy id namespace in PutPolicy (SMA-477)"
```

---

### Task 10: End-to-end boot test + stale-comment fixes

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs`
- Modify: `rs/crates/services/paigasus-iam/src/application/bootstrap.rs` (doc comment only)

- [ ] **Step 1: Write the end-to-end test**

Append to `tests/authz_bootstrap.rs`:

```rust
/// The whole boot path: an out-of-band edit is converged AND leaves exactly one audit row that
/// the standard audit query can actually find.
#[tokio::test]
async fn boot_reverts_an_out_of_band_edit_and_records_it_in_the_audit_log() {
    use paigasus_iam_core::authz::model::root_prn;
    use paigasus_iam_core::{AuditFilter, AuditLog};

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let gens = Generations::memory();
    reconcile_starter(&reconcile_deps(&db, &gens)).await.unwrap();

    let target = "forbid-archived-writes";
    let original = stored_source(&db, target).await;
    tamper_policy(&db, target, "permit(principal, action, resource);", Some(&"0".repeat(64))).await;

    reconcile_starter(&reconcile_deps(&db, &gens)).await.unwrap();
    assert_eq!(stored_source(&db, target).await, original, "boot must converge the edited row back");

    let audit = PgAuditLog::new(db.clone());
    let entries = audit
        .query(&AuditFilter {
            actor_prn: None,
            resource_prn: Some(root_prn().canonical()),
            action: Some("PutPolicy".to_string()),
            outcome: None,
            from: Some(Utc::now() - chrono::Duration::days(1)),
            to: None,
            cursor: None,
            limit: 50,
        })
        .await
        .unwrap();

    let ours: Vec<_> = entries.iter().filter(|e| e.detail["source"] == serde_json::json!("starter_policy_reconcile")).collect();
    assert_eq!(ours.len(), 1, "exactly one reconciliation audit row");
    assert_eq!(ours[0].detail["policy_id"], serde_json::json!(target));
    assert_eq!(ours[0].detail["reason"], serde_json::json!("external_modification"));
    assert_eq!(ours[0].detail["previous_content"]["source"], serde_json::json!("permit(principal, action, resource);"));
    assert_eq!(ours[0].actor_prn, None);

    // And a third boot is quiet again.
    reconcile_starter(&reconcile_deps(&db, &gens)).await.unwrap();
    let after = audit
        .query(&AuditFilter {
            actor_prn: None,
            resource_prn: Some(root_prn().canonical()),
            action: Some("PutPolicy".to_string()),
            outcome: None,
            from: Some(Utc::now() - chrono::Duration::days(1)),
            to: None,
            cursor: None,
            limit: 50,
        })
        .await
        .unwrap();
    assert_eq!(
        after.iter().filter(|e| e.detail["source"] == serde_json::json!("starter_policy_reconcile")).count(),
        1,
        "a converged row must not keep auditing every boot"
    );
}
```

- [ ] **Step 2: Fix the stale "seven roles" language**

`system_roles()` returns **eight**. Rename the test `reconcile_seeds_every_starter_policy_and_the_seven_system_roles` to `..._and_the_eight_system_roles`, and fix `bootstrap.rs`'s module doc if any "seven" survived Task 7's rewrite:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
rg -n 'seven system roles|seven_system_roles' rs/
```

Expected after the fix: no matches.

- [ ] **Step 3: Run the suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam -p paigasus-iam-core --no-tests=pass && cargo clippy --workspace -- -D warnings && cargo fmt --check
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-iam/
git commit -m "test(rs): cover the boot reconciliation path end to end (SMA-477)"
```

---

### Task 11: Rewrite the runbook section

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md` (section "Starter-policy drift warning at boot", ~lines 1437-1477)

- [ ] **Step 1: Replace the section**

The documented behaviour no longer exists — replace the whole section (heading through the "Tracked as SMA-477" paragraph) with:

```markdown
### Starter-policy reconciliation at boot

**What happens.** On every boot, `bootstrap::reconcile_policies` converges each starter Cedar
policy row to the code-defined content from `authz::roles::starter_policies()`, and
`reconcile_roles` does the same for the `role` table. These rows are **code-owned**: the
`PutPolicy` API refuses both a persisted `system = true` row and any policy id in the starter
namespace (`authz::roles::STARTER_POLICY_IDS`), so the database is not a supported place to
customize them.

Each policy emits `iam_starter_policy_reconciles_total{outcome=...}`:

| `outcome` | Meaning | Action |
|---|---|---|
| `unchanged` | Content matches and provenance checks out. | None. |
| `seeded` | The row was absent and has been created. | None (expected on a fresh database). |
| `reconciled` | A release changed the policy; the row was converged. | None — this is the routine case that used to warn forever. |
| `adopted` | The row predates the fingerprint column, so its provenance was unknowable. | None, but see below if it also changed content. |
| `stale_binary` | The stored row was written by a NEWER release; this replica left it alone. | Expected briefly during a deploy. Persisting means an old replica is still running. |
| `externally_modified` | Something other than this service wrote the row. Converged and audited. | **Investigate.** |
| `orphaned` | A `system = true` row whose id is no longer code-defined. | Investigate; it still compiles and still links grants, and `DeletePolicy` refuses to remove it. |
| `failed` | Converging one row errored. The stored row was kept and the replica still booted. | Check the ERROR log line. Transient at low volume. |

**`externally_modified` — the one that matters.** It logs

```text
a system-owned starter policy was modified outside this service; converging it back to the code-defined content
```

and writes one `audit_log` entry capturing what was overwritten, because converging destroys the
evidence. Retrieve it with `action = "PutPolicy"` and `resource_prn` = the Root PRN, then match
`detail.source = "starter_policy_reconcile"`; `detail.previous_content.source` holds the
overwritten Cedar source (truncated at 8 KiB, flagged by `detail.previous_content.truncated`).

**Always pass an explicit `from`.** `PgAuditLog::query` applies a default lookback whenever both
`from` and `to` are absent (`audit.query_default_window_days`, default 90), so an unfiltered
query against an older database silently returns nothing.

**What the warning is and is not.** It detects accidental and naive edits. It is a *provenance
hint*, not tamper evidence: the only actor who can modify a `system = true` row is one with
direct SQL access, and that same access recomputes the fingerprint trivially, at which point the
edit reads as a routine code change. Do not treat a quiet log as proof nothing was touched.

**A hand-patched starter policy is reverted on the next replica boot.** There is effectively no
escape hatch: a forked non-system policy can add a `forbid` but can never remove a code-defined
one, and a forked role *template* is never linked by any grant (a grant resolves its template by
`role_key`). Starter policies can be tightened out-of-band and cannot be loosened. If you need a
different starter policy, change the code.

**`adopted` on the first boot after upgrading** is expected: the fingerprint column starts NULL
for every pre-existing row and is stamped on that boot. If the row's content had also drifted, an
audit entry with `reason = "adopted_unfingerprinted"` records what was replaced.

**A pure provenance stamp still bumps `updated_at`** without changing any content, so an
`updated_at` change visible through `ListPolicies` is not by itself evidence of a policy change.
```

- [ ] **Step 2: Fix the pre-existing id typo**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
rg -n 'forbid_archived_writes' docs/
```

Any hit is wrong — the constant is `forbid-archived-writes` (`roles.rs:41`). Fix each.

- [ ] **Step 3: Commit**

```bash
git add docs/ops/RUNBOOK-observability.md
git commit -m "docs(ops): document boot-time starter policy reconciliation (SMA-477)"
```

---

### Task 12: Full CI gate run

**Files:** none (verification only).

- [ ] **Step 1: Run the whole graph exactly as CI does**

Per-project Moon tasks do NOT run the repo-level gates, so this is the only way to catch `:deny`, `:machete`, codegen drift and CODEOWNERS before pushing:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift \
  :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: all green. No new crate and no new dependency were introduced, so `:deny`, `:machete` and `:affected-smoke` should need no waiver.

- [ ] **Step 2: Diagnose any failure**

Moon reports an unattributed "N failed". Find which:

```bash
jq '.actions[] | select(.status=="failed") | .label' .moon/cache/ciReport.json
```

- [ ] **Step 3: Confirm the plan's acceptance criteria**

Re-read the spec's §8 and confirm each is covered by a test that ran. Every one maps to a named test in Tasks 1, 2, 4, 5, 7, 9, or 10.

- [ ] **Step 4: Commit any fixes**

```bash
git add -A
git commit -m "fix(rs): satisfy the repo CI gates for starter policy reconciliation (SMA-477)"
```

---

## Self-review notes

**Spec coverage.** D1 → Tasks 4/5/7. D2 → Tasks 1/3 (`content_fingerprint`, CHECK constraint) + Task 11 (the stated limit). D3 → Task 1 (`Adopted`), Task 7 (INFO + audit). D4 → Task 1's ordering test. D5 → Tasks 4/5. D6 → Task 1 (`!system` branch), Task 4 (restore), Task 9 (reserved namespace). D7 → Tasks 1/5. D8 → Tasks 6/7 (audit shape, `root_prn`, truncation, metric). D9 → Task 7 (`FailingAuditLog`). D10 → Task 4 (`content_changed()` gate) + its `policy_gen` test. D11 → Task 2 (revision + pin), Task 1 (`StaleBinary`), Task 4 (defer test). D12 → Task 4 (`lock_timeout`), Task 7 (skip vs fatal). D13 → Tasks 4/5 (`orphaned_*`), Task 7 (WARN, no audit). §5 → Task 11. §8 AC1-11 → Task 12 step 3.

**Type consistency.** `existing_policy_ids` is declared on the port in Task 4, implemented on `PgPolicyStore` in Task 4, faked in Task 7 — one name throughout. `content_fingerprint(kind, source, description)` has the same three-argument shape in Tasks 1, 2, and 4. `StarterPolicyOutcome`'s variants and `metric_label()` strings match between Task 1's definition, Task 6's documented label set, and Task 7's match arms. `map_db_err` is introduced by renaming `pg_policies.rs`'s private `map_err` in Task 5 — Task 4's code still says `map_err` because it is written before that rename, which is correct in sequence.

**Ordering constraint that must not be reordered.** Task 9 (the reserved-namespace guard in `put_in`) MUST land after Task 8. Before Task 8, boot still seeds through `PolicyStore::put`, and the guard would reject the seed itself — no replica would start.
