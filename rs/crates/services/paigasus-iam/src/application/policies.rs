// SPDX-License-Identifier: Apache-2.0

//! `PolicyService`: authored Cedar policy/template CRUD use cases (SMA-444 Task 17,
//! ADR-0013). Every operation is platform-scoped — `PutPolicy`/`DeletePolicy`/`ListPolicies`
//! are Root-only actions (design §3.2: only `platform_admin` holds them), so every method
//! authorizes against `root_prn()` before ever touching the store.
//!
//! **SMA-446 Slice B Task B5 — the UoW reference pattern, applied to `put`/`delete` (copied
//! from `RoleService::grant`/`revoke`, Task B4 — see `application::roles`'s module docs for
//! the pattern itself):** once `put`/`delete`'s authorize check passes, the mutation, its
//! [`DomainEvent`], and its [`AuditEntry`] all share ONE freshly-minted `correlation_id` and
//! commit together on ONE [`UnitOfWork`]-scoped transaction (`policies.put_in`/`delete_in`,
//! `outbox.enqueue`, `audit.record`, then `tx.commit()`). Only once that commit succeeds does
//! the service run its one post-commit side effect: an AWAITED `gen_bumper.bump()`.
//!
//! **The crux — preserving SMA-444's conflict-absorption through the UoW:** `put_in`'s
//! same-content unique-violation race absorbs into [`PutOutcome::AbsorbedIdempotent`] via a
//! SAVEPOINT rather than the pre-Slice-B "abort the whole txn, re-read on a fresh connection"
//! posture (see `pg_policies.rs`'s module docs for the store-level mechanics). `put` treats
//! `AbsorbedIdempotent` exactly like `RoleService::revoke`'s `revoke_in == false`: a true
//! no-op emits NOTHING — no outbox event, no audit entry, no post-commit bump — because the
//! winning writer already did all three for this row. A different-content race still
//! surfaces as `AuthzError::Conflict` -> `TenancyError::PolicyConflict`, unchanged. `delete`
//! mirrors `RoleService::revoke`'s idempotent-DELETE posture directly: `delete_in` returning
//! `false` (the policy id never existed) is a no-op that emits nothing either.
//!
//! The store itself (`PolicyStore::put_in`/`delete_in`) separately rejects mutation of an
//! already-persisted `system = true` row (`AuthzError::SystemImmutable` ->
//! `TenancyError::SystemImmutable`) — this service does not duplicate that check.

use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;
use paigasus_iam_core::authz::model::{PolicyKind, root_prn};
use paigasus_iam_core::{Action, AuditEntry, AuditLog, AuditOutcome, Clock, DomainEvent, EventType, IdGenerator, Outbox, PolicyDocument, PolicyGenBumper, PolicyStore, PutOutcome, UnitOfWork};
use paigasus_kernel::Prn;
use std::sync::Arc;

/// The wire string for a [`PolicyKind`], used only for a `DomainEvent`'s JSON payload — a
/// small, deliberate duplication of `pg_policies.rs::kind_to_str` (an adapter-layer private
/// helper this application-layer module cannot reach, and a five-line pure match isn't worth
/// a visibility change on an unrelated file, mirroring `application::roles::parse_principal_prn`'s
/// own duplication-over-coupling precedent).
fn policy_kind_wire(kind: PolicyKind) -> &'static str {
    match kind {
        PolicyKind::Static => "static",
        PolicyKind::Template => "template",
    }
}

/// The `DomainEvent`/audit resource identity for a policy document: `policy_id` is an
/// arbitrary caller-chosen string (not a UUID), so it cannot round-trip through
/// `paigasus_kernel::Prn::build` (which requires a `Uuid` resource id) — a plain
/// `"policy/{id}"` string is used instead, exactly like `RoleGrant::linked_policy_id`'s own
/// `format!("grant:{id}")` non-`Prn` identifier.
fn policy_aggregate_prn(policy_id: &str) -> String {
    format!("policy/{policy_id}")
}

