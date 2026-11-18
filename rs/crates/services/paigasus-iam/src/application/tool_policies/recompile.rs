// SPDX-License-Identifier: Apache-2.0

//! The tool policy re-compile job (SMA-781, spec section 8).
//!
//! A new compiler generation or schema revision makes the stored live sets stale. The job
//! compiles each stale set again, one at a time, with no transaction open:
//!
//! - The same output: a new live version with the origin `recompile` and its published event.
//! - Another output, or a rejection: a held row with a report. The old version stays live, and
//!   an admin accepts or discards the held row (`held.rs`). A held row enqueues no event (Q3).
//!
//! **One run at a time.** The job takes the [`RecompileLock`](paigasus_iam_core::tool_policy::RecompileLock)
//! first, and a held lock is [`RecompileRun::AlreadyRunning`]. The job releases the guard
//! explicitly on every path it owns (F16). A release error is a warning, with no retry: the
//! end of the lock is then not confirmed, and the lock goes when its connection closes.
//!
//! **An older replica writes nothing (H12).** When a stored row has a higher generation or
//! schema revision than the running ones, this replica is older than another one, and the run
//! stops with [`RecompileRun::OlderReplica`].
//!
//! **A proposal is made once (H6, Q1).** The job skips a set when a held or discarded row
//! already exists for the same live version, generation and schema revision.
//!
//! **The RPC run is its own task (F15).** IAM drops a request at a fixed 30 s timeout. The RPC
//! spawns the job, and the task owns the lock guard, so a dropped RPC future does not stop the
//! job. The caller then reads the results with `ListHeldToolPolicyVersions` and the log.

use std::future::Future;

use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::tool_policy::{
    CompiledOutput, HeldReport, HeldStatus, HeldVersion, ReportRejection, TOOL_SCHEMA_REVISION, ToolEntry, ToolPolicySetKey, ToolPolicyState, ToolPolicyVersion, VersionOrigin, diff_outputs,
    failed_report, outputs_identical,
};
use paigasus_iam_core::{Action, Clock, IdGenerator};
use paigasus_kernel::Prn;
use paigasus_observability::names;
use serde_json::json;

use super::sets::decode_compiled;
use super::{Author, Compiled, SetTarget, ToolPolicyService, origin_str};
use crate::application::error::TenancyError;

/// The `created_by` of a row that the boot run of the job writes (plan H15).
pub const RECOMPILE_JOB_ACTOR: &str = "system:tool-policy-recompile";

/// The audit action of each set result that wrote a row (H16).
const AUDIT_ACTION: &str = "RunToolPolicyRecompile";

/// The result of one stale set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecompileOutcome {
    /// The same output: `version` is the new live version.
    Identical { version: u64 },
    /// Another output: `version` is the new held row.
    HeldDifferent { version: u64 },
    /// The compile rejected the set: `version` is the new held row, with no output.
    HeldFailed { version: u64 },
    /// A held or discarded row already exists for this live version and these compiler values.
    AlreadyProposed,
    /// The set changed after the read, or its organization is gone. Nothing was written.
    Conflict,
    /// The set could not be done: the compiler was busy or unavailable, or a read or a write of
    /// this set failed (for example a live row whose compiled bytes do not decode). Nothing was
    /// written, a warning names the set, and the next run tries again. The job goes on with the
    /// next set.
    Error,
}

/// The result of one stale set, with its key and the live version that the job compiled again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetRecompile {
    pub key: ToolPolicySetKey,
    pub base_version: u64,
    pub outcome: RecompileOutcome,
}

/// The result of one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecompileRun {
    /// One result per stale set, in the repository order (the floor first, then by org UUID).
    Ran(Vec<SetRecompile>),
    /// Another holder has the lock. Nothing was compiled.
    AlreadyRunning,
    /// A stored row is newer than this replica (H12). Nothing was written.
    OlderReplica { stored_generation: u32, stored_schema_revision: u32 },
}

/// Every value of the `result` label of the metric. `main` primes each series at zero from this
/// list, so that the first increment of a series is visible to `increase()`.
pub const RECOMPILE_RESULT_LABELS: [&str; 5] = ["identical", "held_different", "held_failed", "conflict", "error"];

/// The `result` label of the metric. `None` for `AlreadyProposed`, which writes nothing new.
pub(crate) fn outcome_label(o: &RecompileOutcome) -> Option<&'static str> {
    match o {
        RecompileOutcome::Identical { .. } => Some("identical"),
        RecompileOutcome::HeldDifferent { .. } => Some("held_different"),
        RecompileOutcome::HeldFailed { .. } => Some("held_failed"),
        RecompileOutcome::Conflict => Some("conflict"),
        RecompileOutcome::Error => Some("error"),
        RecompileOutcome::AlreadyProposed => None,
    }
}

/// The version number that an outcome wrote, if any.
fn written_version(o: &RecompileOutcome) -> Option<u64> {
    match o {
        RecompileOutcome::Identical { version } | RecompileOutcome::HeldDifferent { version } | RecompileOutcome::HeldFailed { version } => Some(*version),
        _ => None,
    }
}

fn set_label(key: &ToolPolicySetKey) -> String {
    match key {
        ToolPolicySetKey::Floor => "floor".to_owned(),
        ToolPolicySetKey::Tenant(org) => format!("tenant:{org}"),
    }
}

/// A held write: the output (`None` for a failed compile), the report and the outcome to give.
struct HeldWrite<'a> {
    out: Option<&'a CompiledOutput>,
    report: HeldReport,
    result: &'static str,
}

