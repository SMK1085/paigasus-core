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

use metrics::counter;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::authz::reconcile::{RoleOutcome, StarterPolicyOutcome};
use paigasus_iam_core::authz::roles::{self as authz_roles, STARTER_POLICY_IDS, STARTER_POLICY_REVISION};
use paigasus_iam_core::{AuditEntry, AuditLog, AuditOutcome, AuthzError, Clock, IdGenerator, PolicyDocument, SystemPolicyReconciler, SystemRoleReconciler};
use paigasus_observability::names;
use std::sync::Arc;

/// Per-field cap on the overwritten content copied into an audit entry — applied to the source
/// AND the description. The value is attacker-influenced text landing in an append-only table,
/// so it is bounded rather than trusted.
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

/// Caps one copied text field at [`MAX_AUDITED_SOURCE_BYTES`], walking BACK to the nearest char
/// boundary so `text[..end]` can never split a multi-byte character — slicing mid-character
/// panics, and this runs inside `AppState::new`, so the panic would kill the replica before it
/// served a request. The overwritten text is attacker-influenced, so it may well be the operator
/// who chose where byte 8192 lands. A walked-back result is SHORTER than the cap.
fn truncate_audited_text(text: &str) -> (String, bool) {
    if text.len() <= MAX_AUDITED_SOURCE_BYTES {
        return (text.to_string(), false);
    }
    let mut end = MAX_AUDITED_SOURCE_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// Why a `reconcile_system` failure is (or is not) fatal, decided against the pre-loop snapshot
/// of persisted ids. Split out as a pure function so each branch — and, critically, the message
/// each one selects — is unit-testable: the three cases are genuinely different operational
/// stories, and reporting one as another sends on-call the wrong way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureKind {
    /// The row is persisted: it governed decisions perfectly well before this change.
    Survivable,
    /// The snapshot was readable and this id was NOT in it — the row really is missing.
    FatalMissingRow,
    /// The snapshot could not be read at all, so this row's existence is UNKNOWN. Fail closed,
    /// but never claim the row is missing: it usually is not.
    FatalUnknownSnapshot,
}

/// `snapshot == None` means the pre-loop read itself failed (see [`reconcile_policies`]).
fn classify_failure(snapshot: Option<&[String]>, policy_id: &str) -> FailureKind {
    match snapshot {
        Some(ids) if ids.iter().any(|id| id == policy_id) => FailureKind::Survivable,
        Some(_) => FailureKind::FatalMissingRow,
        None => FailureKind::FatalUnknownSnapshot,
    }
}

