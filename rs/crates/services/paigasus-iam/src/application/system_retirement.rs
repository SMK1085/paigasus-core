// SPDX-License-Identifier: Apache-2.0

//! `SystemRetirementService` — the Root-only use case that retires an orphaned system-owned
//! `policy` row (and its `role` row, if any) whose id the code catalog no longer defines
//! (SMA-481). Modelled field-for-field on `DeadLetterService` (`application/dead_letters.rs`):
//! a `SystemRetirementDeps` bag, `Arc`-held ports, `#[derive(Clone)]`, an `audit_entry` helper,
//! and Root-only enforcement inside the service via `self.authorize.check(actor,
//! Action::RetireSystemPolicy, &root_prn())` rather than the Cedar schema's shared `appliesTo`
//! block.
//!
//! **D3 — the destructive deletes never touch `PolicyStore::delete_in`.** That trait's
//! `SystemImmutable` guard is exactly what must keep holding for the public `DeletePolicy` API.
//! This service drives `SystemRowRetirer::delete_role_in`/`delete_policy_in` instead — a
//! deliberately narrower, Root-only port whose whole point is to bypass that guard for rows
//! already established as orphaned (`paigasus_iam_core::authz::retirement`'s own module doc).
//!
//! **D4 — retirement never deletes a grant, which is why a template is inert but a static
//! policy is not.** When grants of a retiring role key survive, this service refuses and names
//! them (`RetireOutcome::Blocked`) rather than cascading the delete: a bulk cascade would have
//! to reproduce `RoleService::revoke`'s own audit row, `DomainEvent`, and anti-escalation check
//! to avoid being exactly the "silently dropping grants is an authorization change" the
//! originating issue warns against. With zero grants linked, a template contributes NOTHING to
//! the compiled `PolicySet`, so removing it provably cannot change any decision. A STATIC
//! policy is different in kind, not degree: it compiles unconditionally and is evaluated on
//! every request regardless of any grant, so deleting one changes decisions fleet-wide the
//! instant it commits (for `forbid-archived-writes`, archived resources become writable). A
//! static retirement is therefore refused (`RetireOutcome::NeedsAcknowledgement`, carrying the
//! `kind`/`source`/`description` that would be destroyed AS the preview) unless the caller
//! passes `ack = true`; the flag is a no-op on a template, so an operator never has to know
//! which kind they hold before calling.
//!
//! **D6 — the role row is locked `FOR UPDATE`, not merely read.** A replica running an older
//! binary mid-deploy still defines the retiring key in its code catalog and will happily grant
//! it. `SELECT … FOR UPDATE` takes PostgreSQL's `FOR KEY SHARE` conflict on the row, so such a
//! grant's `INSERT` blocks until this transaction ends rather than racing this service's own
//! `delete_role_in` to commit first.
//!
//! **D11 — retirement requires proof the fleet has converged, and is honest about what that
//! proof is.** No in-band mechanism can fully close the fleet-skew window: a binary old enough
//! to still define the retiring id is also old enough to predate any tombstone check this
//! service could add. So `retire` refuses (`TenancyError::FleetNotConverged`) unless every
//! remaining system-owned row's `starter_revision` is at least this binary's own
//! `STARTER_POLICY_REVISION` — the strongest evidence available, not a guarantee. A `NULL`
//! revision refuses too: it proves nothing about which binary last wrote the row, and treating
//! it as `0` would permit exactly the retirement this guard exists to defer.
//!
//! **D12 — a grant at an archived scope cannot be revoked, so the endpoint says so instead of
//! looping.** `RevokeRole` is itself inside `forbid-archived-writes`'s forbid, so an operator
//! told merely "revoke the surviving grants" would hit a refusal loop for any grant whose scope
//! node is archived. Retirement does not special-case this by deleting the grant anyway — that
//! would breach D4's template guarantee for one convenience — so the supported remedy (restore
//! the node, revoke, re-archive) lives in the runbook and in the `Blocked` response's listed
//! grants, not in this service's control flow.

use std::sync::Arc;
use std::time::Duration;

use metrics::counter;
use paigasus_iam_core::authz::model::{PolicyKind, root_prn};
use paigasus_iam_core::authz::roles::{self as authz_roles, STARTER_POLICY_REVISION};
use paigasus_iam_core::{Action, AuditEntry, AuditLog, AuditOutcome, Clock, DomainEvent, EventType, IdGenerator, Outbox, PolicyGenBumper, RetireOutcome, SystemRowRetirer};
use paigasus_kernel::Prn;
use paigasus_observability::names;
use uuid::Uuid;

