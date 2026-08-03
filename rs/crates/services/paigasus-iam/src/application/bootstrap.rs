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

    /// Returns a scripted outcome for every policy, recording what it was asked to reconcile.
    #[derive(Default)]
    struct ScriptedPolicies {
        outcome: Mutex<Option<StarterPolicyOutcome>>,
        fail: bool,
        seen: Mutex<Vec<String>>,
        orphans: Vec<String>,
        existing: Vec<String>,
        existing_calls: AtomicUsize,
    }

    impl ScriptedPolicies {
        fn with(outcome: StarterPolicyOutcome) -> Arc<Self> {
            Arc::new(ScriptedPolicies {
                outcome: Mutex::new(Some(outcome)),
                existing: starter_policies().into_iter().map(|d| d.policy_id).collect(),
                ..Default::default()
            })
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
        async fn existing_policy_ids(&self) -> Result<Vec<String>, AuthzError> {
            self.existing_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.existing.clone())
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
        ReconcileStarterDeps {
            policies,
            roles: Arc::new(ScriptedRoles),
            audit,
            ids: KernelIdGenerator,
            clock: SystemClock,
        }
    }

    fn tampered() -> StarterPolicyOutcome {
        StarterPolicyOutcome::ExternallyModified {
            content_changed: true,
            previous_content: PolicyContent {
                kind: PolicyKind::Static,
                source: "permit(principal, action, resource);".to_string(),
                description: "old".to_string(),
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
        assert_eq!(e.detail["previous_content"]["source"], serde_json::json!("permit(principal, action, resource);"));
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

    /// The survivable-vs-fatal split above is only sound if the snapshot predates every write:
    /// re-reading it per policy would let a row this boot just seeded make a LATER failure on a
    /// still-missing row look survivable. One call, before the loop.
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