/// Reconciles every `authz::roles::starter_policies()` document. Only a failure to SEED an
/// absent policy propagates (see the module docs).
pub async fn reconcile_policies<I: IdGenerator, C: Clock>(deps: &ReconcileStarterDeps<I, C>) -> Result<(), AuthzError> {
    // Captured once, BEFORE any convergence, so every policy is judged against ONE consistent
    // basis and the whole loop costs one full-table read rather than nine.
    //
    // A FAILED read degrades to `None`, which [`classify_failure`] treats as fatal — the
    // conservative direction, since the alternative is booting a replica with an incomplete
    // policy set that denies everything. It is warned about rather than swallowed: a blip here
    // against an intact database turns every later failure fatal, and an operator reading only
    // the fatal line would otherwise be told the policy set is incomplete when it is not.
    let existing_ids: Option<Vec<String>> = match deps.policies.existing_policy_ids().await {
        Ok(ids) => Some(ids),
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not read the persisted policy-id snapshot; every starter policy reconciliation failure this boot will be treated as fatal because no row can be proven to already exist"
            );
            None
        }
    };

    for doc in authz_roles::starter_policies() {
        let outcome = match deps.policies.reconcile_system(&doc, STARTER_POLICY_REVISION).await {
            Ok(outcome) => outcome,
            Err(err) => {
                count("failed");
                match classify_failure(existing_ids.as_deref(), &doc.policy_id) {
                    // The row exists: it governed decisions perfectly well before this change,
                    // so keeping it for one more boot beats refusing to start (SMA-477 D12).
                    FailureKind::Survivable => {
                        tracing::error!(policy_id = %doc.policy_id, error = %err, "starter policy reconciliation failed; keeping the stored row for this boot");
                        continue;
                    }
                    // The row is MISSING and could not be written. `AppState::new` guarantees
                    // the initial snapshot compiles at least the starter set; booting past this
                    // would compile a partial one, which denies everything.
                    FailureKind::FatalMissingRow => {
                        tracing::error!(policy_id = %doc.policy_id, error = %err, "failed to seed a missing starter policy; refusing to boot with an incomplete policy set");
                        return Err(err);
                    }
                    // Deliberately NOT the message above: the row is probably fine, we simply
                    // could not prove it. Says so, and points at the warning that explains why.
                    FailureKind::FatalUnknownSnapshot => {
                        tracing::error!(
                            policy_id = %doc.policy_id,
                            error = %err,
                            "starter policy reconciliation failed and the policy-id snapshot was unreadable (see the earlier warning), so this row cannot be proven to exist; refusing to boot rather than risk an incomplete policy set"
                        );
                        return Err(err);
                    }
                }
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
    // BOTH copied fields are capped: they come from the same externally-modified row, so the
    // description is exactly as attacker-influenced as the source. Two flags rather than one —
    // `truncated` has always meant "the source was truncated" and stays that way.
    let (source, truncated) = truncate_audited_text(&previous.source);
    let (description, description_truncated) = truncate_audited_text(&previous.description);
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
                "description": description,
                "truncated": truncated,
                "description_truncated": description_truncated,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::clock::SystemClock;
    use crate::adapters::id::KernelIdGenerator;
    use crate::application::fakes::{FailingAuditLog, FakeAuditLog};
    use paigasus_iam_core::authz::model::PolicyKind;
    use paigasus_iam_core::authz::reconcile::PolicyContent;
    use paigasus_iam_core::authz::roles::starter_policies;
    // Only the `SystemRoleReconciler` fake below names `Role`; the module's own code infers it
    // from `system_roles()`, so importing it up there would be an unused import (clippy).
    use paigasus_iam_core::Role;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Every starter `policy_id`, i.e. the snapshot a healthy database returns on a warm boot.
    fn all_starter_ids() -> Vec<String> {
        starter_policies().into_iter().map(|d| d.policy_id).collect()
    }

    fn backend_error(msg: &'static str) -> AuthzError {
        AuthzError::Backend(Box::new(std::io::Error::other(msg)))
    }

    /// Returns a scripted outcome for every policy, recording what it was asked to reconcile.
    #[derive(Default)]
    struct ScriptedPolicies {
        outcome: Mutex<Option<StarterPolicyOutcome>>,
        fail: bool,
        seen: Mutex<Vec<String>>,
        orphans: Vec<String>,
        existing: Vec<String>,
        existing_calls: AtomicUsize,
        /// Simulates the pre-loop snapshot read itself failing, INDEPENDENTLY of `existing` —
        /// so a fixture can hold "every row is really there, but we could not read the list".
        existing_fails: bool,
        /// The `known` set the orphan scan was called with, captured so a test can prove the
        /// scan ran and was handed the code-defined ids.
        orphan_known: Mutex<Vec<String>>,
    }

    impl ScriptedPolicies {
        fn with(outcome: StarterPolicyOutcome) -> Arc<Self> {
            Arc::new(ScriptedPolicies {
                outcome: Mutex::new(Some(outcome)),
                existing: all_starter_ids(),
                ..Default::default()
            })
        }
    }

    #[async_trait::async_trait]
    impl SystemPolicyReconciler for ScriptedPolicies {
        async fn reconcile_system(&self, doc: &PolicyDocument, _revision: u32) -> Result<StarterPolicyOutcome, AuthzError> {
            self.seen.lock().unwrap().push(doc.policy_id.clone());
            if self.fail {
                return Err(backend_error("db down"));
            }
            Ok(self.outcome.lock().unwrap().clone().unwrap_or(StarterPolicyOutcome::Unchanged))
        }
        async fn orphaned_system_policy_ids(&self, known: &[&str]) -> Result<Vec<String>, AuthzError> {
            *self.orphan_known.lock().unwrap() = known.iter().map(|k| (*k).to_string()).collect();
            Ok(self.orphans.clone())
        }
        async fn existing_policy_ids(&self) -> Result<Vec<String>, AuthzError> {
            self.existing_calls.fetch_add(1, Ordering::SeqCst);
            if self.existing_fails {
                return Err(backend_error("snapshot read failed"));
            }
            Ok(self.existing.clone())
        }
    }

    #[derive(Default)]
    struct ScriptedRoles {
        fail: bool,
        outcome: Option<RoleOutcome>,
        seen: Mutex<Vec<String>>,
        orphans: Vec<String>,
        orphan_known: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl SystemRoleReconciler for ScriptedRoles {
        async fn reconcile_role(&self, role: &Role) -> Result<RoleOutcome, AuthzError> {
            self.seen.lock().unwrap().push(role.key.clone());
            if self.fail {
                return Err(backend_error("role table down"));
            }
            Ok(self.outcome.unwrap_or(RoleOutcome::Unchanged))
        }
        async fn orphaned_system_role_keys(&self, known: &[&str]) -> Result<Vec<String>, AuthzError> {
            *self.orphan_known.lock().unwrap() = known.iter().map(|k| (*k).to_string()).collect();
            Ok(self.orphans.clone())
        }
    }

    fn deps(policies: Arc<ScriptedPolicies>, audit: Arc<dyn AuditLog>) -> ReconcileStarterDeps<KernelIdGenerator, SystemClock> {
        deps_with_roles(policies, Arc::new(ScriptedRoles::default()), audit)
    }

    fn deps_with_roles(policies: Arc<ScriptedPolicies>, roles: Arc<ScriptedRoles>, audit: Arc<dyn AuditLog>) -> ReconcileStarterDeps<KernelIdGenerator, SystemClock> {
        ReconcileStarterDeps {
            policies,
            roles,
            audit,
            ids: KernelIdGenerator,
            clock: SystemClock,
        }
    }

    fn tampered() -> StarterPolicyOutcome {
        tampered_with("permit(principal, action, resource);".to_string(), "old".to_string())
    }

    fn tampered_with(source: String, description: String) -> StarterPolicyOutcome {
        StarterPolicyOutcome::ExternallyModified {
            content_changed: true,
            previous_content: PolicyContent {
                kind: PolicyKind::Static,
                source,
                description,
            },
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
        assert_eq!(e.detail["policy_id"], serde_json::json!(starter_policies()[0].policy_id), "the row must name the policy it is about");
        assert_eq!(e.detail["content_changed"], serde_json::json!(true));
        assert_eq!(e.detail["previous_content"]["source"], serde_json::json!("permit(principal, action, resource);"));
        assert_eq!(e.detail["previous_content"]["description"], serde_json::json!("old"));
    }

    /// A row hand-edited to exactly the code-defined value is still an external edit, and is
    /// still audited — but `content_changed` must report the truth rather than a hardcoded
    /// `true`, or the entry claims content was destroyed when none was.
    #[tokio::test]
    async fn an_external_edit_that_changed_nothing_audits_with_content_changed_false() {
        let entries = FakeAuditLog::default();
        let d = deps(
            ScriptedPolicies::with(StarterPolicyOutcome::ExternallyModified {
                content_changed: false,
                previous_content: PolicyContent {
                    kind: PolicyKind::Static,
                    source: "identical".to_string(),
                    description: String::new(),
                },
            }),
            Arc::new(entries.clone()),
        );
        reconcile_policies(&d).await.unwrap();

        let rows = entries.0.lock().unwrap();
        assert_eq!(rows.len(), starter_policies().len(), "provenance alone is still worth one row");
        assert_eq!(rows[0].detail["content_changed"], serde_json::json!(false));
    }

    #[tokio::test]
    async fn an_adopted_row_audits_only_when_its_content_actually_changed() {
        let changed = FakeAuditLog::default();
        let d = deps(
            ScriptedPolicies::with(StarterPolicyOutcome::Adopted {
                content_changed: true,
                previous_content: Some(PolicyContent {
                    kind: PolicyKind::Static,
                    source: "old".to_string(),
                    description: String::new(),
                }),
            }),
            Arc::new(changed.clone()),
        );
        reconcile_policies(&d).await.unwrap();
        assert_eq!(changed.0.lock().unwrap().len(), starter_policies().len());
        assert_eq!(changed.0.lock().unwrap()[0].detail["reason"], serde_json::json!("adopted_unfingerprinted"));

        let stamped = FakeAuditLog::default();
        let d = deps(
            ScriptedPolicies::with(StarterPolicyOutcome::Adopted {
                content_changed: false,
                previous_content: None,
            }),
            Arc::new(stamped.clone()),
        );
        reconcile_policies(&d).await.unwrap();
        assert!(stamped.0.lock().unwrap().is_empty(), "a pure fingerprint stamp is not an event");

        // The classifier only attaches `previous_content` when the content actually changed, so
        // this pair is unreachable through `classify_starter_policy` — which is precisely what
        // the `content_changed` guard defends against, and the only fixture that can falsify it.
        // Without this case, mutating that guard to `if true` leaves the suite green, because
        // the `if let Some(..)` inside it never fires on a `previous_content: None` fixture.
        let defensive = FakeAuditLog::default();
        let d = deps(
            ScriptedPolicies::with(StarterPolicyOutcome::Adopted {
                content_changed: false,
                previous_content: Some(PolicyContent {
                    kind: PolicyKind::Static,
                    source: "a misbehaving adapter's leftover snapshot".to_string(),
                    description: String::new(),
                }),
            }),
            Arc::new(defensive.clone()),
        );
        reconcile_policies(&d).await.unwrap();
        assert!(
            defensive.0.lock().unwrap().is_empty(),
            "content_changed: false must suppress the audit even when a snapshot is attached"
        );
    }

    #[tokio::test]
    async fn routine_outcomes_write_no_audit_rows() {
        for outcome in [
            StarterPolicyOutcome::Unchanged,
            StarterPolicyOutcome::Reconciled,
            StarterPolicyOutcome::Absent,
            StarterPolicyOutcome::StaleBinary,
        ] {
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
                previous_content: PolicyContent {
                    kind: PolicyKind::Static,
                    source: huge,
                    description: String::new(),
                },
            }),
            Arc::new(entries.clone()),
        );
        reconcile_policies(&d).await.unwrap();

        let rows = entries.0.lock().unwrap();
        let src = rows[0].detail["previous_content"]["source"].as_str().unwrap();
        assert_eq!(src.len(), MAX_AUDITED_SOURCE_BYTES);
        assert_eq!(rows[0].detail["previous_content"]["truncated"], serde_json::json!(true));
    }

    /// The ASCII fixture above cannot reach the char-boundary walk-back: byte 8192 of an all-`x`
    /// string is always a boundary, so deleting the walk-back leaves that test green. A tampered
    /// Cedar comment carrying any multi-byte text is the real case — and slicing mid-character
    /// PANICS inside `AppState::new`, killing the replica.
    #[tokio::test]
    async fn a_multi_byte_previous_source_truncates_without_splitting_a_character() {
        // A 3-byte char, so `MAX_AUDITED_SOURCE_BYTES % 3 == 2` lands strictly inside one.
        let huge = "→".repeat(MAX_AUDITED_SOURCE_BYTES / 3 + 10);
        assert!(huge.len() > MAX_AUDITED_SOURCE_BYTES, "fixture must exceed the cap");
        assert!(!huge.is_char_boundary(MAX_AUDITED_SOURCE_BYTES), "fixture must put the cap mid-character or it proves nothing");

        let entries = FakeAuditLog::default();
        let d = deps(ScriptedPolicies::with(tampered_with(huge.clone(), String::new())), Arc::new(entries.clone()));
        reconcile_policies(&d).await.unwrap();

        let rows = entries.0.lock().unwrap();
        let src = rows[0].detail["previous_content"]["source"].as_str().unwrap();
        assert!(
            src.len() < MAX_AUDITED_SOURCE_BYTES,
            "the cap lands mid-character, so the result must walk BACK below it, got {}",
            src.len()
        );
        assert!(huge.starts_with(src), "truncation must yield a prefix, never re-encoded or mangled text");
        assert_eq!(rows[0].detail["previous_content"]["truncated"], serde_json::json!(true));
    }

    /// The description comes off the same externally-modified row as the source, so it is
    /// equally attacker-influenced and equally capped — with its OWN flag, so neither field's
    /// marker can stand in for the other's.
    #[tokio::test]
    async fn an_oversized_previous_description_is_truncated_and_separately_marked() {
        let huge = "d".repeat(MAX_AUDITED_SOURCE_BYTES + 500);
        let entries = FakeAuditLog::default();
        let d = deps(ScriptedPolicies::with(tampered_with("small".to_string(), huge)), Arc::new(entries.clone()));
        reconcile_policies(&d).await.unwrap();

        let rows = entries.0.lock().unwrap();
        let previous = &rows[0].detail["previous_content"];
        assert_eq!(previous["description"].as_str().unwrap().len(), MAX_AUDITED_SOURCE_BYTES);
        assert_eq!(previous["description_truncated"], serde_json::json!(true));
        assert_eq!(previous["truncated"], serde_json::json!(false), "the source flag must track the source, not the description");
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

        // Seeding failure: an absent policy that cannot be written must stop the replica.
        let seed_fail = Arc::new(ScriptedPolicies {
            fail: true,
            existing: vec![],
            ..Default::default()
        });
        let d = deps(seed_fail, Arc::new(FakeAuditLog::default()));
        reconcile_policies(&d).await.expect_err("a failure to seed a missing starter policy must fail boot");
    }

    /// A failure classified against the snapshot has three genuinely different operational
    /// stories, and reporting one as another sends on-call the wrong way — "refusing to boot
    /// with an incomplete policy set" about a database holding all nine rows invites destructive
    /// remediation. Collapsing the unreadable case into the missing-row case reddens here.
    #[test]
    fn a_failure_is_classified_against_the_snapshot_and_an_unreadable_one_is_its_own_case() {
        let ids = vec!["forbid_archived_writes".to_string()];
        assert_eq!(classify_failure(Some(&ids), "forbid_archived_writes"), FailureKind::Survivable);
        assert_eq!(classify_failure(Some(&ids), "org_admin"), FailureKind::FatalMissingRow);
        assert_eq!(
            classify_failure(None, "forbid_archived_writes"),
            FailureKind::FatalUnknownSnapshot,
            "an unreadable snapshot must never be reported as a missing row"
        );
    }

    /// A connection blip while reading the snapshot, against a database holding every row. The
    /// degradation must be fail-CLOSED (we cannot prove the row is there) — and the fixture
    /// populates `existing` with all nine ids, so a fake that ignored `existing_fails` would
    /// take the survivable path and return `Ok`, reddening this test.
    #[tokio::test]
    async fn an_unreadable_id_snapshot_makes_every_failure_fatal() {
        let scripted = Arc::new(ScriptedPolicies {
            fail: true,
            existing_fails: true,
            existing: all_starter_ids(),
            ..Default::default()
        });
        let d = deps(scripted.clone(), Arc::new(FakeAuditLog::default()));
        reconcile_policies(&d)
            .await
            .expect_err("an unreadable snapshot must fail closed, not be swallowed into the survivable path");
        assert_eq!(scripted.existing_calls.load(Ordering::SeqCst), 1, "the failed read is still only attempted once");
    }

    /// Requirement 7: an orphaned system row is REPORTED, never audited. `audit_log` is
    /// append-only and this scan runs on every replica on every boot, so one audit row per
    /// orphan would compound without bound. Nothing else pins that today.
    #[tokio::test]
    async fn orphaned_policy_rows_are_scanned_and_reported_but_never_audited() {
        let scripted = Arc::new(ScriptedPolicies {
            outcome: Mutex::new(Some(StarterPolicyOutcome::Unchanged)),
            existing: all_starter_ids(),
            orphans: vec!["retired_starter_a".to_string(), "retired_starter_b".to_string()],
            ..Default::default()
        });
        let entries = FakeAuditLog::default();
        let d = deps(scripted.clone(), Arc::new(entries.clone()));
        reconcile_policies(&d).await.unwrap();

        assert_eq!(
            *scripted.orphan_known.lock().unwrap(),
            STARTER_POLICY_IDS.iter().map(|k| (*k).to_string()).collect::<Vec<_>>(),
            "the scan must run, and against the code-defined id set"
        );
        assert!(entries.0.lock().unwrap().is_empty(), "an orphan is logged and counted, never written to the append-only audit table");
    }

    /// The role half is untested otherwise: a role-table failure must NOT stop a replica (the
    /// columns are introspectable-only), every role is still attempted, orphan keys are scanned,
    /// and — as above — nothing here audits.
    #[tokio::test]
    async fn a_role_failure_never_stops_boot_and_orphan_roles_are_reported_not_audited() {
        let roles = Arc::new(ScriptedRoles {
            fail: true,
            orphans: vec!["retired_role".to_string()],
            ..Default::default()
        });
        let entries = FakeAuditLog::default();
        let d = deps_with_roles(ScriptedPolicies::with(StarterPolicyOutcome::Unchanged), roles.clone(), Arc::new(entries.clone()));
        reconcile_roles(&d).await.expect("a role reconciliation failure must never stop a replica starting");

        let expected_keys: Vec<String> = authz_roles::system_roles().into_iter().map(|r| r.key).collect();
        assert_eq!(*roles.seen.lock().unwrap(), expected_keys, "every role is still attempted after one fails");
        assert_eq!(*roles.orphan_known.lock().unwrap(), expected_keys, "the orphan scan must run, against the code-defined role catalog");
        assert!(entries.0.lock().unwrap().is_empty(), "role drift is cosmetic — logged, never audited");
    }

    /// One read, not nine: every policy is judged against ONE consistent basis, and the whole
    /// loop costs a single full-table read. (Re-reading per policy would not flip any single
    /// classification — the guard compares against the failing doc's OWN id — but it would make
    /// the basis drift mid-loop and multiply the cost.)
    #[tokio::test]
    async fn the_existing_id_snapshot_is_read_once_before_the_loop() {
        let scripted = ScriptedPolicies::with(StarterPolicyOutcome::Reconciled);
        let d = deps(scripted.clone(), Arc::new(FakeAuditLog::default()));
        reconcile_policies(&d).await.unwrap();
        assert!(
            scripted.seen.lock().unwrap().len() > 1,
            "the fixture must span several policies for a per-iteration read to be distinguishable"
        );
        assert_eq!(
            scripted.existing_calls.load(Ordering::SeqCst),
            1,
            "the existing-id snapshot must be captured once, before any convergence"
        );
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