impl<I: IdGenerator + 'static, C: Clock + 'static> ToolPolicyService<I, C> {
    /// The RPC entry: authorizes `ManageToolPolicyRecompile` at `root_prn()`, then runs. The job
    /// runs in a spawned task that owns the lock guard (F15), and this call waits for it.
    pub async fn run_recompile(&self, actor: &Prn) -> Result<RecompileRun, TenancyError> {
        self.authorize.check(actor, Action::ManageToolPolicyRecompile, &root_prn()).await?;
        let svc = self.clone();
        let actor = actor.clone();
        // The process shutdown stops the job between two sets, like the boot run.
        let mut shutdown = self.shutdown.clone();
        let stop = async move {
            match shutdown.as_mut() {
                Some(rx) => {
                    let _ = rx.changed().await;
                }
                None => std::future::pending().await,
            }
        };
        let job = tokio::spawn(async move { svc.run_job(Some(&actor), stop).await });
        job.await.map_err(|error| {
            tracing::error!(%error, "the tool policy re-compile task ended abnormally");
            TenancyError::Internal
        })?
    }

    /// The boot entry: no actor (H15). Stops between two sets when `stop` resolves.
    pub async fn run_recompile_at_boot(&self, stop: impl Future<Output = ()> + Send) -> Result<RecompileRun, TenancyError> {
        self.run_job(None, stop).await
    }

    async fn run_job(&self, actor: Option<&Prn>, stop: impl Future<Output = ()> + Send) -> Result<RecompileRun, TenancyError> {
        let Some(guard) = self.recompile_lock.try_acquire().await? else {
            tracing::info!("tool policy re-compile: another replica runs the job");
            return Ok(RecompileRun::AlreadyRunning);
        };
        let run = self.run_locked(&Author::of(actor), stop).await;
        // Explicit on every path, the stop and the error paths too (F16).
        if let Err(error) = guard.release().await {
            tracing::warn!(%error, "tool policy re-compile: the release of the lock is not confirmed");
        }
        run
    }

    async fn run_locked(&self, author: &Author, stop: impl Future<Output = ()> + Send) -> Result<RecompileRun, TenancyError> {
        let generation = self.compiler.generation();
        let (stored_generation, stored_schema_revision) = self.repo.max_stored_revisions().await?;
        if stored_generation > generation || stored_schema_revision > TOOL_SCHEMA_REVISION {
            tracing::warn!(
                stored_generation,
                stored_schema_revision,
                running_generation = generation,
                running_schema_revision = TOOL_SCHEMA_REVISION,
                "tool policy re-compile: a stored row is newer than this replica; the job writes nothing"
            );
            return Ok(RecompileRun::OlderReplica {
                stored_generation,
                stored_schema_revision,
            });
        }
        let stale = self.repo.stale_live_versions(generation, TOOL_SCHEMA_REVISION).await?;
        let mut stop = std::pin::pin!(stop);
        let mut results = Vec::with_capacity(stale.len());
        for live in stale {
            // Polled once between two sets: a set that started always ends.
            let stopped = tokio::select! {
                biased;
                () = &mut stop => true,
                () = std::future::ready(()) => false,
            };
            if stopped {
                tracing::info!(done = results.len(), "tool policy re-compile: stopped between two sets");
                break;
            }
            let outcome = self.recompile_set(author, &live).await;
            if let Some(label) = outcome_label(&outcome) {
                metrics::counter!(names::IAM_TOOL_POLICY_RECOMPILE_RESULTS_TOTAL, "result" => label).increment(1);
            }
            tracing::info!(
                set = %set_label(&live.key),
                base_version = live.version,
                outcome = ?outcome,
                new_version = ?written_version(&outcome),
                "tool policy re-compile result"
            );
            results.push(SetRecompile {
                key: live.key,
                base_version: live.version,
                outcome,
            });
        }
        Ok(RecompileRun::Ran(results))
    }

    /// One stale set. An error of this set is its outcome, not an error of the run: one bad set
    /// must not stop the sets after it (a stale tenant set refuses every catalog write).
    async fn recompile_set(&self, author: &Author, live: &ToolPolicyVersion) -> RecompileOutcome {
        match self.try_recompile_set(author, live).await {
            Ok(outcome) => outcome,
            Err(TenancyError::ToolPolicyVersionConflict) => RecompileOutcome::Conflict,
            Err(error) => {
                tracing::warn!(set = %set_label(&live.key), base_version = live.version, error = ?error, "tool policy re-compile: the set failed; it stays out of date");
                RecompileOutcome::Error
            }
        }
    }