use crate::application::authorize::Authorize;
use crate::application::bootstrap::truncate_audited_text;
use crate::application::error::TenancyError;

/// At most this many surviving grants are listed in a refusal. Unbounded would load every row
/// into a `Vec` inside a transaction and serialise it whole — a denial of service against the
/// operator's own tooling. The true total is reported separately, so nothing is hidden.
pub const GRANT_LIST_CAP: u64 = 100;

/// Bounds the wait for a contended row. Mirrors `reconcile_system`'s own 5s: this is an
/// operator-triggered request and must fail with a message rather than hang.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// Constructor bag, mirroring `DeadLetterDeps` — keeps `new` from growing a six-argument
/// positional signature.
pub struct SystemRetirementDeps {
    pub retirer: Arc<dyn SystemRowRetirer>,
    pub outbox: Arc<dyn Outbox>,
    pub audit: Arc<dyn AuditLog>,
    pub gen_bumper: Arc<dyn PolicyGenBumper>,
    pub ids: Arc<dyn IdGenerator>,
    pub clock: Arc<dyn Clock>,
    pub authorize: Authorize,
}

#[derive(Clone)]
pub struct SystemRetirementService {
    retirer: Arc<dyn SystemRowRetirer>,
    outbox: Arc<dyn Outbox>,
    audit: Arc<dyn AuditLog>,
    gen_bumper: Arc<dyn PolicyGenBumper>,
    ids: Arc<dyn IdGenerator>,
    clock: Arc<dyn Clock>,
    authorize: Authorize,
}

/// The wire string for a [`PolicyKind`], used only for the audit entry's `destroyed_content`
/// JSON. A small, deliberate duplication of `policies.rs::policy_kind_wire` — that helper is
/// private to an unrelated module, and a five-line pure match isn't worth a visibility change
/// on it (mirrors `application::roles::parse_principal_prn`'s own duplication-over-coupling
/// precedent).
fn policy_kind_wire(kind: PolicyKind) -> &'static str {
    match kind {
        PolicyKind::Static => "static",
        PolicyKind::Template => "template",
    }
}

/// The row was observed present under a `FOR UPDATE` lock held by this very transaction, so a
/// delete affecting no rows is impossible without a data-integrity break. Surfaced as an error
/// rather than degraded to `role_deleted: false`, which would report a retirement that did not
/// happen — mirrors `pg_policies.rs`'s "unique-constraint violation but no row on re-read"
/// posture.
fn require_deleted(deleted: bool, what: &str, id: &str) -> Result<(), TenancyError> {
    if deleted {
        return Ok(());
    }
    tracing::error!(kind = what, policy_id = %id, "a locked {what} row vanished mid-retirement");
    Err(TenancyError::Internal)
}

impl SystemRetirementService {
    #[must_use]
    pub fn new(deps: SystemRetirementDeps) -> Self {
        SystemRetirementService {
            retirer: deps.retirer,
            outbox: deps.outbox,
            audit: deps.audit,
            gen_bumper: deps.gen_bumper,
            ids: deps.ids,
            clock: deps.clock,
            authorize: deps.authorize,
        }
    }

    /// Builds the retirement audit entry, sharing `corr` with the `DomainEvent` this same call
    /// enqueues (the repo-wide one-correlation-id-per-mutation convention, `roles.rs`/
    /// `policies.rs`).
    fn audit_entry(&self, actor: &Prn, corr: Uuid, detail: serde_json::Value) -> AuditEntry {
        AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: self.clock.now(),
            actor_prn: Some(actor.canonical()),
            action: Action::RetireSystemPolicy.as_wire().to_string(),
            resource_prn: Some(root_prn().canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: Vec::new(),
            detail,
            correlation_id: Some(corr),
        }
    }

