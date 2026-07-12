// SPDX-License-Identifier: Apache-2.0

//! `CedarAuthorizer`: the `Authorizer` port's implementation (ADR-0013, spec §7) — composes
//! the compiled-policy snapshot (Task 13), the entity-slice loader/cache (Task 12/14), the
//! generation-keyed decision cache (Task 14), and an [`AuditSink`] into the one component
//! `is_authorized` callers reach for. It is wired into `AppState`/`main.rs` (SMA-446 Slice
//! A); this module builds and unit-tests the authorizer itself.
//!
//! **`is_authorized` flow (spec D11/D12, AC1):**
//! 1. Best-effort synchronous [`PolicySnapshot::reload_if_stale`] — a grant made moments ago
//!    is visible to THIS decision without waiting out the background poll interval (AC1). An
//!    `Err` (e.g. a transient store hiccup) is logged and swallowed: reload failures never
//!    fail a decision, they just mean this call evaluates against the last-known-good
//!    snapshot.
//! 2. Read the current compiled-policy snapshot via [`PolicySnapshot::current`] — a cheap,
//!    in-memory `Arc` clone that never errors. Its `r#gen` is the authoritative policy
//!    generation THIS call will evaluate against, and it is the ONLY source of the
//!    decision-cache key's policy component: it is never re-derived from a second,
//!    independently-timed [`GenerationsReader::policy_gen`] read. That second read used to
//!    be able to observe a concurrent policy bump the just-taken snapshot predates, minting
//!    a decision-cache key one generation NEWER than the policy set actually evaluated —
//!    caching a decision under the wrong generation until TTL. Deriving the key's policy
//!    component from `compiled.r#gen` itself closes that window: key and evaluated policy
//!    set are always the same generation, by construction.
//! 3. Read the entity generation counter via [`GenerationsReader::entity_gen`] to complete
//!    the decision-cache key. If it errors (the Redis-backed counter is unreachable), the
//!    cache is bypassed entirely for this call — no key, no `get`, no `put` — and evaluation
//!    proceeds unconditionally (D11/D12's fail-open property: an accelerator outage costs
//!    latency, never correctness). If it succeeds and the key is already cached, that cached
//!    [`Decision`] is returned immediately. Hits re-audit **denials only** (full trail,
//!    D3/D8): a cached `Deny` still gets one fresh audit event per call, because a denial's
//!    audit trail must never have a gap, cached or not. A cached `Allow`, however, is
//!    returned WITHOUT touching the audit sink again — the original miss that populated the
//!    cache already recorded the one audit event for this exact question, and auditing an
//!    `Allow` again here would just double-record it.
//! 4. On a miss (or a bypassed cache), the authoritative path: load the [`EntitySlice`] for
//!    `(resource, principal)` and decide via [`PolicyEngine::decide`] against the SAME
//!    compiled snapshot read in step 2 (never a second `snapshot.current()` read). An
//!    `AuthzError::ResourceNotFound` slice-load error — the request names a tenancy node that
//!    doesn't (or no longer does) exist, reachable via the direct
//!    `POST /v1/authz/is-authorized` API with an arbitrary caller-supplied `resource_prn` —
//!    is caught here and turned into a fail-closed `Deny` (marked
//!    [`RESOURCE_NOT_FOUND_MARKER`]) rather than propagated: never a 500, and never an
//!    existence oracle that would let an unauthorized caller distinguish "denied" from
//!    "doesn't exist". Every OTHER slice-load error still propagates unchanged — it's a
//!    genuine backend failure (the request can't be decided at all), never swallowed.
//! 5. Record one [`AuthzDecisionEvent`] via the injected [`AuditSink`].
//! 6. Best-effort populate the decision cache (only if step 3 computed a key).
//!
//! **Timestamps:** [`AuthzDecisionEvent::at`] is stamped with `chrono::Utc::now()` directly
//! rather than through an injected `Clock` port. `CedarAuthorizer` doesn't otherwise need a
//! clock, and every existing `Clock` consumer in this crate (`adapters::clock::SystemClock`)
//! exists to make a *tested* time-dependent computation (JWKS TTL expiry, etc.)
//! deterministic; nothing here asserts on `at`'s value, so adding a clock dependency here
//! would be DI ceremony without a corresponding test benefit. If a future task needs to
//! assert audit timestamps precisely, threading a `Clock` through here is the natural
//! extension.

use super::decision_cache::decision_key;
use super::generation::Generations;
use super::policy_snapshot::PolicySnapshot;
use async_trait::async_trait;
use paigasus_iam_core::authz::engine::PolicyEngine;
use paigasus_iam_core::authz::model::AuthzDecisionEvent;
use paigasus_iam_core::{AccessRequest, AuditSink, Authorizer, AuthzError, Decision, DecisionCache, Effect, EntitySliceLoader};
use std::sync::Arc;

/// The `determining_policies` marker for the fail-closed `Deny` `is_authorized` returns when
/// the entity-slice loader reports the request's resource doesn't exist (SMA-444 review fix)
/// — mirrors `authz::engine::DEFAULT_DENY_MARKER`'s role as a synthetic, non-Cedar-policy-id
/// diagnostic string.
pub const RESOURCE_NOT_FOUND_MARKER: &str = "resource-not-found";