    /// One stale set (spec section 8, steps a to e). A version conflict of the write is `Err`;
    /// the caller maps it to `Conflict`.
    async fn try_recompile_set(&self, author: &Author, live: &ToolPolicyVersion) -> Result<RecompileOutcome, TenancyError> {
        // a. A proposal for this live version and these compiler values exists (H6).
        if self.repo.proposal_exists(&live.key, live.version, self.compiler.generation(), TOOL_SCHEMA_REVISION).await? {
            return Ok(RecompileOutcome::AlreadyProposed);
        }
        // b. The organization (its stored PRN for the event and the audit entry, F9), the state
        // and the catalog.
        let target = match live.key {
            ToolPolicySetKey::Tenant(org) => match self.orgs.find(org).await? {
                Some(view) => SetTarget {
                    key: live.key,
                    resource: view.node.id.prn().clone(),
                },
                None => return Ok(RecompileOutcome::Conflict),
            },
            ToolPolicySetKey::Floor => SetTarget { key: live.key, resource: root_prn() },
        };
        let state = self.repo.state(&live.key).await?;
        if state.live_version != live.version {
            return Ok(RecompileOutcome::Conflict);
        }
        let catalog: Vec<ToolEntry> = match live.key {
            ToolPolicySetKey::Tenant(org) => self.repo.catalog(org).await?.into_iter().map(|s| s.entry).collect(),
            ToolPolicySetKey::Floor => Vec::new(),
        };

        // c. Compile, with no transaction open. A busy or unavailable compiler is `Error`.
        let compiled = self.compile(target.kind(), &live.source, &catalog).await?;

        // d and e. Write one result.
        match &compiled {
            Compiled::Accepted(out) => {
                let old = decode_compiled(&live.compiled)?;
                if outputs_identical(&old, out) {
                    self.write_identical(author, &target, &state, live, out, &catalog).await
                } else {
                    let write = HeldWrite {
                        out: Some(out),
                        report: diff_outputs(&old, out),
                        result: "held_different",
                    };
                    self.write_held(author, &target, &state, live, write, &catalog).await
                }
            }
            Compiled::Rejected(rejections) => {
                let report = failed_report(
                    rejections
                        .iter()
                        .map(|r| ReportRejection {
                            policy_id: r.policy_id.clone(),
                            code: r.reason.code().to_owned(),
                            message: r.message.clone(),
                        })
                        .collect(),
                );
                let write = HeldWrite {
                    out: None,
                    report,
                    result: "held_failed",
                };
                self.write_held(author, &target, &state, live, write, &catalog).await
            }
        }
    }

    /// The same output: the next live version, with the origin `recompile` and its event.
    async fn write_identical(
        &self,
        author: &Author,
        target: &SetTarget,
        state: &ToolPolicyState,
        live: &ToolPolicyVersion,
        out: &CompiledOutput,
        catalog: &[ToolEntry],
    ) -> Result<RecompileOutcome, TenancyError> {
        let tx = self.uow.begin().await?;
        let now = self.clock.now();
        let corr = self.ids.new_correlation_id();
        let version = self
            .write_new_live_version(&*tx, author, target, state, &live.source, out, catalog, VersionOrigin::Recompile, corr, now)
            .await?;
        let detail = json!({
            "set": target.audit_set(),
            "version": version.version,
            "origin": origin_str(VersionOrigin::Recompile),
            "result": "identical",
            "base_version": live.version,
        });
        self.audit.record(&*tx, &self.audit_entry(author, AUDIT_ACTION, &target.resource, detail, now, corr)).await?;
        tx.commit().await?;
        Ok(RecompileOutcome::Identical { version: version.version })
    }

    /// Another output or a rejection: a held row at the next number. The live version stays.
    /// `insert_held_in` discards an older held row of the set in the same transaction (H8).
    async fn write_held(
        &self,
        author: &Author,
        target: &SetTarget,
        state: &ToolPolicyState,
        live: &ToolPolicyVersion,
        write: HeldWrite<'_>,
        catalog: &[ToolEntry],
    ) -> Result<RecompileOutcome, TenancyError> {
        let tx = self.uow.begin().await?;
        let locked = self.lock_and_recheck(&*tx, &target.key, state).await?;
        let now = self.clock.now();
        let corr = self.ids.new_correlation_id();
        let number = locked.next_version();
        let catalog_revision = match target.key {
            ToolPolicySetKey::Tenant(_) => Some(state.catalog_revision),
            ToolPolicySetKey::Floor => None,
        };
        let held = HeldVersion {
            key: target.key,
            version: number,
            base_version: live.version,
            source: live.source.clone(),
            compiled: write.out.map(|out| self.encode_output(&target.key, number, out, catalog_revision, catalog)),
            compiler_generation: self.compiler.generation(),
            schema_revision: TOOL_SCHEMA_REVISION,
            catalog_revision,
            status: HeldStatus::Held,
            report: write.report,
            created_by: author.created_by.clone(),
            created_at: now,
        };
        self.repo.insert_held_in(&*tx, &held).await?;
        let detail = json!({
            "set": target.audit_set(),
            "version": number,
            "origin": null,
            "result": write.result,
            "base_version": live.version,
        });
        self.audit.record(&*tx, &self.audit_entry(author, AUDIT_ACTION, &target.resource, detail, now, corr)).await?;
        tx.commit().await?;
        Ok(match write.out {
            Some(_) => RecompileOutcome::HeldDifferent { version: number },
            None => RecompileOutcome::HeldFailed { version: number },
        })
    }
}

#[cfg(test)]
mod tests {
    use std::future::{pending, poll_fn};
    use std::task::Poll;
    use std::time::Duration;

    use super::super::tests::{FLOOR_DENY, Harness, MAX_WINDOW_MS, accepted, audit_actions, bare_harness, harness, output_with_history, tenant};
    use super::*;
    use crate::application::fakes::ConcurrentChange;
    use crate::application::tool_policies::RejectionReason;
    use crate::application::tool_policies::sets::{CompiledMeta, decode_compiled, encode_compiled};
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    use paigasus_iam_core::authz::model::root_prn;
    use paigasus_iam_core::tool_policy::{
        CompileOutcome, CompiledOutput, CompilerFailure, HeldResult, HeldStatus, HeldVersion, RecompileLock, Rejection, ReportRejection, STARTER_TOOL_POLICY_SOURCE, SetKind, TOOL_SCHEMA_REVISION,
        ToolPolicyRepository, ToolPolicySetKey, ToolPolicyVersion, VersionOrigin, VersionStatus, base_schema_json, failed_report, tenant_schema_json,
    };
    use paigasus_iam_core::{Action, DomainEvent, EventType};
    use paigasus_observability::names;
    use serde_json::json;
    use uuid::Uuid;