    /// Retires the orphaned system-owned row at `id`: the `role` row (if any) then the `policy`
    /// row, in the only order `fk_role_template`/`fk_role_grant_role` permit. `ack` acknowledges
    /// the decision-change a STATIC policy's removal causes (D4); it is ignored for a template.
    ///
    /// Order of checks — every one before `begin_retirement` runs without opening a transaction
    /// or taking a lock: (1) Root-only (module docs); (2) `id` must NOT be a still-code-defined
    /// starter id (D7 — retiring a live starter policy would just be re-seeded at the next boot,
    /// but in the window between, it stops governing); (3) the fleet must have converged past
    /// this binary's `STARTER_POLICY_REVISION` (D11). Then, under one locked transaction: (4)
    /// the `policy` row must exist and be system-owned; (5) its `role` row (if any) must also be
    /// system-owned (D6/D7); (6) any surviving grant blocks the retirement, writing nothing
    /// (D4/D5/D12); (7) a STATIC policy without `ack` is refused, writing nothing (D4); only
    /// then are the rows actually deleted, audited, and committed (D9), and the post-commit
    /// `policy_gen` bump awaited (D10).
    pub async fn retire(&self, actor: &Prn, id: &str, ack: bool) -> Result<RetireOutcome, TenancyError> {
        self.authorize.check(actor, Action::RetireSystemPolicy, &root_prn()).await?;

        if authz_roles::is_starter_policy_id(id) {
            return Err(TenancyError::SystemImmutable(id.to_string()));
        }

        if self.retirer.min_starter_revision().await?.is_none_or(|r| r < STARTER_POLICY_REVISION) {
            counter!(names::IAM_SYSTEM_ROWS_RETIRED_TOTAL, "outcome" => "refused").increment(1);
            return Err(TenancyError::FleetNotConverged);
        }

        let tx = self.retirer.begin_retirement(LOCK_TIMEOUT).await?;

        // The policy row is the FK PARENT of the role row, so it is locked first (§3.2 / D6).
        let Some(policy) = self.retirer.lock_policy_in(&*tx, id).await? else {
            return Err(TenancyError::NotFound);
        };
        if !policy.system {
            return Err(TenancyError::NotSystemOwned(id.to_string()));
        }

        let role = self.retirer.lock_role_in(&*tx, id).await?;
        if role.as_ref().is_some_and(|r| !r.system) {
            return Err(TenancyError::NotSystemOwned(id.to_string()));
        }

        if role.is_some() {
            let survivors = self.retirer.surviving_grants_in(&*tx, id, GRANT_LIST_CAP).await?;
            if survivors.total > 0 {
                counter!(names::IAM_SYSTEM_ROWS_RETIRED_TOTAL, "outcome" => "blocked").increment(1);
                return Ok(RetireOutcome::Blocked {
                    role_key: id.to_string(),
                    truncated: survivors.truncated(GRANT_LIST_CAP),
                    grants: survivors.grants,
                    total: survivors.total,
                });
            }
        }

        if policy.kind == PolicyKind::Static && !ack {
            counter!(names::IAM_SYSTEM_ROWS_RETIRED_TOTAL, "outcome" => "refused").increment(1);
            return Ok(RetireOutcome::NeedsAcknowledgement {
                policy_id: id.to_string(),
                kind: policy.kind,
                source: policy.source,
                description: policy.description,
            });
        }

        // role -> policy: the only order `fk_role_template` permits. Both deletes are asserted
        // (`require_deleted`): the rows were just observed present under a held lock, so a
        // `false` here is a data-integrity break, never a legitimate `role_deleted: false`.
        let role_deleted = match &role {
            Some(_) => {
                require_deleted(self.retirer.delete_role_in(&*tx, id).await?, "role", id)?;
                true
            }
            None => false,
        };
        require_deleted(self.retirer.delete_policy_in(&*tx, id).await?, "policy", id)?;

        // One freshly-minted correlation id, shared by the event and the audit entry (D9).
        let corr = self.ids.new_correlation_id();
        let now = self.clock.now();
        let event = DomainEvent {
            id: self.ids.new_event_id(),
            event_type: EventType::PolicyDeleted,
            schema_version: 1,
            aggregate_prn: format!("policy/{id}"),
            actor_prn: Some(actor.canonical()),
            occurred_at: now,
            payload: serde_json::json!({
                "policy_id": id,
                "reason": "system_retirement",
                "role_deleted": role_deleted,
            }),
            correlation_id: Some(corr),
        };

        // Retirement destroys the evidence, so the audit row carries what was destroyed —
        // capped by the SAME helper SMA-477's boot convergence audit uses (D9), not re-derived.
        let (source, truncated) = truncate_audited_text(&policy.source);
        let (description, description_truncated) = truncate_audited_text(&policy.description);
        let detail = serde_json::json!({
            "policy_id": id,
            "role_deleted": role_deleted,
            "source": "system_retirement",
            "destroyed_content": {
                "kind": policy_kind_wire(policy.kind),
                "source": source,
                "description": description,
                "truncated": truncated,
                "description_truncated": description_truncated,
            },
        });
        let entry = self.audit_entry(actor, corr, detail);

        self.outbox.enqueue(&*tx, &event).await?;
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;

        // Post-commit, awaited (D10): guarantees the bump has happened by the time `retire`
        // returns, and can only ever run for a mutation that actually committed.
        self.gen_bumper.bump().await;
        counter!(names::IAM_SYSTEM_ROWS_RETIRED_TOTAL, "outcome" => "retired").increment(1);
        Ok(RetireOutcome::Retired {
            policy_id: id.to_string(),
            kind: policy.kind,
            role_deleted,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::bootstrap::MAX_AUDITED_SOURCE_BYTES;
    use crate::application::fakes::{CountingTransaction, FakeAuditLog, FakeAuthorizer, FakeOutbox, FakePolicyGenBumper, FixedClock, SeqIds};
    use async_trait::async_trait;
    use paigasus_iam_core::{AuthzError, GrantRef, StoredPolicy, StoredRole, SurvivingGrants, Transaction};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn actor() -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(1)).unwrap()
    }

    fn stored_policy(system: bool, kind: PolicyKind) -> StoredPolicy {
        StoredPolicy {
            policy_id: "legacy_auditor".to_string(),
            kind,
            source: "permit(principal, action, resource);".to_string(),
            description: "a legacy auditor role".to_string(),
            system,
        }
    }

    fn stored_policy_with_source(source: &str) -> StoredPolicy {
        StoredPolicy {
            policy_id: "legacy_auditor".to_string(),
            kind: PolicyKind::Template,
            source: source.to_string(),
            description: "a legacy auditor role".to_string(),
            system: true,
        }
    }

    fn grant_ref(id: &str) -> GrantRef {
        GrantRef {
            id: id.to_string(),
            principal_prn: Prn::build("iam", "", None, "principal", Uuid::from_u128(1)).unwrap().canonical(),
            scope_prn: root_prn().canonical(),
        }
    }

    /// Mirrors `bootstrap.rs`'s `ScriptedPolicies`: `Mutex`-held scripted returns plus recorded
    /// calls. Every read method ignores its `id`/`key` argument and returns the scripted value
    /// regardless — this fake proves the SERVICE's control flow, not the store's per-row
    /// dispatch (real dispatch is the Postgres integration tests' job). `calls`/`commits` are
    /// `Arc`-backed so every `clone()` of a `ScriptedRetirer` shares one recorded-call log and
    /// one commit counter — the same reason every other fake in `fakes.rs` does this.
    #[derive(Clone)]
    struct ScriptedRetirer {
        policy: Option<StoredPolicy>,
        role: Option<StoredRole>,
        survivors: SurvivingGrants,
        min_revision: Option<u32>,
        fail_delete_policy: bool,
        role_delete_returns_false: bool,
        calls: Arc<Mutex<Vec<String>>>,
        commits: Arc<AtomicUsize>,
    }

    impl Default for ScriptedRetirer {
        fn default() -> Self {
            ScriptedRetirer {
                policy: None,
                role: None,
                survivors: SurvivingGrants { grants: Vec::new(), total: 0 },
                min_revision: None,
                fail_delete_policy: false,
                role_delete_returns_false: false,
                calls: Arc::new(Mutex::new(Vec::new())),
                commits: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl ScriptedRetirer {
        /// Identity. `ScriptedRetirer`'s interior state already lives behind `Arc<Mutex<_>>`/
        /// `Arc<AtomicUsize>` fields, so `clone()` alone shares state across every handle — this
        /// exists purely so a call site can read as "this fake is shared across the service AND
        /// the post-call assertions" without an actual `Arc<Self>` wrapper.
        fn shared(self) -> Self {
            self
        }

        fn record(&self, name: &str) {
            self.calls.lock().unwrap().push(name.to_string());
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn commits(&self) -> usize {
            self.commits.load(Ordering::SeqCst)
        }
    }

    fn backend_err() -> AuthzError {
        AuthzError::Backend(Box::new(std::io::Error::other("simulated mid-retirement store failure")))
    }

    #[async_trait]
    impl SystemRowRetirer for ScriptedRetirer {
        async fn begin_retirement(&self, _lock_timeout: Duration) -> Result<Box<dyn Transaction>, AuthzError> {
            self.record("begin_retirement");
            Ok(Box::new(CountingTransaction(self.commits.clone())))
        }

        async fn lock_policy_in(&self, _tx: &dyn Transaction, _policy_id: &str) -> Result<Option<StoredPolicy>, AuthzError> {
            self.record("lock_policy_in");
            Ok(self.policy.clone())
        }

        async fn lock_role_in(&self, _tx: &dyn Transaction, _key: &str) -> Result<Option<StoredRole>, AuthzError> {
            self.record("lock_role_in");
            Ok(self.role.clone())
        }

        async fn surviving_grants_in(&self, _tx: &dyn Transaction, _role_key: &str, _cap: u64) -> Result<SurvivingGrants, AuthzError> {
            self.record("surviving_grants_in");
            Ok(self.survivors.clone())
        }

        async fn min_starter_revision(&self) -> Result<Option<u32>, AuthzError> {
            self.record("min_starter_revision");
            Ok(self.min_revision)
        }

        async fn delete_role_in(&self, _tx: &dyn Transaction, _key: &str) -> Result<bool, AuthzError> {
            self.record("delete_role_in");
            Ok(!self.role_delete_returns_false)
        }

        async fn delete_policy_in(&self, _tx: &dyn Transaction, _policy_id: &str) -> Result<bool, AuthzError> {
            self.record("delete_policy_in");
            if self.fail_delete_policy {
                return Err(backend_err());
            }
            Ok(true)
        }
    }

    /// The healthy-fleet baseline: a system-owned template, no role row, converged revision.
    fn converged() -> ScriptedRetirer {
        ScriptedRetirer {
            policy: Some(stored_policy(true, PolicyKind::Template)),
            min_revision: Some(STARTER_POLICY_REVISION),
            ..Default::default()
        }
    }

    /// [`converged`] plus a system-owned role row at the same key.
    fn converged_with_role() -> ScriptedRetirer {
        ScriptedRetirer {
            role: Some(StoredRole {
                key: "legacy_auditor".to_string(),
                system: true,
            }),
            ..converged()
        }
    }

    /// [`converged_with_role`] with no surviving grants — the template happy path's fixture.
    fn converged_with_role_and_no_grants() -> ScriptedRetirer {
        ScriptedRetirer {
            survivors: SurvivingGrants { grants: Vec::new(), total: 0 },
            ..converged_with_role()
        }
    }

    /// Builds a service over `retirer` with `RetireSystemPolicy` allowed at `Root` and
    /// throwaway outbox/audit/bumper fakes (fine for every test that doesn't itself assert on
    /// what got emitted — see `svc_with_sinks` for those).
    fn svc(retirer: impl SystemRowRetirer + 'static) -> SystemRetirementService {
        let authz = FakeAuthorizer::default();
        authz.allow(Action::RetireSystemPolicy, &root_prn());
        SystemRetirementService::new(SystemRetirementDeps {
            retirer: Arc::new(retirer),
            outbox: Arc::new(FakeOutbox::default()),
            audit: Arc::new(FakeAuditLog::default()),
            gen_bumper: Arc::new(FakePolicyGenBumper::default()),
            ids: Arc::new(SeqIds::default()),
            clock: Arc::new(FixedClock::default()),
            authorize: Authorize::new(Arc::new(authz)),
        })
    }

    /// Like `svc`, but the authorizer denies everything — the always-deny-by-default fake,
    /// never told to allow `RetireSystemPolicy`.
    fn svc_denying_authz(retirer: impl SystemRowRetirer + 'static) -> SystemRetirementService {
        SystemRetirementService::new(SystemRetirementDeps {
            retirer: Arc::new(retirer),
            outbox: Arc::new(FakeOutbox::default()),
            audit: Arc::new(FakeAuditLog::default()),
            gen_bumper: Arc::new(FakePolicyGenBumper::default()),
            ids: Arc::new(SeqIds::default()),
            clock: Arc::new(FixedClock::default()),
            authorize: Authorize::new(Arc::new(FakeAuthorizer::default())),
        })
    }

    /// Like `svc`, but returns the outbox/audit/gen-bumper fakes alongside the service so a
    /// test can assert on exactly what `retire` emitted through them (mirrors `roles.rs`'s
    /// `new_service_with_fakes`).
    fn svc_with_sinks(retirer: impl SystemRowRetirer + 'static) -> (SystemRetirementService, FakeOutbox, FakeAuditLog, FakePolicyGenBumper) {
        let authz = FakeAuthorizer::default();
        authz.allow(Action::RetireSystemPolicy, &root_prn());
        let outbox = FakeOutbox::default();
        let audit = FakeAuditLog::default();
        let bumper = FakePolicyGenBumper::default();
        let svc = SystemRetirementService::new(SystemRetirementDeps {
            retirer: Arc::new(retirer),
            outbox: Arc::new(outbox.clone()),
            audit: Arc::new(audit.clone()),
            gen_bumper: Arc::new(bumper.clone()),
            ids: Arc::new(SeqIds::default()),
            clock: Arc::new(FixedClock::default()),
            authorize: Authorize::new(Arc::new(authz)),
        });
        (svc, outbox, audit, bumper)
    }

    /// Root-only, and the check comes first: an unauthorized caller must not learn whether the
    /// id exists, and must not take a row lock.
    #[tokio::test]
    async fn an_unauthorized_actor_is_forbidden_and_touches_no_retirer_port() {
        let retirer = ScriptedRetirer::default();
        let svc = svc_denying_authz(retirer.clone());
        assert_eq!(svc.retire(&actor(), "legacy_auditor", false).await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(retirer.calls(), Vec::<String>::new(), "not even begin_retirement may run");
        assert_eq!(retirer.commits(), 0, "an unauthorized call must never commit a transaction");
    }

    /// D7's load-bearing guard. Retiring a LIVE starter policy would be re-seeded next boot,
    /// but in the window between, that policy stops governing: forbid-archived-writes gone
    /// means archived resources become writable. Asserted for a role key AND the static id.
    #[tokio::test]
    async fn a_still_code_defined_id_is_refused_before_any_read() {
        for id in ["platform_admin", "forbid-archived-writes"] {
            let retirer = ScriptedRetirer::default();
            let svc = svc(retirer.clone());
            assert_eq!(svc.retire(&actor(), id, true).await.unwrap_err(), TenancyError::SystemImmutable(id.to_string()));
            assert!(retirer.calls().is_empty(), "{id} must be refused before a transaction opens");
            assert_eq!(retirer.commits(), 0, "a still-code-defined id must never commit a transaction");
        }
    }

    /// D11. Both a low revision and a NULL must refuse — a NULL proves nothing about which
    /// binary last wrote the row, and reading it as 0 would permit exactly the retirement this
    /// guard exists to defer.
    #[tokio::test]
    async fn an_unconverged_fleet_is_refused() {
        for min in [Some(STARTER_POLICY_REVISION - 1), None] {
            let retirer = ScriptedRetirer {
                min_revision: min,
                ..Default::default()
            }
            .shared();
            let svc = svc(retirer.clone());
            assert_eq!(svc.retire(&actor(), "legacy_auditor", true).await.unwrap_err(), TenancyError::FleetNotConverged);
            assert!(!retirer.calls().contains(&"begin_retirement".to_string()));
            assert_eq!(retirer.commits(), 0, "an unconverged fleet must never commit a transaction");
        }
    }

    #[tokio::test]
    async fn an_absent_row_is_not_found_and_a_non_system_row_is_refused() {
        {
            let retirer = ScriptedRetirer { policy: None, ..converged() }.shared();
            let svc = svc(retirer.clone());
            assert_eq!(svc.retire(&actor(), "gone", true).await.unwrap_err(), TenancyError::NotFound);
            assert_eq!(retirer.commits(), 0, "a not-found row must never commit a transaction");
        }

        // A non-system POLICY row: DeletePolicy already serves those.
        {
            let retirer = ScriptedRetirer {
                policy: Some(stored_policy(false, PolicyKind::Template)),
                ..converged()
            }
            .shared();
            let svc = svc(retirer.clone());
            assert_eq!(svc.retire(&actor(), "op_policy", true).await.unwrap_err(), TenancyError::NotSystemOwned("op_policy".to_string()));
            assert_eq!(retirer.commits(), 0);
        }

        // A non-system ROLE row at a system policy's id — the half the first draft omitted.
        {
            let retirer = ScriptedRetirer {
                policy: Some(stored_policy(true, PolicyKind::Template)),
                role: Some(StoredRole {
                    key: "legacy_auditor".to_string(),
                    system: false,
                }),
                ..converged()
            }
            .shared();
            let svc = svc(retirer.clone());
            assert_eq!(
                svc.retire(&actor(), "legacy_auditor", true).await.unwrap_err(),
                TenancyError::NotSystemOwned("legacy_auditor".to_string())
            );
            assert_eq!(retirer.commits(), 0, "a non-system row must never commit a transaction");
        }
    }

    /// D4/D5: survivors block, and blocking writes NOTHING. The `total` is reported from the
    /// store, not from the returned page's length.
    #[tokio::test]
    async fn surviving_grants_block_the_retirement_and_write_nothing() {
        let retirer = ScriptedRetirer {
            survivors: SurvivingGrants {
                grants: vec![grant_ref("a"), grant_ref("b")],
                total: 7,
            },
            ..converged_with_role()
        }
        .shared();
        let (svc, outbox, audit, bumper) = svc_with_sinks(retirer.clone());

        let outcome = svc.retire(&actor(), "legacy_auditor", true).await.unwrap();
        match outcome {
            RetireOutcome::Blocked { role_key, grants, total, truncated } => {
                assert_eq!(role_key, "legacy_auditor");
                assert_eq!(grants.len(), 2);
                assert_eq!(total, 7, "the true total, not the page length");
                assert!(truncated || total <= GRANT_LIST_CAP);
            }
            other => panic!("expected Blocked, got {other:?}"),
        }
        assert!(!retirer.calls().iter().any(|c| c.starts_with("delete_")), "a blocked retirement deletes nothing");
        assert!(outbox.0.lock().unwrap().is_empty(), "and enqueues nothing");
        assert!(audit.0.lock().unwrap().is_empty(), "and audits nothing");
        assert_eq!(bumper.calls(), 0, "and bumps nothing");
        assert_eq!(retirer.commits(), 0, "a blocked retirement must never commit its transaction");
    }

    /// D4's static half — the finding that invalidated the first draft's central claim.
    /// A static policy compiles unconditionally, so removing it changes decisions fleet-wide.
    #[tokio::test]
    async fn a_static_policy_needs_acknowledgement_and_the_refusal_previews_what_would_be_lost() {
        let retirer = ScriptedRetirer {
            policy: Some(StoredPolicy {
                policy_id: "legacy_forbid".to_string(),
                kind: PolicyKind::Static,
                source: "forbid(principal, action, resource);".to_string(),
                description: "a retired guard".to_string(),
                system: true,
            }),
            role: None,
            ..converged()
        }
        .shared();
        let (svc, outbox, audit, bumper) = svc_with_sinks(retirer.clone());

        match svc.retire(&actor(), "legacy_forbid", false).await.unwrap() {
            RetireOutcome::NeedsAcknowledgement { source, description, kind, .. } => {
                assert_eq!(kind, PolicyKind::Static);
                assert_eq!(source, "forbid(principal, action, resource);", "the refusal IS the preview");
                assert_eq!(description, "a retired guard");
            }
            other => panic!("expected NeedsAcknowledgement, got {other:?}"),
        }
        assert!(!retirer.calls().iter().any(|c| c.starts_with("delete_")));
        assert!(outbox.0.lock().unwrap().is_empty() && audit.0.lock().unwrap().is_empty() && bumper.calls() == 0);
        assert_eq!(retirer.commits(), 0, "an unacknowledged static retirement must never commit");

        // With the flag it proceeds, and reports role_deleted: false (no role row exists).
        let outcome = svc.retire(&actor(), "legacy_forbid", true).await.unwrap();
        assert_eq!(
            outcome,
            RetireOutcome::Retired {
                policy_id: "legacy_forbid".to_string(),
                kind: PolicyKind::Static,
                role_deleted: false
            }
        );
        assert_eq!(retirer.commits(), 1, "the acknowledged retirement must commit exactly once");
    }

    /// The flag is a no-op on a template: an operator must not have to know which kind they
    /// are dealing with before calling.
    #[tokio::test]
    async fn the_acknowledgement_flag_is_ignored_for_a_template() {
        let svc = svc(converged_with_role_and_no_grants().shared());
        assert!(svc.retire(&actor(), "legacy_auditor", false).await.unwrap().is_retired());
    }

    /// The happy path's full contract: role BEFORE policy, one event carrying the
    /// discriminator, one audit entry, ONE shared correlation_id, one awaited bump.
    #[tokio::test]
    async fn the_template_happy_path_deletes_role_then_policy_and_emits_one_of_each() {
        let retirer = converged_with_role_and_no_grants().shared();
        let (svc, outbox, audit, bumper) = svc_with_sinks(retirer.clone());

        let outcome = svc.retire(&actor(), "legacy_auditor", false).await.unwrap();
        assert_eq!(
            outcome,
            RetireOutcome::Retired {
                policy_id: "legacy_auditor".to_string(),
                kind: PolicyKind::Template,
                role_deleted: true
            }
        );

        let calls = retirer.calls();
        let role_at = calls.iter().position(|c| c == "delete_role_in").expect("role must be deleted");
        let policy_at = calls.iter().position(|c| c == "delete_policy_in").expect("policy must be deleted");
        assert!(role_at < policy_at, "fk_role_template forces role before policy");

        let events = outbox.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::PolicyDeleted);
        assert_eq!(events[0].payload["reason"], serde_json::json!("system_retirement"));
        assert_eq!(events[0].payload["role_deleted"], serde_json::json!(true));

        let entries = audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, Action::RetireSystemPolicy.as_wire());
        assert_eq!(entries[0].resource_prn.as_deref(), Some(root_prn().canonical().as_str()));
        assert_eq!(entries[0].correlation_id, events[0].correlation_id, "one act, one correlation id");

        assert_eq!(bumper.calls(), 1, "awaited exactly once, post-commit");
        assert_eq!(retirer.commits(), 1, "the happy path must commit exactly once");
    }

    /// D9: retirement destroys the evidence, so the audit row carries it — capped by the same
    /// helper boot's convergence audit uses.
    #[tokio::test]
    async fn the_audit_entry_records_the_destroyed_content_and_caps_an_oversized_source() {
        let huge = "x".repeat(MAX_AUDITED_SOURCE_BYTES + 500);
        let retirer = ScriptedRetirer {
            policy: Some(stored_policy_with_source(&huge)),
            ..converged_with_role_and_no_grants()
        }
        .shared();
        let (svc, _outbox, audit, _bumper) = svc_with_sinks(retirer);
        let _outcome = svc.retire(&actor(), "legacy_auditor", true).await.unwrap();

        let entries = audit.0.lock().unwrap();
        let destroyed = &entries[0].detail["destroyed_content"];
        assert_eq!(destroyed["kind"], serde_json::json!("template"));
        assert_eq!(destroyed["source"].as_str().unwrap().len(), MAX_AUDITED_SOURCE_BYTES);
        assert_eq!(destroyed["truncated"], serde_json::json!(true));
    }

    /// A failure between the deletes and the commit must leave nothing behind.
    #[tokio::test]
    async fn a_failure_before_commit_emits_nothing() {
        let retirer = ScriptedRetirer {
            fail_delete_policy: true,
            ..converged_with_role_and_no_grants()
        }
        .shared();
        let (svc, outbox, audit, bumper) = svc_with_sinks(retirer.clone());
        svc.retire(&actor(), "legacy_auditor", true).await.expect_err("a store failure must propagate");
        assert!(outbox.0.lock().unwrap().is_empty() && audit.0.lock().unwrap().is_empty() && bumper.calls() == 0);
        assert_eq!(retirer.commits(), 0, "a failure before commit must never commit");
    }

    /// The rows were observed present under a held lock, so a `false` here is a data-integrity
    /// break — never a silent `role_deleted: false`, which would misreport what happened.
    #[tokio::test]
    async fn a_delete_that_affected_no_rows_under_a_held_lock_is_an_error() {
        let retirer = ScriptedRetirer {
            role_delete_returns_false: true,
            ..converged_with_role_and_no_grants()
        }
        .shared();
        let svc = svc(retirer.clone());
        assert_eq!(svc.retire(&actor(), "legacy_auditor", true).await.unwrap_err(), TenancyError::Internal);
        assert_eq!(retirer.commits(), 0, "a data-integrity break must never commit");
    }
}