/// Abstraction over "read the two authz generation counters", so `CedarAuthorizer` can be
/// exercised against a fake that errors (simulating a `Generations::Redis` outage) without
/// any real Redis connection in a unit test. [`Generations`] (Task 10's concrete
/// memory/redis backend) implements this directly below, so production callers just wrap a
/// plain `Generations` in an `Arc`.
#[async_trait]
pub trait GenerationsReader: Send + Sync {
    async fn policy_gen(&self) -> Result<u64, AuthzError>;
    async fn entity_gen(&self) -> Result<u64, AuthzError>;
}

#[async_trait]
impl GenerationsReader for Generations {
    async fn policy_gen(&self) -> Result<u64, AuthzError> {
        Generations::policy_gen(self).await
    }

    async fn entity_gen(&self) -> Result<u64, AuthzError> {
        Generations::entity_gen(self).await
    }
}

/// The `Authorizer` port's implementation (ADR-0013): composes a [`PolicySnapshot`] (the
/// authoritative compiled policy set), an [`EntitySliceLoader`] (typically `SliceCache`
/// wrapping a Postgres-backed loader), a [`DecisionCache`] accelerator, the
/// [`GenerationsReader`] the cache key is derived from, and an [`AuditSink`]. Not `Clone` —
/// callers share ONE instance across every `AppState` clone via `Arc<CedarAuthorizer>`
/// (mirroring `PolicySnapshot`'s own Arc-sharing posture).
pub struct CedarAuthorizer {
    snapshot: Arc<PolicySnapshot>,
    slices: Arc<dyn EntitySliceLoader>,
    decisions: Arc<dyn DecisionCache>,
    gens: Arc<dyn GenerationsReader>,
    audit: Arc<dyn AuditSink>,
}

impl CedarAuthorizer {
    #[must_use]
    pub fn new(snapshot: Arc<PolicySnapshot>, slices: Arc<dyn EntitySliceLoader>, decisions: Arc<dyn DecisionCache>, gens: Arc<dyn GenerationsReader>, audit: Arc<dyn AuditSink>) -> Self {
        Self {
            snapshot,
            slices,
            decisions,
            gens,
            audit,
        }
    }

    /// Reads the entity generation counter and completes the decision-cache key iff it
    /// succeeds (D11/D12 fail-open): an error (a Redis outage) is logged and mapped to
    /// `None` — the caller then skips the cache entirely for this call rather than caching
    /// under a partial/guessed key. `policy_gen` is deliberately NOT read here (nor anywhere
    /// else in this module) — the key's policy component is always `compiled_gen`, the
    /// `r#gen` of the exact [`CompiledPolicies`] snapshot the caller evaluated, so the key
    /// and the evaluated policy set can never drift apart.
    async fn cache_key(&self, compiled_gen: u64, req: &AccessRequest) -> Option<String> {
        match self.gens.entity_gen().await {
            Ok(entity_gen) => Some(decision_key(compiled_gen, entity_gen, req)),
            Err(err) => {
                tracing::warn!(error = %err, "cedar_authorizer: entity generation counter unreadable — bypassing the decision cache for this call (fail-open, D11/D12)");
                None
            }
        }
    }
}