/// Policy/template CRUD use cases. `policies` is `Arc<dyn PolicyStore>` — the same shared
/// handle `AppState` composes into `PolicySnapshot`, so a later task's wiring clones one
/// `Arc` rather than standing up a second store instance (mirrors `RoleService`'s posture).
/// `uow`/`outbox`/`audit`/`gen_bumper` are SMA-446 Slice B Task B5's Unit-of-Work reference
/// pattern (module docs): `put`/`delete` drive the mutation + its outbox event + its audit
/// entry through `uow` atomically, then run `gen_bumper`'s awaited, best-effort post-commit
/// bump. `ids`/`clock` stay generic-DI, mirroring `RoleService`.
#[derive(Clone)]
pub struct PolicyService<I, C> {
    policies: Arc<dyn PolicyStore>,
    authorize: Authorize,
    uow: Arc<dyn UnitOfWork>,
    outbox: Arc<dyn Outbox>,
    audit: Arc<dyn AuditLog>,
    gen_bumper: Arc<dyn PolicyGenBumper>,
    ids: I,
    clock: C,
}

/// Named-field constructor params for [`PolicyService::new`] (SMA-446 Slice B Task B5) —
/// copies `application::roles::RoleServiceDeps`'s DI-params idiom verbatim: one field per
/// dependency, built with struct syntax at the call site so each argument is self-labeling.
pub struct PolicyServiceDeps<I, C> {
    pub policies: Arc<dyn PolicyStore>,
    pub authorize: Authorize,
    pub uow: Arc<dyn UnitOfWork>,
    pub outbox: Arc<dyn Outbox>,
    pub audit: Arc<dyn AuditLog>,
    pub gen_bumper: Arc<dyn PolicyGenBumper>,
    pub ids: I,
    pub clock: C,
}