    /// A metric name, its labels and its value.
    type Sample = (String, Vec<(String, String)>, DebugValue);

    /// The identity output of `source` for the set of `key`, as the fake compiler gives it.
    fn identity(key: &ToolPolicySetKey, source: &str) -> CompiledOutput {
        CompiledOutput {
            cedar_policies: source.to_owned(),
            cedar_schema_json: match key {
                ToolPolicySetKey::Tenant(_) => tenant_schema_json(&[]),
                ToolPolicySetKey::Floor => base_schema_json(),
            },
            conditions: Vec::new(),
        }
    }

    /// A live version 1 of the set of `key`, stored with no service call. Its output is the
    /// identity output of `source`.
    fn seed(h: &Harness, key: ToolPolicySetKey, source: &str, generation: u32, schema_revision: u32) {
        let catalog_revision = matches!(key, ToolPolicySetKey::Tenant(_)).then_some(0);
        let meta = CompiledMeta {
            key,
            version: 1,
            compiler_generation: generation,
            schema_revision,
            catalog_revision,
            max_window_ms: MAX_WINDOW_MS,
            catalog_names: Vec::new(),
        };
        h.repo.seed_live(ToolPolicyVersion {
            key,
            version: 1,
            source: source.to_owned(),
            compiled: encode_compiled(&meta, &identity(&key, source)),
            compiler_generation: generation,
            schema_revision,
            catalog_revision,
            status: VersionStatus::Live,
            origin: VersionOrigin::Save,
            created_by: h.actor.canonical(),
            created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        });
    }

    /// A stale live tenant set (generation 0) with the starter source.
    fn stale_tenant(h: &Harness) {
        seed(h, tenant(h), STARTER_TOOL_POLICY_SOURCE, 0, TOOL_SCHEMA_REVISION);
    }

    /// A stale live floor (generation 0).
    fn stale_floor(h: &Harness) {
        seed(h, ToolPolicySetKey::Floor, FLOOR_DENY, 0, TOOL_SCHEMA_REVISION);
    }

    /// The next compile gives the starter with one more policy, `extra`.
    fn push_different(h: &Harness) {
        let mut out = identity(&tenant(h), STARTER_TOOL_POLICY_SOURCE);
        out.cedar_policies = format!("{STARTER_TOOL_POLICY_SOURCE}\n@id(\"extra\")\nforbid(principal, action, resource);\n");
        h.compiler.push(Ok(CompileOutcome::Accepted(out)));
    }

    fn with_recompile_action(h: Harness) -> Harness {
        h.authz.allow(Action::ManageToolPolicyRecompile, &root_prn());
        h
    }

    async fn boot_run(h: &Harness) -> RecompileRun {
        h.svc.run_recompile_at_boot(pending()).await.unwrap()
    }

    fn ran(run: RecompileRun) -> Vec<SetRecompile> {
        match run {
            RecompileRun::Ran(results) => results,
            other => panic!("expected Ran, got {other:?}"),
        }
    }

    fn one(key: ToolPolicySetKey, outcome: RecompileOutcome) -> Vec<SetRecompile> {
        vec![SetRecompile { key, base_version: 1, outcome }]
    }

    fn events(h: &Harness) -> Vec<DomainEvent> {
        h.outbox.0.lock().unwrap().clone()
    }

    async fn live(h: &Harness, key: &ToolPolicySetKey) -> ToolPolicyVersion {
        h.repo.live(key).await.unwrap().unwrap()
    }

    /// Nothing was written: no commit, no event, no audit entry, no insert, no held row.
    fn assert_nothing_written(h: &Harness) {
        assert_eq!(h.uow.commits(), 0);
        assert!(events(h).is_empty());
        assert!(audit_actions(h).is_empty());
        assert_eq!(h.repo.insert_attempts(), 0);
        assert!(h.repo.held_rows(&tenant(h)).is_empty());
        assert!(h.repo.held_rows(&ToolPolicySetKey::Floor).is_empty());
    }