#[async_trait]
impl Authorizer for CedarAuthorizer {
    async fn is_authorized(&self, req: &AccessRequest) -> Result<Decision, AuthzError> {
        // Step 1 (AC1): best-effort synchronous staleness reload. A reload error never fails
        // the decision — it just means this call evaluates against the last-known-good
        // snapshot rather than a fresher one.
        if let Err(err) = self.snapshot.reload_if_stale().await {
            tracing::warn!(error = %err, "cedar_authorizer: policy snapshot reload_if_stale failed — deciding against the last-known-good snapshot");
        }

        // Step 2: read the snapshot that will actually be evaluated FIRST. `compiled.r#gen`
        // is the authoritative policy generation this call decides against, and it's the
        // source of the decision-cache key's policy component below — never a second,
        // independently-timed `Generations::policy_gen()` read.
        let compiled = self.snapshot.current().await;

        // Step 3: fail-open cache lookup, keyed off `compiled.r#gen` (not a fresh
        // `policy_gen()` read) plus the entity generation.
        let cache_key = self.cache_key(compiled.r#gen, req).await;
        if let Some(key) = &cache_key
            && let Some(cached) = self.decisions.get(key).await
        {
            // Hits re-audit denials only (full trail, D3/D8): a cached `Deny` still gets a
            // fresh audit event on every call, even though the decision itself is served
            // from cache — a denial's audit trail must never have a gap. A cached `Allow` is
            // NOT re-audited here: the original miss that populated the cache already
            // recorded the one audit event for this exact question, and auditing it again
            // would just double-record the same decision.
            if cached.effect == Effect::Deny {
                let event = AuthzDecisionEvent {
                    principal_prn: req.principal.canonical(),
                    action: req.action.as_wire().to_string(),
                    resource_prn: req.resource.canonical(),
                    effect: cached.effect,
                    determining_policies: cached.determining_policies.clone(),
                    at: chrono::Utc::now(),
                };
                self.audit.record(&event).await;
            }
            return Ok(cached);
        }

        // Step 4: the authoritative path — always runs on a miss OR a bypassed cache. Uses
        // the SAME `compiled` snapshot read in step 2, so the policy set evaluated here can
        // never be a different generation than the one the cache key (step 3) was minted
        // from. `ResourceNotFound` is caught and turned into a fail-closed `Deny` — never a
        // 500, and never an existence oracle — while every other slice-load error (a genuine
        // backend failure) still propagates unchanged.
        let decision = match self.slices.load(&req.resource, &req.principal).await {
            Ok(slice) => PolicyEngine::decide(&compiled.policy_set, &slice, req),
            Err(AuthzError::ResourceNotFound(_)) => Decision {
                effect: Effect::Deny,
                determining_policies: vec![RESOURCE_NOT_FOUND_MARKER.to_string()],
            },
            Err(err) => return Err(err),
        };

        // Step 5: audit every decision this method actually computes (never on a cache hit).
        let event = AuthzDecisionEvent {
            principal_prn: req.principal.canonical(),
            action: req.action.as_wire().to_string(),
            resource_prn: req.resource.canonical(),
            effect: decision.effect,
            determining_policies: decision.determining_policies.clone(),
            at: chrono::Utc::now(),
        };
        self.audit.record(&event).await;

        // Step 6: best-effort populate the cache (only if step 3 minted a key).
        if let Some(key) = cache_key {
            self.decisions.put(&key, &decision).await;
        }

        Ok(decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::authz::decision_cache::MemoryDecisionCache;
    use paigasus_iam_core::authz::model::{ContextValue, EntitySlice, GrantScope, ROOT_ENTITY, SliceEntity};
    use paigasus_iam_core::authz::roles::starter_policies;
    use paigasus_iam_core::tenancy::{OrganizationId, ProjectId, TeamId, TenancyNodeRef};
    use paigasus_iam_core::{Action, Effect, PolicyDocument, PolicyStore, PrincipalId, RequestContext, RoleGrant, RoleGrantStore, Transaction};
    use paigasus_kernel::{Prn, to_cedar_uid};
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn principal_prn(n: u128) -> Prn {
        Prn::build("iam", "", None, "principal", u(n)).expect("static test prn parts are valid")
    }

    fn cedar_tuple(prn: &Prn) -> (String, String) {
        let uid = to_cedar_uid(prn);
        (uid.entity_type, uid.entity_id)
    }

    fn root_tuple() -> (String, String) {
        (ROOT_ENTITY.0.to_string(), ROOT_ENTITY.1.to_string())
    }

    fn active_attrs() -> BTreeMap<String, ContextValue> {
        BTreeMap::from([("effective_status".to_string(), ContextValue::Str("active".to_string()))])
    }

    /// A `Root -> org -> team -> project` hierarchy plus one principal — the same shared
    /// entity universe every test in this module decides requests against. The
    /// [`FixtureSliceLoader`] fake below always returns this same slice; every test here
    /// only ever asks about `project`, so a canned, unvarying slice is enough.
    struct Fixture {
        org: OrganizationId,
        project: ProjectId,
        principal: Prn,
        slice: EntitySlice,
    }

    fn fixture() -> Fixture {
        let org = OrganizationId::from_uuid(u(1));
        let team = TeamId::from_parts(org.uuid(), u(2));
        let project = ProjectId::from_parts(org.uuid(), u(3));
        let principal = principal_prn(4);

        let entities = vec![
            SliceEntity {
                uid: root_tuple(),
                parents: vec![],
                attrs: BTreeMap::new(),
            },
            SliceEntity {
                uid: cedar_tuple(org.prn()),
                parents: vec![root_tuple()],
                attrs: active_attrs(),
            },
            SliceEntity {
                uid: cedar_tuple(team.prn()),
                parents: vec![cedar_tuple(org.prn())],
                attrs: active_attrs(),
            },
            SliceEntity {
                uid: cedar_tuple(project.prn()),
                parents: vec![cedar_tuple(team.prn())],
                attrs: active_attrs(),
            },
            SliceEntity {
                uid: cedar_tuple(&principal),
                parents: vec![],
                attrs: BTreeMap::from([
                    ("kind".to_string(), ContextValue::Str("user".to_string())),
                    ("status".to_string(), ContextValue::Str("active".to_string())),
                ]),
            },
        ];

        Fixture {
            org,
            project,
            principal,
            slice: EntitySlice { entities },
        }
    }

    fn org_admin_grant(id: Uuid, principal: &Prn, org: &OrganizationId) -> RoleGrant {
        RoleGrant {
            id,
            principal: PrincipalId::from_prn(principal.clone()),
            role_key: "org_admin".to_string(),
            scope: GrantScope::Node(TenancyNodeRef::Organization(org.clone())),
            linked_policy_id: format!("grant:{id}"),
            created_at: chrono::Utc::now(),
        }
    }

    /// In-memory `PolicyStore` fake sharing a caller-supplied [`Generations`] handle for its
    /// own `policy_gen`/`bump_policy_gen` — mirroring how the real `PgPolicyStore` (Task 10)
    /// and `CedarAuthorizer::gens` share ONE `Generations` handle in production wiring. This
    /// is load-bearing for the AC1 test below: bumping through this store must ALSO be
    /// visible to a `CedarAuthorizer` built over the same `Generations` clone, so the second
    /// call mints a different decision-cache key rather than replaying the first call's
    /// cached (stale) deny.
    struct FakePolicyStore {
        docs: Mutex<Vec<PolicyDocument>>,
        gens: Generations,
    }

    impl FakePolicyStore {
        fn new(docs: Vec<PolicyDocument>, gens: Generations) -> Self {
            Self { docs: Mutex::new(docs), gens }
        }
    }

    #[async_trait]
    impl PolicyStore for FakePolicyStore {
        async fn list_all(&self) -> Result<Vec<PolicyDocument>, AuthzError> {
            Ok(self.docs.lock().unwrap().clone())
        }

        async fn put(&self, _doc: &PolicyDocument) -> Result<(), AuthzError> {
            unimplemented!("cedar_authorizer tests never write through PolicyStore::put")
        }

        async fn delete(&self, _policy_id: &str) -> Result<(), AuthzError> {
            unimplemented!("cedar_authorizer tests never write through PolicyStore::delete")
        }

        async fn policy_gen(&self) -> Result<u64, AuthzError> {
            self.gens.policy_gen().await
        }

        async fn bump_policy_gen(&self) -> Result<u64, AuthzError> {
            self.gens.bump_policy_gen().await
        }
    }

    /// In-memory `RoleGrantStore` fake: a plain `Mutex<Vec<RoleGrant>>`, seeded up front.
    struct FakeRoleGrantStore {
        grants: Mutex<Vec<RoleGrant>>,
    }

    impl FakeRoleGrantStore {
        fn new(grants: Vec<RoleGrant>) -> Self {
            Self { grants: Mutex::new(grants) }
        }
    }

    #[async_trait]
    impl RoleGrantStore for FakeRoleGrantStore {
        async fn grant(&self, g: &RoleGrant) -> Result<(), AuthzError> {
            self.grants.lock().unwrap().push(g.clone());
            Ok(())
        }

        async fn revoke(&self, id: Uuid) -> Result<(), AuthzError> {
            self.grants.lock().unwrap().retain(|g| g.id != id);
            Ok(())
        }

        // Txn-scoped twins (SMA-446, Slice B): this fake has no real backing transaction, so
        // `tx` is ignored and the mutation applies immediately — mirrors `grant`/`revoke`
        // above, which never used a `Transaction` either.
        async fn grant_in(&self, _tx: &dyn Transaction, g: &RoleGrant) -> Result<(), AuthzError> {
            self.grants.lock().unwrap().push(g.clone());
            Ok(())
        }

        async fn revoke_in(&self, _tx: &dyn Transaction, id: Uuid) -> Result<bool, AuthzError> {
            let mut grants = self.grants.lock().unwrap();
            let before = grants.len();
            grants.retain(|g| g.id != id);
            Ok(grants.len() != before)
        }

        async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(self.grants.lock().unwrap().clone())
        }

        async fn list_by_principal(&self, _p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError> {
            unimplemented!("cedar_authorizer tests never query by principal")
        }

        async fn find(&self, id: Uuid) -> Result<Option<RoleGrant>, AuthzError> {
            Ok(self.grants.lock().unwrap().iter().find(|g| g.id == id).cloned())
        }
    }

    /// An `EntitySliceLoader` fake that always returns the same canned slice, counting calls
    /// so tests can assert whether the decision cache actually short-circuited it.
    struct FixtureSliceLoader {
        slice: EntitySlice,
        calls: AtomicUsize,
    }

    impl FixtureSliceLoader {
        fn new(slice: EntitySlice) -> Self {
            Self { slice, calls: AtomicUsize::new(0) }
        }
    }

    #[async_trait]
    impl EntitySliceLoader for FixtureSliceLoader {
        async fn load(&self, _resource: &Prn, _principal: &Prn) -> Result<EntitySlice, AuthzError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.slice.clone())
        }

        async fn entity_gen(&self) -> Result<u64, AuthzError> {
            Ok(0)
        }
    }

    /// An `EntitySliceLoader` fake that always fails with a genuine backend error — proves a
    /// non-`ResourceNotFound` slice-load error propagates out of `is_authorized` rather than
    /// being swallowed into a `Deny` (distinct from `ResourceNotFoundSliceLoader` below, whose
    /// specific error variant DOES get caught and turned into a `Deny`).
    struct FailingSliceLoader;

    #[async_trait]
    impl EntitySliceLoader for FailingSliceLoader {
        async fn load(&self, _resource: &Prn, _principal: &Prn) -> Result<EntitySlice, AuthzError> {
            Err(AuthzError::Backend("simulated postgres outage loading the entity slice".into()))
        }

        async fn entity_gen(&self) -> Result<u64, AuthzError> {
            Ok(0)
        }
    }

    /// An `EntitySliceLoader` fake that always fails with `AuthzError::ResourceNotFound` —
    /// mirrors `PgEntitySliceLoader::load`'s `missing(..)` case (the request names a tenancy
    /// node that doesn't exist). Proves `is_authorized` catches exactly this variant and
    /// turns it into a fail-closed `Deny`, never a propagated error (SMA-444 review fix).
    struct ResourceNotFoundSliceLoader;

    #[async_trait]
    impl EntitySliceLoader for ResourceNotFoundSliceLoader {
        async fn load(&self, _resource: &Prn, _principal: &Prn) -> Result<EntitySlice, AuthzError> {
            Err(AuthzError::ResourceNotFound("organization deadbeef not found for entity-slice load".to_string()))
        }

        async fn entity_gen(&self) -> Result<u64, AuthzError> {
            Ok(0)
        }
    }

    /// A capturing `AuditSink` fake: records every event into a `Mutex<Vec<_>>` so tests can
    /// assert both content and count (in particular, that a cache hit never double-audits).
    #[derive(Default)]
    struct CapturingAuditSink {
        events: Mutex<Vec<AuthzDecisionEvent>>,
    }

    #[async_trait]
    impl AuditSink for CapturingAuditSink {
        async fn record(&self, ev: &AuthzDecisionEvent) {
            self.events.lock().unwrap().push(ev.clone());
        }
    }

    /// A `GenerationsReader` fake whose both reads always fail — simulates a
    /// `Generations::Redis` backend whose Redis is down, without any real network I/O.
    struct FailingGenerations;

    #[async_trait]
    impl GenerationsReader for FailingGenerations {
        async fn policy_gen(&self) -> Result<u64, AuthzError> {
            Err(AuthzError::Backend("simulated generations-redis outage".into()))
        }

        async fn entity_gen(&self) -> Result<u64, AuthzError> {
            Err(AuthzError::Backend("simulated generations-redis outage".into()))
        }
    }

    /// A `GenerationsReader` fake whose `policy_gen()` returns a DIFFERENT value on every
    /// single call (as if a policy bump landed in the window between every read), while
    /// `entity_gen()` stays stable. This exists to prove the cache-key generation-drift fix:
    /// `is_authorized` must never call `policy_gen()` at all, deriving the key's policy
    /// component solely from the evaluated `CompiledPolicies::r#gen`. If `is_authorized`
    /// still read `policy_gen()` to mint the key (the pre-fix behavior), two back-to-back
    /// identical calls against an unchanged snapshot would mint two different keys and the
    /// decision cache would never hit.
    struct DriftingPolicyGenerations {
        policy_gen_calls: AtomicUsize,
    }

    impl DriftingPolicyGenerations {
        fn new() -> Self {
            Self {
                policy_gen_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl GenerationsReader for DriftingPolicyGenerations {
        async fn policy_gen(&self) -> Result<u64, AuthzError> {
            Ok(self.policy_gen_calls.fetch_add(1, Ordering::SeqCst) as u64)
        }

        async fn entity_gen(&self) -> Result<u64, AuthzError> {
            Ok(0)
        }
    }

    fn base_request(fx: &Fixture, action: Action) -> AccessRequest {
        AccessRequest {
            principal: fx.principal.clone(),
            action,
            resource: fx.project.prn().clone(),
            context: RequestContext::empty(),
        }
    }

    #[tokio::test]
    async fn default_deny_records_exactly_one_audit_event() {
        let fx = fixture();
        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(starter_policies(), Generations::memory()));
        let grants: Arc<dyn RoleGrantStore> = Arc::new(FakeRoleGrantStore::new(vec![]));
        let snapshot = Arc::new(PolicySnapshot::new(policies, grants).await.expect("snapshot builds"));
        let slices = Arc::new(FixtureSliceLoader::new(fx.slice.clone()));
        let audit = Arc::new(CapturingAuditSink::default());

        let authorizer = CedarAuthorizer::new(
            snapshot,
            slices as Arc<dyn EntitySliceLoader>,
            Arc::new(MemoryDecisionCache::new()) as Arc<dyn DecisionCache>,
            Arc::new(Generations::memory()) as Arc<dyn GenerationsReader>,
            audit.clone() as Arc<dyn AuditSink>,
        );

        let req = base_request(&fx, Action::GetProject);
        let decision = authorizer.is_authorized(&req).await.expect("decision succeeds");
        assert_eq!(decision.effect, Effect::Deny);

        let events = audit.events.lock().unwrap();
        assert_eq!(events.len(), 1, "exactly one decision must be audited");
        assert_eq!(events[0].effect, Effect::Deny);
        assert_eq!(events[0].determining_policies, vec![paigasus_iam_core::authz::engine::DEFAULT_DENY_MARKER.to_string()]);
        assert_eq!(events[0].principal_prn, fx.principal.canonical());
        assert_eq!(events[0].resource_prn, fx.project.prn().canonical());
    }

    /// AC1: a grant made after construction must be visible to the SAME `CedarAuthorizer`
    /// instance's very next call — proving `is_authorized` calls `reload_if_stale`
    /// synchronously rather than relying on the background poll loop.
    #[tokio::test]
    async fn allow_after_grant_reloads_synchronously_ac1() {
        let fx = fixture();
        let gens = Generations::memory();
        let policies_store = Arc::new(FakePolicyStore::new(starter_policies(), gens.clone()));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();
        let snapshot = Arc::new(PolicySnapshot::new(policies, grants).await.expect("snapshot builds"));
        let slices = Arc::new(FixtureSliceLoader::new(fx.slice.clone()));
        let audit = Arc::new(CapturingAuditSink::default());

        let authorizer = CedarAuthorizer::new(
            snapshot,
            slices as Arc<dyn EntitySliceLoader>,
            Arc::new(MemoryDecisionCache::new()) as Arc<dyn DecisionCache>,
            // Share the SAME `Generations` handle the `FakePolicyStore` bumps below — this
            // mirrors production wiring (one `Generations` behind both `PgPolicyStore` and
            // `CedarAuthorizer::gens`) and, just as importantly, means the bump that makes
            // `PolicySnapshot` reload ALSO mints a fresh decision-cache key, so the second
            // call below can't be served the first call's cached `Deny`.
            Arc::new(gens) as Arc<dyn GenerationsReader>,
            audit.clone() as Arc<dyn AuditSink>,
        );

        let req = base_request(&fx, Action::CreateProject);

        let before = authorizer.is_authorized(&req).await.expect("decision succeeds");
        assert_eq!(before.effect, Effect::Deny, "no grant yet");

        let grant_id = u(200);
        grants_store.grant(&org_admin_grant(grant_id, &fx.principal, &fx.org)).await.expect("grant succeeds");
        policies_store.bump_policy_gen().await.expect("bump succeeds");

        let after = authorizer.is_authorized(&req).await.expect("decision succeeds");
        assert_eq!(after.effect, Effect::Allow);
        assert_eq!(after.determining_policies, vec![format!("grant:{grant_id}")]);

        assert_eq!(audit.events.lock().unwrap().len(), 2, "two distinct decisions were computed, both audited");
    }

    /// A cache hit still skips the (expensive) slice load — that part of the original
    /// short-circuit claim holds regardless of effect. What changed (SMA-446, D3/D8): this
    /// fixture's request is a default-deny (no grant), so the cache-hit `Deny` is now
    /// RE-audited for the full trail rather than silently skipped — see
    /// `cache_hit_deny_is_re_audited_but_cache_hit_allow_is_not` below for the direct
    /// deny-vs-allow contrast.
    #[tokio::test]
    async fn cache_hit_short_circuits_the_slice_load_but_re_audits_a_cached_deny() {
        let fx = fixture();
        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(starter_policies(), Generations::memory()));
        let grants: Arc<dyn RoleGrantStore> = Arc::new(FakeRoleGrantStore::new(vec![]));
        let snapshot = Arc::new(PolicySnapshot::new(policies, grants).await.expect("snapshot builds"));
        let slices = Arc::new(FixtureSliceLoader::new(fx.slice.clone()));
        let audit = Arc::new(CapturingAuditSink::default());

        let authorizer = CedarAuthorizer::new(
            snapshot,
            slices.clone() as Arc<dyn EntitySliceLoader>,
            Arc::new(MemoryDecisionCache::new()) as Arc<dyn DecisionCache>,
            Arc::new(Generations::memory()) as Arc<dyn GenerationsReader>,
            audit.clone() as Arc<dyn AuditSink>,
        );

        let req = base_request(&fx, Action::GetProject);

        let first = authorizer.is_authorized(&req).await.expect("first call succeeds");
        let second = authorizer.is_authorized(&req).await.expect("second call succeeds");

        assert_eq!(first, second);
        assert_eq!(first.effect, Effect::Deny, "no grant yet — the fixture principal is denied by default");
        assert_eq!(slices.calls.load(Ordering::SeqCst), 1, "a cache hit must not re-invoke the slice loader");
        assert_eq!(
            audit.events.lock().unwrap().len(),
            2,
            "a cache-hit Deny must be re-audited for the full trail (D3/D8), even though the slice load was skipped"
        );
    }

    /// Direct contrast of the two cache-hit audit branches (SMA-446, D3/D8): a cache-hit
    /// `Deny` is re-audited every single call (a denial's audit trail must never have a gap),
    /// while a cache-hit `Allow` is NOT — the original miss already recorded it, and auditing
    /// it again on every subsequent hit would just double-record the same decision. Both
    /// halves also assert the slice loader ran exactly once, proving the second call in each
    /// pair was a genuine cache hit and not a re-evaluation.
    #[tokio::test]
    async fn cache_hit_deny_is_re_audited_but_cache_hit_allow_is_not() {
        // --- Deny path: default-deny, no grant seeded. ---
        let fx = fixture();
        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(starter_policies(), Generations::memory()));
        let grants: Arc<dyn RoleGrantStore> = Arc::new(FakeRoleGrantStore::new(vec![]));
        let snapshot = Arc::new(PolicySnapshot::new(policies, grants).await.expect("snapshot builds"));
        let slices = Arc::new(FixtureSliceLoader::new(fx.slice.clone()));
        let audit = Arc::new(CapturingAuditSink::default());

        let authorizer = CedarAuthorizer::new(
            snapshot,
            slices.clone() as Arc<dyn EntitySliceLoader>,
            Arc::new(MemoryDecisionCache::new()) as Arc<dyn DecisionCache>,
            Arc::new(Generations::memory()) as Arc<dyn GenerationsReader>,
            audit.clone() as Arc<dyn AuditSink>,
        );

        let req = base_request(&fx, Action::GetProject);

        let first = authorizer.is_authorized(&req).await.expect("first (miss) call succeeds");
        assert_eq!(first.effect, Effect::Deny);
        let second = authorizer.is_authorized(&req).await.expect("second (hit) call succeeds");
        assert_eq!(second, first);

        assert_eq!(slices.calls.load(Ordering::SeqCst), 1, "the second call was a real cache hit — the slice loader ran only once");
        assert_eq!(audit.events.lock().unwrap().len(), 2, "miss + re-audited hit: the cache-hit Deny must be re-audited for the full trail");

        // --- Allow path: seed an org_admin grant so the decision is Allow from the start. ---
        let fx = fixture();
        let grant_id = u(500);
        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(starter_policies(), Generations::memory()));
        let grants: Arc<dyn RoleGrantStore> = Arc::new(FakeRoleGrantStore::new(vec![org_admin_grant(grant_id, &fx.principal, &fx.org)]));
        let snapshot = Arc::new(PolicySnapshot::new(policies, grants).await.expect("snapshot builds"));
        let slices = Arc::new(FixtureSliceLoader::new(fx.slice.clone()));
        let audit = Arc::new(CapturingAuditSink::default());

        let authorizer = CedarAuthorizer::new(
            snapshot,
            slices.clone() as Arc<dyn EntitySliceLoader>,
            Arc::new(MemoryDecisionCache::new()) as Arc<dyn DecisionCache>,
            Arc::new(Generations::memory()) as Arc<dyn GenerationsReader>,
            audit.clone() as Arc<dyn AuditSink>,
        );

        let req = base_request(&fx, Action::CreateProject);

        let first = authorizer.is_authorized(&req).await.expect("first (miss) call succeeds");
        assert_eq!(first.effect, Effect::Allow);
        let second = authorizer.is_authorized(&req).await.expect("second (hit) call succeeds");
        assert_eq!(second, first);

        assert_eq!(slices.calls.load(Ordering::SeqCst), 1, "the second call was a real cache hit — the slice loader ran only once");
        assert_eq!(
            audit.events.lock().unwrap().len(),
            1,
            "a cache-hit Allow must NOT be re-audited — the original miss already recorded it"
        );
    }