impl<I, C> PolicyService<I, C>
where
    I: IdGenerator,
    C: Clock,
{
    pub fn new(deps: PolicyServiceDeps<I, C>) -> Self {
        Self {
            policies: deps.policies,
            authorize: deps.authorize,
            uow: deps.uow,
            outbox: deps.outbox,
            audit: deps.audit,
            gen_bumper: deps.gen_bumper,
            ids: deps.ids,
            clock: deps.clock,
        }
    }

    /// Authorizes `actor` for `Action::PutPolicy` at `root_prn()`, then persists `doc`
    /// through the UoW reference pattern (module docs). The store validates the Cedar source
    /// and rejects editing an existing system row (`AuthzError::SystemImmutable`, surfaced
    /// before the txn ever opens a savepoint). Only when the store's outcome is
    /// `Inserted`/`Updated` does this enqueue the `DomainEvent` + `AuditEntry` and, after a
    /// successful commit, run the awaited `gen_bumper.bump()` — `PutOutcome::
    /// AbsorbedIdempotent` (the crux, module docs) skips all three: it is a true no-op from
    /// THIS caller's perspective, even though the row now exists with the requested content.
    pub async fn put(&self, actor: &Prn, doc: PolicyDocument) -> Result<(), TenancyError> {
        self.authorize.check(actor, Action::PutPolicy, &root_prn()).await?;

        let now = self.clock.now();
        let corr = self.ids.new_correlation_id();
        let event = DomainEvent {
            id: self.ids.new_event_id(),
            event_type: EventType::PolicyPut,
            schema_version: 1,
            aggregate_prn: policy_aggregate_prn(&doc.policy_id),
            actor_prn: Some(actor.canonical()),
            occurred_at: now,
            payload: serde_json::json!({"policy_id": doc.policy_id, "kind": policy_kind_wire(doc.kind)}),
            correlation_id: Some(corr),
        };
        let entry = AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: now,
            actor_prn: Some(actor.canonical()),
            action: "PutPolicy".to_string(),
            resource_prn: Some(root_prn().canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: vec![],
            detail: serde_json::json!({"policy_id": doc.policy_id}),
            correlation_id: Some(corr),
        };

        let tx = self.uow.begin().await?;
        let outcome = self.policies.put_in(&*tx, &doc).await?;
        if !matches!(outcome, PutOutcome::AbsorbedIdempotent) {
            self.outbox.enqueue(&*tx, &event).await?;
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;

        // Post-commit, awaited (module docs): guarantees the bump has happened by the time
        // `put` returns (AC1), and can only ever run for a mutation that actually committed
        // AND actually changed something (never for an absorbed idempotent no-op).
        if !matches!(outcome, PutOutcome::AbsorbedIdempotent) {
            self.gen_bumper.bump().await;
        }
        Ok(())
    }

    /// Authorizes `actor` for `Action::DeletePolicy` at `root_prn()`, then deletes
    /// `policy_id` through the UoW reference pattern. The store rejects deleting an existing
    /// system row; `delete_in` returning `false` (the id never existed, or was already
    /// deleted — an idempotent race) is treated exactly like `RoleService::revoke`'s vanished-
    /// grant case: nothing is enqueued or recorded, and the post-commit bump never runs,
    /// since nothing was actually deleted by THIS call.
    pub async fn delete(&self, actor: &Prn, policy_id: &str) -> Result<(), TenancyError> {
        self.authorize.check(actor, Action::DeletePolicy, &root_prn()).await?;

        let now = self.clock.now();
        let corr = self.ids.new_correlation_id();
        let event = DomainEvent {
            id: self.ids.new_event_id(),
            event_type: EventType::PolicyDeleted,
            schema_version: 1,
            aggregate_prn: policy_aggregate_prn(policy_id),
            actor_prn: Some(actor.canonical()),
            occurred_at: now,
            payload: serde_json::json!({"policy_id": policy_id}),
            correlation_id: Some(corr),
        };
        let entry = AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: now,
            actor_prn: Some(actor.canonical()),
            action: "DeletePolicy".to_string(),
            resource_prn: Some(root_prn().canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: vec![],
            detail: serde_json::json!({"policy_id": policy_id}),
            correlation_id: Some(corr),
        };

        let tx = self.uow.begin().await?;
        let existed = self.policies.delete_in(&*tx, policy_id).await?;
        if existed {
            self.outbox.enqueue(&*tx, &event).await?;
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;

        if existed {
            self.gen_bumper.bump().await;
        }
        Ok(())
    }

    /// Authorizes `actor` for `Action::ListPolicies` at `root_prn()`, then lists every
    /// persisted policy/template, paginated in-service (the store has no `limit`/`offset` of
    /// its own — `list_all` always returns the full, consistent snapshot).
    pub async fn list(&self, actor: &Prn, limit: u64, offset: u64) -> Result<Vec<PolicyDocument>, TenancyError> {
        self.authorize.check(actor, Action::ListPolicies, &root_prn()).await?;
        let mut docs = self.policies.list_all().await?;
        docs.sort_by(|a, b| a.policy_id.cmp(&b.policy_id));
        Ok(docs.into_iter().skip(offset as usize).take(limit as usize).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAuditLog, FakeAuthorizer, FakeOutbox, FakePolicyGenBumper, FakeUnitOfWork, FixedClock, InMemoryPolicies, SeqIds};
    use async_trait::async_trait;
    use chrono::Utc;
    use paigasus_iam_core::authz::model::PolicyKind;
    use paigasus_iam_core::{AuthzError, Transaction};
    use uuid::Uuid;

    fn actor_prn(n: u128) -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap()
    }

    fn doc(policy_id: &str) -> PolicyDocument {
        let now = Utc::now();
        PolicyDocument {
            policy_id: policy_id.to_string(),
            kind: PolicyKind::Static,
            source: r#"permit(principal, action == Pgs::Iam::Action::"GetOrganization", resource);"#.to_string(),
            description: "test policy".to_string(),
            system: false,
            created_at: now,
            updated_at: now,
        }
    }

    /// Builds a `PolicyService` over a fresh `InMemoryPolicies` store and fresh, unshared
    /// SMA-446 Slice B fakes — fine for every scenario here that doesn't itself assert on
    /// what got emitted (see `new_service_with_fakes` for those).
    fn new_service(fake: FakeAuthorizer) -> PolicyService<SeqIds, FixedClock> {
        new_service_with_fakes(fake, Arc::new(InMemoryPolicies::default())).svc
    }

    /// Bundles a `PolicyService` together with the SMA-446 Slice B fakes it was built over,
    /// so a test can assert on exactly what `put`/`delete` emitted through them — mirrors
    /// `application::roles::tests::ServiceWithFakes`.
    struct ServiceWithFakes {
        svc: PolicyService<SeqIds, FixedClock>,
        outbox: FakeOutbox,
        audit: FakeAuditLog,
        bumper: FakePolicyGenBumper,
    }

    /// Like `new_service`, but over a caller-supplied `policies` store (so a test can inject
    /// one that always absorbs or always fails) and returning the outbox/audit/gen-bumper
    /// fakes alongside the service for direct assertion.
    fn new_service_with_fakes(fake: FakeAuthorizer, policies: Arc<dyn PolicyStore>) -> ServiceWithFakes {
        let outbox = FakeOutbox::default();
        let audit = FakeAuditLog::default();
        let bumper = FakePolicyGenBumper::default();
        let svc = PolicyService::new(PolicyServiceDeps {
            policies,
            authorize: Authorize::new(Arc::new(fake)),
            uow: Arc::new(FakeUnitOfWork::default()),
            outbox: Arc::new(outbox.clone()),
            audit: Arc::new(audit.clone()),
            gen_bumper: Arc::new(bumper.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });
        ServiceWithFakes { svc, outbox, audit, bumper }
    }

    /// A `PolicyStore` whose `put_in` always reports [`PutOutcome::AbsorbedIdempotent`] —
    /// simulates the same-content savepoint-absorption race (`pg_policies.rs` module docs):
    /// `PolicyService::put` must treat it as a true no-op from this caller's perspective.
    #[derive(Default)]
    struct AbsorbingPolicyStore;

    #[async_trait]
    impl PolicyStore for AbsorbingPolicyStore {
        async fn list_all(&self) -> Result<Vec<PolicyDocument>, AuthzError> {
            Ok(Vec::new())
        }

        async fn put(&self, _doc: &PolicyDocument) -> Result<(), AuthzError> {
            unimplemented!("this fake only exercises put_in")
        }

        async fn delete(&self, _policy_id: &str) -> Result<(), AuthzError> {
            unimplemented!("this fake only exercises put_in")
        }

        async fn put_in(&self, _tx: &dyn Transaction, _doc: &PolicyDocument) -> Result<PutOutcome, AuthzError> {
            Ok(PutOutcome::AbsorbedIdempotent)
        }

        async fn delete_in(&self, _tx: &dyn Transaction, _policy_id: &str) -> Result<bool, AuthzError> {
            unimplemented!("this fake only exercises put_in")
        }

        async fn policy_gen(&self) -> Result<u64, AuthzError> {
            Ok(0)
        }

        async fn bump_policy_gen(&self) -> Result<u64, AuthzError> {
            Ok(0)
        }
    }

    /// A `PolicyStore` whose `put_in` always fails — simulates a store error mid-txn (guard
    /// D2's `RoleGrantStore` analogue): `PolicyService::put` must roll back before ever
    /// touching the outbox/audit log, and its post-commit bump must never run.
    #[derive(Default)]
    struct FailingPutStore;

    #[async_trait]
    impl PolicyStore for FailingPutStore {
        async fn list_all(&self) -> Result<Vec<PolicyDocument>, AuthzError> {
            Ok(Vec::new())
        }

        async fn put(&self, _doc: &PolicyDocument) -> Result<(), AuthzError> {
            unimplemented!("this fake only exercises put_in")
        }

        async fn delete(&self, _policy_id: &str) -> Result<(), AuthzError> {
            unimplemented!("this fake only exercises put_in")
        }

        async fn put_in(&self, _tx: &dyn Transaction, _doc: &PolicyDocument) -> Result<PutOutcome, AuthzError> {
            Err(AuthzError::Backend(Box::new(std::io::Error::other("simulated mid-txn store failure"))))
        }

        async fn delete_in(&self, _tx: &dyn Transaction, _policy_id: &str) -> Result<bool, AuthzError> {
            unimplemented!("this fake only exercises put_in")
        }

        async fn policy_gen(&self) -> Result<u64, AuthzError> {
            Ok(0)
        }

        async fn bump_policy_gen(&self) -> Result<u64, AuthzError> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn put_denies_an_unauthorized_actor() {
        let svc = new_service(FakeAuthorizer::default());
        let err = svc.put(&actor_prn(1), doc("p1")).await.unwrap_err();
        assert_eq!(err, TenancyError::Forbidden);
    }

    #[tokio::test]
    async fn put_succeeds_for_an_authorized_actor_and_list_returns_it() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::PutPolicy, &root_prn());
        fake.allow(Action::ListPolicies, &root_prn());
        let svc = new_service(fake);
        let actor = actor_prn(1);

        svc.put(&actor, doc("p1")).await.unwrap();
        let listed = svc.list(&actor, 50, 0).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].policy_id, "p1");
    }

    #[tokio::test]
    async fn delete_denies_an_unauthorized_actor() {
        let svc = new_service(FakeAuthorizer::default());
        assert_eq!(svc.delete(&actor_prn(1), "p1").await.unwrap_err(), TenancyError::Forbidden);
    }

    #[tokio::test]
    async fn list_denies_an_unauthorized_actor() {
        let svc = new_service(FakeAuthorizer::default());
        assert_eq!(svc.list(&actor_prn(1), 50, 0).await.unwrap_err(), TenancyError::Forbidden);
    }

    /// SMA-446 Slice B — the UoW reference pattern's core contract for `put`: enqueues
    /// exactly one `DomainEvent` and records exactly one `AuditEntry`, the two sharing ONE
    /// correlation id, and its post-commit `PolicyGenBumper::bump()` has already run — is
    /// AWAITED, not fire-and-forget — by the time `put` returns (AC1).
    #[tokio::test]
    async fn put_emits_one_event_and_one_audit_entry_sharing_a_correlation_id_and_awaits_the_bump() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::PutPolicy, &root_prn());
        let ServiceWithFakes { svc, outbox, audit, bumper } = new_service_with_fakes(fake, Arc::new(InMemoryPolicies::default()));
        let actor = actor_prn(1);

        svc.put(&actor, doc("p1")).await.unwrap();

        let events = outbox.0.lock().unwrap();
        assert_eq!(events.len(), 1, "put must enqueue exactly one domain event");
        assert_eq!(events[0].event_type, EventType::PolicyPut);
        assert_eq!(events[0].actor_prn, Some(actor.canonical()));
        assert_eq!(events[0].payload["policy_id"], serde_json::json!("p1"));

        let entries = audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1, "put must record exactly one audit entry");
        assert_eq!(entries[0].action, "PutPolicy");
        assert_eq!(entries[0].outcome, AuditOutcome::Committed);
        assert_eq!(entries[0].actor_prn, Some(actor.canonical()));

        assert!(events[0].correlation_id.is_some());
        assert_eq!(events[0].correlation_id, entries[0].correlation_id, "the event and the audit entry must share one correlation id");

        assert_eq!(bumper.calls(), 1, "the post-commit gen bump must have been awaited exactly once by the time put returns");
    }

    /// SMA-446 Slice B Task B5 — the crux: a same-content savepoint absorption
    /// (`PutOutcome::AbsorbedIdempotent`) must emit NEITHER an outbox event NOR an audit
    /// entry, and must never bump `policy_gen` — the winning writer already did (module docs
    /// on `pg_policies.rs`/`PolicyStore::put_in`), mirroring `RoleService::revoke`'s
    /// vanished-grant no-op.
    #[tokio::test]
    async fn put_on_an_absorbed_idempotent_outcome_emits_nothing_and_never_bumps() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::PutPolicy, &root_prn());
        let ServiceWithFakes { svc, outbox, audit, bumper } = new_service_with_fakes(fake, Arc::new(AbsorbingPolicyStore));
        let actor = actor_prn(1);

        svc.put(&actor, doc("p1")).await.unwrap();

        assert!(outbox.0.lock().unwrap().is_empty(), "an absorbed put must not enqueue an event");
        assert!(audit.0.lock().unwrap().is_empty(), "an absorbed put must not record an audit entry");
        assert_eq!(bumper.calls(), 0, "an absorbed put must never bump policy_gen");
    }

    /// Guard (SMA-446 Slice B, mirrors `RoleService`'s guard D2): a store error mid-txn must
    /// roll the whole unit of work back — `put` must never enqueue an event, record an audit
    /// entry, or run its post-commit bump for a mutation that never actually committed.
    #[tokio::test]
    async fn a_store_error_mid_txn_rolls_back_and_never_emits_or_bumps() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::PutPolicy, &root_prn());
        let ServiceWithFakes { svc, outbox, audit, bumper } = new_service_with_fakes(fake, Arc::new(FailingPutStore));
        let actor = actor_prn(1);

        let err = svc.put(&actor, doc("p1")).await.unwrap_err();
        assert_eq!(err, TenancyError::Internal, "AuthzError::Backend from a mid-txn store failure maps to Internal");

        assert!(outbox.0.lock().unwrap().is_empty(), "a rolled-back put must not enqueue an event");
        assert!(audit.0.lock().unwrap().is_empty(), "a rolled-back put must not record an audit entry");
        assert_eq!(bumper.calls(), 0, "a rolled-back put must never bump policy_gen");
    }

    /// SMA-446 Slice B — the UoW reference pattern's core contract for `delete`: mirrors
    /// `put`'s own event/audit/bump proof above.
    #[tokio::test]
    async fn delete_emits_one_event_and_one_audit_entry_and_awaits_the_bump() {
        // Seed the row directly through the bare store (bypassing the service under test
        // entirely) so this test's `outbox`/`audit`/`bumper` fakes see ONLY `delete`'s own
        // emissions, not a prior `put`'s.
        let seeded = InMemoryPolicies::default();
        seeded.put(&doc("p1")).await.unwrap();

        let fake = FakeAuthorizer::default();
        fake.allow(Action::DeletePolicy, &root_prn());
        let policies: Arc<dyn PolicyStore> = Arc::new(seeded);
        let ServiceWithFakes { svc, outbox, audit, bumper } = new_service_with_fakes(fake, policies);
        let actor = actor_prn(1);

        svc.delete(&actor, "p1").await.unwrap();

        let events = outbox.0.lock().unwrap();
        assert_eq!(events.len(), 1, "delete must enqueue exactly one domain event");
        assert_eq!(events[0].event_type, EventType::PolicyDeleted);

        let entries = audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1, "delete must record exactly one audit entry");
        assert_eq!(entries[0].action, "DeletePolicy");

        assert_eq!(events[0].correlation_id, entries[0].correlation_id, "the event and the audit entry must share one correlation id");
        assert_eq!(bumper.calls(), 1, "delete's post-commit gen bump must have been awaited exactly once");
    }

    /// SMA-446 Slice B: `delete_in` returning `false` (the policy id never existed) is an
    /// idempotent no-op — `delete` must succeed WITHOUT enqueuing an event, recording an
    /// audit entry, or running its post-commit bump, mirroring `RoleService::revoke`'s
    /// vanished-grant test.
    #[tokio::test]
    async fn delete_of_a_policy_that_never_existed_is_an_idempotent_no_op() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::DeletePolicy, &root_prn());
        let ServiceWithFakes { svc, outbox, audit, bumper } = new_service_with_fakes(fake, Arc::new(InMemoryPolicies::default()));
        let actor = actor_prn(1);

        svc.delete(&actor, "never-existed").await.unwrap();

        assert!(outbox.0.lock().unwrap().is_empty(), "an idempotent no-op delete must not enqueue an event");
        assert!(audit.0.lock().unwrap().is_empty(), "an idempotent no-op delete must not record an audit entry");
        assert_eq!(bumper.calls(), 0, "an idempotent no-op delete must never bump policy_gen");
    }
}