    #[tokio::test]
    async fn an_identical_result_stores_a_new_live_version_with_origin_recompile() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let h = harness();
        stale_tenant(&h);
        let run = boot_run(&h).await;
        assert_eq!(ran(run), one(tenant(&h), RecompileOutcome::Identical { version: 2 }));
        let v = live(&h, &tenant(&h)).await;
        assert_eq!(
            (v.version, v.origin, v.compiler_generation, v.schema_revision, v.catalog_revision),
            (2, VersionOrigin::Recompile, 1, TOOL_SCHEMA_REVISION, Some(0))
        );
        assert_eq!(v.source, STARTER_TOOL_POLICY_SOURCE);
        assert_eq!(decode_compiled(&v.compiled).unwrap(), identity(&tenant(&h), STARTER_TOOL_POLICY_SOURCE));
        let events = events(&h);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::ToolPolicySetPublished);
        assert_eq!(events[0].aggregate_prn, h.org_prn.canonical());
        assert_eq!(events[0].payload, json!({"org_id": h.org.to_string(), "version": 2, "origin": "recompile"}));
        assert_eq!(h.uow.commits(), 1);
        let samples: Vec<Sample> = snapshotter
            .snapshot()
            .into_vec()
            .into_iter()
            .map(|(k, _, _, v)| (k.key().name().to_owned(), k.key().labels().map(|l| (l.key().to_owned(), l.value().to_owned())).collect(), v))
            .collect();
        assert_eq!(
            samples,
            vec![(
                names::IAM_TOOL_POLICY_RECOMPILE_RESULTS_TOTAL.to_owned(),
                vec![("result".to_owned(), "identical".to_owned())],
                DebugValue::Counter(1)
            )]
        );
    }

    #[tokio::test]
    async fn a_different_result_keeps_the_old_version_live_and_holds_the_new_output() {
        let h = harness();
        stale_tenant(&h);
        push_different(&h);
        assert_eq!(ran(boot_run(&h).await), one(tenant(&h), RecompileOutcome::HeldDifferent { version: 2 }));
        let held = h.repo.held(&tenant(&h)).await.unwrap().unwrap();
        assert_eq!((held.version, held.base_version, held.status), (2, 1, HeldStatus::Held));
        assert_eq!((held.compiler_generation, held.schema_revision, held.catalog_revision), (1, TOOL_SCHEMA_REVISION, Some(0)));
        assert_eq!(held.source, STARTER_TOOL_POLICY_SOURCE);
        assert_eq!(held.report.result, HeldResult::Different);
        assert_eq!(held.report.policies.added, vec!["extra".to_owned()]);
        assert!(decode_compiled(held.compiled.as_deref().unwrap()).unwrap().cedar_policies.contains("@id(\"extra\")"));
        // The live version stays, and a held row enqueues no event (Q3).
        let v = live(&h, &tenant(&h)).await;
        assert_eq!((v.version, v.compiler_generation), (1, 0));
        assert!(events(&h).is_empty());
        let state = h.repo.state(&tenant(&h)).await.unwrap();
        assert_eq!((state.live_version, state.last_version), (1, 2));
    }

    #[tokio::test]
    async fn a_rejected_compile_holds_a_failed_row_with_its_rejections() {
        let h = harness();
        stale_tenant(&h);
        h.compiler.push(Ok(CompileOutcome::Rejected(vec![Rejection {
            policy_id: Some("p".to_owned()),
            line: Some(2),
            column: Some(5),
            reason: RejectionReason::PolicyInvalid.code().to_owned(),
            message: "bad".to_owned(),
        }])));
        assert_eq!(ran(boot_run(&h).await), one(tenant(&h), RecompileOutcome::HeldFailed { version: 2 }));
        let held = h.repo.held(&tenant(&h)).await.unwrap().unwrap();
        assert_eq!(held.compiled, None);
        assert_eq!(held.report.result, HeldResult::Failed);
        assert_eq!(
            held.report.rejections,
            vec![ReportRejection {
                policy_id: Some("p".to_owned()),
                code: RejectionReason::PolicyInvalid.code().to_owned(),
                message: "bad".to_owned(),
            }]
        );
        assert_eq!(live(&h, &tenant(&h)).await.version, 1);
    }

    #[tokio::test]
    async fn a_set_stored_with_a_history_rule_is_held_failed() {
        let h = harness();
        stale_tenant(&h);
        h.compiler
            .push(Ok(CompileOutcome::Accepted(output_with_history(STARTER_TOOL_POLICY_SOURCE, tenant_schema_json(&[]), &["rate"]))));
        assert_eq!(ran(boot_run(&h).await), one(tenant(&h), RecompileOutcome::HeldFailed { version: 2 }));
        let held = h.repo.held(&tenant(&h)).await.unwrap().unwrap();
        assert_eq!(held.report.rejections.len(), 1);
        assert_eq!(held.report.rejections[0].policy_id.as_deref(), Some("rate"));
        assert_eq!(held.report.rejections[0].code, RejectionReason::HistoryUnsupported.code());
    }

    #[tokio::test]
    async fn a_restart_does_not_write_a_second_held_row() {
        let h = harness();
        stale_tenant(&h);
        push_different(&h);
        boot_run(&h).await;
        push_different(&h);
        assert_eq!(ran(boot_run(&h).await), one(tenant(&h), RecompileOutcome::AlreadyProposed));
        assert_eq!(h.repo.held_rows(&tenant(&h)).len(), 1);
        assert_eq!(h.compiler.calls(), 1);
        assert_eq!(audit_actions(&h).len(), 1);
    }

    #[tokio::test]
    async fn a_discarded_proposal_is_not_held_again_on_the_next_run() {
        let h = harness();
        stale_tenant(&h);
        push_different(&h);
        boot_run(&h).await;
        h.svc.discard_held(&h.actor, crate::application::tool_policies::HeldTarget::Tenant(h.org), 2).await.unwrap();
        push_different(&h);
        assert_eq!(ran(boot_run(&h).await), one(tenant(&h), RecompileOutcome::AlreadyProposed));
        assert_eq!(h.repo.held(&tenant(&h)).await.unwrap(), None);
        assert_eq!(h.compiler.calls(), 1);
    }

    #[tokio::test]
    async fn a_newer_generation_replaces_an_older_held_row() {
        let h = harness();
        stale_tenant(&h);
        h.repo.seed_held(HeldVersion {
            key: tenant(&h),
            version: 2,
            base_version: 1,
            source: STARTER_TOOL_POLICY_SOURCE.to_owned(),
            compiled: None,
            compiler_generation: 1,
            schema_revision: TOOL_SCHEMA_REVISION,
            catalog_revision: Some(0),
            status: HeldStatus::Held,
            report: failed_report(Vec::new()),
            created_by: RECOMPILE_JOB_ACTOR.to_owned(),
            created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        });
        h.compiler.set_generation(2);
        push_different(&h);
        assert_eq!(ran(boot_run(&h).await), one(tenant(&h), RecompileOutcome::HeldDifferent { version: 3 }));
        let rows: Vec<(u64, HeldStatus, u32)> = h.repo.held_rows(&tenant(&h)).iter().map(|r| (r.version, r.status, r.compiler_generation)).collect();
        assert_eq!(rows, vec![(2, HeldStatus::Discarded, 1), (3, HeldStatus::Held, 2)]);
    }

    #[tokio::test]
    async fn a_current_live_set_is_not_compiled() {
        let h = harness();
        seed(&h, tenant(&h), STARTER_TOOL_POLICY_SOURCE, 1, TOOL_SCHEMA_REVISION);
        assert_eq!(ran(boot_run(&h).await), Vec::new());
        assert_eq!(h.compiler.calls(), 0);
        assert_nothing_written(&h);
    }

    #[tokio::test]
    async fn the_floor_is_recompiled_against_the_base_schema() {
        let h = harness();
        stale_floor(&h);
        assert_eq!(ran(boot_run(&h).await), one(ToolPolicySetKey::Floor, RecompileOutcome::Identical { version: 2 }));
        let request = &h.compiler.requests()[0];
        assert_eq!((request.kind, request.source.as_str()), (SetKind::Floor, FLOOR_DENY));
        assert_eq!(request.schema_json, base_schema_json());
        let v = live(&h, &ToolPolicySetKey::Floor).await;
        assert_eq!((v.version, v.origin, v.catalog_revision), (2, VersionOrigin::Recompile, None));
        let events = events(&h);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::ToolFloorPublished);
        assert_eq!(events[0].payload, json!({"version": 2, "origin": "recompile"}));
    }

    #[tokio::test]
    async fn a_save_between_the_read_and_the_lock_is_a_conflict_and_writes_nothing() {
        for change in [ConcurrentChange::LiveVersion, ConcurrentChange::CatalogRevision] {
            // The identical path (a live write) and the different path (a held write).
            for different in [false, true] {
                let h = harness();
                stale_tenant(&h);
                if different {
                    push_different(&h);
                }
                h.repo.change_before_next_lock(change);
                assert_eq!(ran(boot_run(&h).await), one(tenant(&h), RecompileOutcome::Conflict), "{change:?}, different {different}");
                assert_nothing_written(&h);
            }
        }
    }

    #[tokio::test]
    async fn a_set_of_an_absent_organization_is_a_conflict() {
        let h = harness();
        let gone = ToolPolicySetKey::Tenant(Uuid::from_u128(999));
        seed(&h, gone, STARTER_TOOL_POLICY_SOURCE, 0, TOOL_SCHEMA_REVISION);
        assert_eq!(ran(boot_run(&h).await), one(gone, RecompileOutcome::Conflict));
        assert_eq!(h.compiler.calls(), 0);
        assert_nothing_written(&h);
    }

    #[tokio::test]
    async fn a_set_that_fails_to_read_is_an_error_and_the_job_goes_on() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let h = harness();
        // The floor goes first. Its compiled bytes do not decode.
        h.repo.seed_live(ToolPolicyVersion {
            key: ToolPolicySetKey::Floor,
            version: 1,
            source: FLOOR_DENY.to_owned(),
            compiled: vec![0xff; 4],
            compiler_generation: 0,
            schema_revision: TOOL_SCHEMA_REVISION,
            catalog_revision: None,
            status: VersionStatus::Live,
            origin: VersionOrigin::Save,
            created_by: h.actor.canonical(),
            created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        });
        stale_tenant(&h);
        let results = ran(boot_run(&h).await);
        assert_eq!(
            results,
            vec![
                SetRecompile {
                    key: ToolPolicySetKey::Floor,
                    base_version: 1,
                    outcome: RecompileOutcome::Error,
                },
                SetRecompile {
                    key: tenant(&h),
                    base_version: 1,
                    outcome: RecompileOutcome::Identical { version: 2 },
                },
            ]
        );
        assert_eq!(live(&h, &ToolPolicySetKey::Floor).await.version, 1);
        assert_eq!(h.lock.releases(), 1);
        let mut samples: Vec<Sample> = snapshotter
            .snapshot()
            .into_vec()
            .into_iter()
            .map(|(k, _, _, v)| (k.key().name().to_owned(), k.key().labels().map(|l| (l.key().to_owned(), l.value().to_owned())).collect(), v))
            .collect();
        samples.sort_by(|a, b| a.1.cmp(&b.1));
        let name = names::IAM_TOOL_POLICY_RECOMPILE_RESULTS_TOTAL.to_owned();
        assert_eq!(
            samples,
            vec![
                (name.clone(), vec![("result".to_owned(), "error".to_owned())], DebugValue::Counter(1)),
                (name, vec![("result".to_owned(), "identical".to_owned())], DebugValue::Counter(1)),
            ]
        );
    }

    #[tokio::test]
    async fn a_save_of_a_later_set_during_the_run_is_a_conflict_and_the_save_stays() {
        const SAVED: &str = "@id(\"saved\")\nforbid(principal, action, resource);\n";
        let h = harness();
        stale_floor(&h);
        stale_tenant(&h);
        let gate = h.compiler.gate();
        let svc = h.svc.clone();
        let job = tokio::spawn(async move { svc.run_recompile_at_boot(pending()).await });
        // The job read the stale sets and waits in the compile of the floor.
        for _ in 0..1000 {
            if h.compiler.calls() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(h.compiler.calls(), 1);
        // An admin saves the tenant set now.
        h.compiler.remove_gate();
        let saved = accepted(h.svc.put_set(&h.actor, h.org, SAVED, 1).await);
        assert_eq!(saved.version, 2);
        gate.add_permits(1);
        let results = ran(job.await.unwrap().unwrap());
        assert_eq!(
            results,
            vec![
                SetRecompile {
                    key: ToolPolicySetKey::Floor,
                    base_version: 1,
                    outcome: RecompileOutcome::Identical { version: 2 },
                },
                SetRecompile {
                    key: tenant(&h),
                    base_version: 1,
                    outcome: RecompileOutcome::Conflict,
                },
            ]
        );
        let v = live(&h, &tenant(&h)).await;
        assert_eq!((v.version, v.source.as_str(), v.origin), (2, SAVED, VersionOrigin::Save));
        // The job did not compile the tenant set.
        assert_eq!(h.compiler.calls(), 2);
    }

    #[tokio::test]
    async fn a_busy_or_unavailable_compiler_is_an_error_and_the_job_goes_on() {
        for failure in [CompilerFailure::Busy, CompilerFailure::Unavailable("down".to_owned())] {
            let h = harness();
            // The floor goes first.
            stale_floor(&h);
            stale_tenant(&h);
            h.compiler.push(Err(failure));
            let results = ran(boot_run(&h).await);
            assert_eq!(
                results,
                vec![
                    SetRecompile {
                        key: ToolPolicySetKey::Floor,
                        base_version: 1,
                        outcome: RecompileOutcome::Error,
                    },
                    SetRecompile {
                        key: tenant(&h),
                        base_version: 1,
                        outcome: RecompileOutcome::Identical { version: 2 },
                    },
                ]
            );
            assert_eq!(live(&h, &ToolPolicySetKey::Floor).await.version, 1);
            assert!(!h.lock.held());
        }
    }

    #[tokio::test]
    async fn a_held_lock_is_already_running_and_compiles_nothing() {
        let h = harness();
        stale_tenant(&h);
        let _other = h.lock.try_acquire().await.unwrap().unwrap();
        assert_eq!(boot_run(&h).await, RecompileRun::AlreadyRunning);
        assert_eq!(h.compiler.calls(), 0);
        assert_nothing_written(&h);
        // The lock of the other holder stays.
        assert!(h.lock.held());
        assert_eq!(h.lock.releases(), 0);
    }

    #[tokio::test]
    async fn a_higher_stored_generation_is_older_replica_and_writes_nothing() {
        let h = harness();
        seed(&h, tenant(&h), STARTER_TOOL_POLICY_SOURCE, 2, TOOL_SCHEMA_REVISION);
        assert_eq!(
            boot_run(&h).await,
            RecompileRun::OlderReplica {
                stored_generation: 2,
                stored_schema_revision: TOOL_SCHEMA_REVISION,
            }
        );
        assert_eq!(h.compiler.calls(), 0);
        assert_nothing_written(&h);
        assert!(!h.lock.held());
        assert_eq!(h.lock.releases(), 1);
    }

    #[tokio::test]
    async fn a_higher_stored_schema_revision_is_older_replica() {
        let h = harness();
        seed(&h, tenant(&h), STARTER_TOOL_POLICY_SOURCE, 1, TOOL_SCHEMA_REVISION + 1);
        assert_eq!(
            boot_run(&h).await,
            RecompileRun::OlderReplica {
                stored_generation: 1,
                stored_schema_revision: TOOL_SCHEMA_REVISION + 1,
            }
        );
        assert_eq!(h.compiler.calls(), 0);
        assert_nothing_written(&h);
        assert_eq!(h.lock.releases(), 1);
    }

    #[tokio::test]
    async fn the_rpc_entry_needs_manage_tool_policy_recompile_at_root() {
        let h = bare_harness();
        stale_tenant(&h);
        let err = h.svc.run_recompile(&h.actor).await.unwrap_err();
        assert_eq!(err.code(), TenancyError::Forbidden.code());
        assert!(!h.lock.held());
        assert_eq!(h.lock.releases(), 0);
        assert_eq!(h.compiler.calls(), 0);

        // The action at the org is not enough.
        h.authz.allow(Action::ManageToolPolicyRecompile, &h.org_prn);
        assert_eq!(h.svc.run_recompile(&h.actor).await.unwrap_err().code(), TenancyError::Forbidden.code());

        let h = with_recompile_action(h);
        assert_eq!(ran(h.svc.run_recompile(&h.actor).await.unwrap()), one(tenant(&h), RecompileOutcome::Identical { version: 2 }));
        assert_eq!(h.lock.releases(), 1);
    }

    #[tokio::test]
    async fn the_boot_run_has_no_actor_and_the_system_created_by() {
        let h = harness();
        stale_tenant(&h);
        boot_run(&h).await;
        let v = live(&h, &tenant(&h)).await;
        assert_eq!(v.created_by, RECOMPILE_JOB_ACTOR);
        let entries = h.audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].actor_prn, None);
        assert_eq!(events(&h)[0].actor_prn, None);
    }

    #[tokio::test]
    async fn the_rpc_run_writes_as_its_caller() {
        let h = with_recompile_action(harness());
        stale_tenant(&h);
        push_different(&h);
        h.svc.run_recompile(&h.actor).await.unwrap();
        let held = h.repo.held(&tenant(&h)).await.unwrap().unwrap();
        assert_eq!(held.created_by, h.actor.canonical());
        let entries = h.audit.0.lock().unwrap().clone();
        assert_eq!(entries[0].actor_prn.as_deref(), Some(h.actor.canonical().as_str()));
    }

    #[tokio::test]
    async fn each_written_result_has_one_audit_entry() {
        let h = harness();
        stale_floor(&h);
        stale_tenant(&h);
        // The floor compiles first (identity, so identical); the tenant then differs.
        h.compiler.push(Ok(CompileOutcome::Accepted(identity(&ToolPolicySetKey::Floor, FLOOR_DENY))));
        push_different(&h);
        boot_run(&h).await;
        let entries = h.audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.action == "RunToolPolicyRecompile"));
        assert_eq!(entries[0].resource_prn.as_deref(), Some(root_prn().canonical().as_str()));
        assert_eq!(
            entries[0].detail,
            json!({"set": "floor", "version": 2, "origin": "recompile", "result": "identical", "base_version": 1})
        );
        assert_eq!(entries[1].resource_prn.as_deref(), Some(h.org_prn.canonical().as_str()));
        assert_eq!(entries[1].detail, json!({"set": "tenant", "version": 2, "origin": null, "result": "held_different", "base_version": 1}));
        // The event of the live write shares the correlation id of its audit entry.
        assert_eq!(events(&h)[0].correlation_id, entries[0].correlation_id);
        assert_eq!(h.uow.commits(), 2);
    }

    #[tokio::test]
    async fn the_boot_run_stops_between_sets_and_releases_the_lock() {
        let h = harness();
        stale_floor(&h);
        stale_tenant(&h);
        let compiler = h.compiler.clone();
        // Ready once the first set compiled. The job polls it only between two sets.
        let stop = poll_fn(move |_| if compiler.calls() >= 1 { Poll::Ready(()) } else { Poll::Pending });
        let results = ran(h.svc.run_recompile_at_boot(stop).await.unwrap());
        assert_eq!(results, one(ToolPolicySetKey::Floor, RecompileOutcome::Identical { version: 2 }));
        assert_eq!(h.compiler.calls(), 1);
        assert_eq!(live(&h, &tenant(&h)).await.version, 1);
        assert!(!h.lock.held());
        assert_eq!(h.lock.releases(), 1);
    }

    #[tokio::test]
    async fn a_dropped_rpc_future_does_not_stop_the_job() {
        // F15: IAM drops a request at its 30 s timeout. The job runs in its own task, so it ends
        // the set, writes it and releases the lock.
        let h = with_recompile_action(harness());
        stale_tenant(&h);
        let gate = h.compiler.gate();
        let timed_out = tokio::time::timeout(Duration::from_millis(50), h.svc.run_recompile(&h.actor)).await;
        assert!(timed_out.is_err(), "the RPC must still wait for the gated compile");
        assert!(h.lock.held());
        assert_eq!(h.compiler.calls(), 1);
        gate.add_permits(1);
        for _ in 0..1000 {
            if !h.lock.held() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(!h.lock.held());
        assert_eq!(h.lock.releases(), 1);
        let v = live(&h, &tenant(&h)).await;
        assert_eq!((v.version, v.origin), (2, VersionOrigin::Recompile));
    }

    #[test]
    fn outcome_labels_are_the_five_metric_values() {
        let labels: Vec<Option<&str>> = [
            RecompileOutcome::Identical { version: 2 },
            RecompileOutcome::HeldDifferent { version: 2 },
            RecompileOutcome::HeldFailed { version: 2 },
            RecompileOutcome::Conflict,
            RecompileOutcome::Error,
            RecompileOutcome::AlreadyProposed,
        ]
        .iter()
        .map(outcome_label)
        .collect();
        assert_eq!(labels, vec![Some("identical"), Some("held_different"), Some("held_failed"), Some("conflict"), Some("error"), None]);
        // The public list that `main` primes is the same set, in the same order.
        let listed: Vec<Option<&str>> = RECOMPILE_RESULT_LABELS.iter().copied().map(Some).collect();
        assert_eq!(listed[..], labels[..5]);
    }

    #[tokio::test]
    async fn the_rpc_run_stops_between_two_sets_at_shutdown() {
        let h = with_recompile_action(harness());
        stale_floor(&h);
        stale_tenant(&h);
        let (tx, rx) = tokio::sync::watch::channel(());
        let svc = h.svc.clone().with_shutdown(rx);
        // The shutdown fires before the first set: the run ends with no result and frees the lock.
        tx.send(()).unwrap();
        let results = ran(svc.run_recompile(&h.actor).await.unwrap());
        assert!(results.is_empty());
        assert_eq!(h.compiler.calls(), 0);
        assert!(!h.lock.held());
    }

    /// The composition root wires the job, and no other test reads `main.rs`: the suite stays
    /// green when `main` loses the boot run, the label priming or the shutdown watch of the RPC
    /// run. Modelled on `the_composition_root_still_migrates_under_the_lock` in `migration_lock.rs`:
    /// comment lines are stripped first, so a comment that names a call does not satisfy it.
    #[test]
    fn the_composition_root_still_runs_the_recompile_job() {
        const MAIN: &str = include_str!("../../main.rs");
        let code = MAIN.lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        assert!(code.contains(".with_shutdown("), "main.rs must give the tool policy service the shutdown watch");
        assert!(code.contains("in RECOMPILE_RESULT_LABELS"), "main.rs must prime each result series at zero");
        assert!(code.contains(".run_recompile_at_boot("), "main.rs must run the re-compile job at boot");
    }
}