    /// D11/D12 fail-open: when the generation counters can't be read at all (a
    /// `Generations::Redis` outage, simulated here without any real Redis), `is_authorized`
    /// must still evaluate and return a correct `Decision` — it just can't accelerate future
    /// identical calls, since neither `get` nor `put` can safely run without a key.
    #[tokio::test]
    async fn fail_open_on_a_generations_read_error_still_decides_and_never_caches() {
        let fx = fixture();
        let grant_id = u(300);
        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(starter_policies(), Generations::memory()));
        // Seed the grant at construction time so `PolicySnapshot::new` compiles it in from
        // the start — this test is about the GENERATIONS read failing, not about reload
        // timing, which the AC1 test above already covers.
        let grants: Arc<dyn RoleGrantStore> = Arc::new(FakeRoleGrantStore::new(vec![org_admin_grant(grant_id, &fx.principal, &fx.org)]));
        let snapshot = Arc::new(PolicySnapshot::new(policies, grants).await.expect("snapshot builds"));
        let slices = Arc::new(FixtureSliceLoader::new(fx.slice.clone()));
        let audit = Arc::new(CapturingAuditSink::default());

        let authorizer = CedarAuthorizer::new(
            snapshot,
            slices.clone() as Arc<dyn EntitySliceLoader>,
            Arc::new(MemoryDecisionCache::new()) as Arc<dyn DecisionCache>,
            Arc::new(FailingGenerations) as Arc<dyn GenerationsReader>,
            audit.clone() as Arc<dyn AuditSink>,
        );

        let req = base_request(&fx, Action::CreateProject);

        let first = authorizer.is_authorized(&req).await.expect("a generations-read error must not fail the decision");
        assert_eq!(first.effect, Effect::Allow);
        assert_eq!(first.determining_policies, vec![format!("grant:{grant_id}")]);

        // A second, identical call must ALSO re-evaluate — proving nothing was cached under
        // a partial/guessed key when the generation reads failed.
        let second = authorizer.is_authorized(&req).await.expect("still decides on the second call");
        assert_eq!(second, first);
        assert_eq!(
            slices.calls.load(Ordering::SeqCst),
            2,
            "the cache must never be consulted when the generations read failed — every call re-evaluates"
        );
        assert_eq!(audit.events.lock().unwrap().len(), 2, "both (uncached) decisions are audited");
    }

    /// Regression test for the cache-key generation-drift bug this fix closes: the
    /// decision-cache key's policy component must come from `compiled.r#gen` (the exact
    /// `PolicySnapshot` generation actually evaluated), never from a second,
    /// independently-timed `GenerationsReader::policy_gen()` read. `DriftingPolicyGenerations`
    /// returns a different `policy_gen()` value on every call while the compiled snapshot
    /// never changes here (no grant/bump happens in this test) — if `is_authorized` still
    /// read `policy_gen()` to mint the key, that would manifest as a permanent cache miss (a
    /// fresh key every call, so the slice loader re-runs); with the fix, the key is stable
    /// across calls at an unchanged snapshot generation, so the second call hits.
    #[tokio::test]
    async fn decision_cache_key_uses_the_evaluated_snapshot_gen_not_a_live_policy_gen_read() {
        let fx = fixture();
        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(starter_policies(), Generations::memory()));
        let grants: Arc<dyn RoleGrantStore> = Arc::new(FakeRoleGrantStore::new(vec![]));
        let snapshot = Arc::new(PolicySnapshot::new(policies, grants).await.expect("snapshot builds"));
        let slices = Arc::new(FixtureSliceLoader::new(fx.slice.clone()));
        let audit = Arc::new(CapturingAuditSink::default());

        let authorizer = CedarAuthorizer::new(
            snapshot,
            slices.clone() as Arc<dyn EntitySliceLoader>,
            Arc::new(MemoryDecisionCache::new()) as Arc<dyn DecisionCache>,
            Arc::new(DriftingPolicyGenerations::new()) as Arc<dyn GenerationsReader>,
            audit.clone() as Arc<dyn AuditSink>,
        );

        let req = base_request(&fx, Action::GetProject);

        let first = authorizer.is_authorized(&req).await.expect("first call succeeds");
        let second = authorizer.is_authorized(&req).await.expect("second call succeeds");

        assert_eq!(first, second);
        assert_eq!(
            slices.calls.load(Ordering::SeqCst),
            1,
            "the cache key must stay stable across calls at an unchanged snapshot generation \
             even though this fake's policy_gen() drifts on every call — proving the key is \
             never derived from a live policy_gen() read"
        );
        // This request is a default-deny (no grant), so the cache-hit second call re-audits
        // (SMA-446, D3/D8) — this assertion is about the audit COUNT tracking the effect, not
        // about the cache-key stability this test otherwise exercises.
        assert_eq!(audit.events.lock().unwrap().len(), 2, "a cache-hit Deny is re-audited for the full trail, even at a stable cache key");
    }

    #[tokio::test]
    async fn slice_load_error_propagates_rather_than_deciding_silently() {
        let fx = fixture();
        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(starter_policies(), Generations::memory()));
        let grants: Arc<dyn RoleGrantStore> = Arc::new(FakeRoleGrantStore::new(vec![]));
        let snapshot = Arc::new(PolicySnapshot::new(policies, grants).await.expect("snapshot builds"));
        let audit = Arc::new(CapturingAuditSink::default());

        let authorizer = CedarAuthorizer::new(
            snapshot,
            Arc::new(FailingSliceLoader) as Arc<dyn EntitySliceLoader>,
            Arc::new(MemoryDecisionCache::new()) as Arc<dyn DecisionCache>,
            Arc::new(Generations::memory()) as Arc<dyn GenerationsReader>,
            audit.clone() as Arc<dyn AuditSink>,
        );

        let req = base_request(&fx, Action::GetProject);
        let err = authorizer.is_authorized(&req).await.expect_err("a slice-load failure must propagate");
        assert!(matches!(err, AuthzError::Backend(_)));
        assert!(audit.events.lock().unwrap().is_empty(), "no decision was computed, so nothing should be audited");
    }

    /// SMA-444 review fix: unlike a genuine backend failure (the test above), a
    /// `ResourceNotFound` slice-load error must NOT propagate — `is_authorized` catches it and
    /// returns a fail-closed `Deny` marked `RESOURCE_NOT_FOUND_MARKER`, and that decision is
    /// still audited like any other (never a silent no-op, and never a 500).
    #[tokio::test]
    async fn resource_not_found_slice_load_denies_instead_of_propagating_and_is_audited() {
        let fx = fixture();
        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(starter_policies(), Generations::memory()));
        let grants: Arc<dyn RoleGrantStore> = Arc::new(FakeRoleGrantStore::new(vec![]));
        let snapshot = Arc::new(PolicySnapshot::new(policies, grants).await.expect("snapshot builds"));
        let audit = Arc::new(CapturingAuditSink::default());

        let authorizer = CedarAuthorizer::new(
            snapshot,
            Arc::new(ResourceNotFoundSliceLoader) as Arc<dyn EntitySliceLoader>,
            Arc::new(MemoryDecisionCache::new()) as Arc<dyn DecisionCache>,
            Arc::new(Generations::memory()) as Arc<dyn GenerationsReader>,
            audit.clone() as Arc<dyn AuditSink>,
        );

        let req = base_request(&fx, Action::GetProject);
        let decision = authorizer.is_authorized(&req).await.expect("a missing resource must deny, never error");
        assert_eq!(decision.effect, Effect::Deny);
        assert_eq!(decision.determining_policies, vec![RESOURCE_NOT_FOUND_MARKER.to_string()]);

        let events = audit.events.lock().unwrap();
        assert_eq!(events.len(), 1, "the fail-closed deny must still be audited, like any other decision");
        assert_eq!(events[0].effect, Effect::Deny);
    }
}
