// SPDX-License-Identifier: Apache-2.0

//! `PolicySnapshot`: the authoritative, in-process compiled Cedar policy set (spec §7,
//! D11, ADR-0013) — decisions evaluate against this even if the accelerator caches
//! (entity-slice, decision) or Redis itself are unavailable.
//!
//! The spec sketches this component as an `ArcSwap<CompiledPolicies>`; this implementation
//! uses `tokio::sync::RwLock<Arc<CompiledPolicies>>` instead (plan Task 13) to avoid adding
//! the `arc-swap` crate for what is not a per-request hot path — policy changes are rare
//! (CRUD / grant / revoke), so an uncontended read-lock's cost is immaterial next to the
//! Postgres + Cedar-compile work a reload already does.
//!
//! **Reload triggers** (D11 / AC1 / AC3): [`PolicySnapshot::reload_if_stale`] recompiles
//! when the store's `policy_gen` has advanced past the currently-compiled `r#gen`;
//! [`PolicySnapshot::spawn_reload`] polls that on an interval AND forces an unconditional
//! reload once `ttl` has elapsed since the last successful (re)load, bounding staleness even
//! if the generation counter never visibly moves to this replica (e.g. the `memory` backend
//! under a change made through a different process). A later task's `CedarAuthorizer` also
//! calls `reload_if_stale` synchronously before deciding, so a grant is visible on the same
//! replica immediately (AC1) rather than waiting out the poll interval.
//!
//! **No-lost-update gen stamping:** [`PolicySnapshot::load_and_compile`] reads `policy_gen`
//! *before* `list_all`-ing policies/grants, and stamps the freshly compiled
//! `CompiledPolicies::r#gen` with that pre-load value — never a value read after the load
//! completes. If a concurrent writer's bump lands in the gap between the gen read and the
//! list reads, the stamped gen still undercounts the store's true current gen, so the NEXT
//! `reload_if_stale` still sees itself as stale and reloads again — the concurrent change is
//! delayed by at most one extra reload, never silently dropped.
//!
//! **Never poisons on a transient error:** every reload path (a manual call, or one iteration
//! of `spawn_reload`'s loop) only swaps in a new compiled snapshot on `Ok` from
//! `load_and_compile`; an `Err` (a transient Postgres hiccup, say) is logged and the previous
//! known-good snapshot keeps serving decisions.
//!
//! **Monotonic-write guard:** [`PolicySnapshot::reload_now`] does not swap in a freshly
//! compiled set unconditionally — [`PolicySnapshot::install_if_newer`] installs it only if
//! its `r#gen` is strictly newer than what's already live. Two reloads can race for the
//! write lock and finish out of gen order (the one that observed an OLDER `policy_gen` can
//! acquire the lock AFTER one that observed a newer gen, e.g. if it was already mid-compile
//! when the second reload started and read a fresher gen); an unconditional swap would then
//! regress the in-memory snapshot to a stale policy set — transiently un-revoking a revoked
//! grant, say — until the next reload self-heals it. That self-heal window is small but
//! security-adjacent, so the swap itself is guarded rather than relied upon to self-correct.

use cedar_policy::PolicyId;
use paigasus_iam_core::authz::engine::{CompiledPolicies, PolicyEngine};
use paigasus_iam_core::{AuthzError, PolicyStore, RoleGrantStore};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

/// The mutable half of [`PolicySnapshot`]'s state: the current compiled policy set plus
/// when it was last (re)loaded — bundled behind one lock so a reload's swap and its
/// TTL-clock reset are always observed together by [`PolicySnapshot::spawn_reload`]'s TTL
/// check.
struct SnapshotState {
    compiled: Arc<CompiledPolicies>,
    loaded_at: Instant,
}

/// The authoritative compiled Cedar policy set, cached in-process and kept fresh via
/// [`PolicyStore::policy_gen`] (D11). Not `Clone` itself — callers share ONE instance across
/// every `AppState` clone via `Arc<PolicySnapshot>` (mirroring
/// `adapters::http::WiredAuthenticator`'s Arc-sharing posture), so a reload triggered through
/// any handle is immediately visible to every other clone.
pub struct PolicySnapshot {
    policies: Arc<dyn PolicyStore>,
    grants: Arc<dyn RoleGrantStore>,
    state: RwLock<SnapshotState>,
}

impl PolicySnapshot {
    /// Loads every policy/template + role grant, compiles them, and stamps the result with
    /// the `policy_gen` observed at the START of this load (see the module docs' "no
    /// lost update" note).
    pub async fn new(policies: Arc<dyn PolicyStore>, grants: Arc<dyn RoleGrantStore>) -> Result<Self, AuthzError> {
        let compiled = Self::load_and_compile(policies.as_ref(), grants.as_ref()).await?;
        Ok(Self {
            policies,
            grants,
            state: RwLock::new(SnapshotState {
                compiled: Arc::new(compiled),
                loaded_at: Instant::now(),
            }),
        })
    }

    /// The current compiled policy set: a read-lock, an `Arc` clone, then release — cheap
    /// enough to call once per decision.
    ///
    /// `async` rather than the sync `fn` a bare `Arc<RwLock<_>>` read might suggest:
    /// `tokio::sync::RwLock`'s only synchronous read, `blocking_read`, panics when called
    /// from within an async execution context — exactly every call site this has
    /// (HTTP/gRPC handlers, the reload loop below).
    pub async fn current(&self) -> Arc<CompiledPolicies> {
        self.state.read().await.compiled.clone()
    }

    /// Reloads iff the store's `policy_gen` has advanced past the currently-compiled
    /// `r#gen`; otherwise a cheap no-op (one `policy_gen` read, no recompile, no swap).
    pub async fn reload_if_stale(&self) -> Result<(), AuthzError> {
        let current_gen = self.state.read().await.compiled.r#gen;
        let store_gen = self.policies.policy_gen().await?;
        if store_gen <= current_gen {
            return Ok(());
        }
        self.reload_now().await
    }

    /// Unconditionally reloads and swaps in the new compiled snapshot. Not exposed directly
    /// — [`Self::reload_if_stale`] gates it on gen advance; [`Self::spawn_reload`]'s TTL
    /// branch calls it unconditionally as the max-staleness backstop (AC3).
    async fn reload_now(&self) -> Result<(), AuthzError> {
        let compiled = Self::load_and_compile(self.policies.as_ref(), self.grants.as_ref()).await?;
        self.install_if_newer(compiled).await;
        Ok(())
    }

    /// Installs `compiled` under the write lock iff its `r#gen` is strictly newer than
    /// what's already installed — the monotonic-write guard from the module docs. If it
    /// isn't (this reload lost a race against one that already installed the same or a
    /// newer generation), `compiled` is dropped and `state` is left completely untouched.
    ///
    /// That deliberately includes `loaded_at`: we only refresh the TTL clock when the
    /// snapshot actually advances, never on a discarded stale result. Doing otherwise
    /// would let a losing, no-op reload mask this replica's true staleness from
    /// [`Self::spawn_reload`]'s TTL backstop — the opposite of what that backstop is for.
    async fn install_if_newer(&self, compiled: CompiledPolicies) {
        let mut state = self.state.write().await;
        if compiled.r#gen > state.compiled.r#gen {
            state.compiled = Arc::new(compiled);
            state.loaded_at = Instant::now();
        }
    }

    /// Reads `policy_gen` (the pre-load stamp — see the module docs), then `list_all`s
    /// policies and grants and [`PolicyEngine::compile`]s them. `PolicyEngine::compile`
    /// silently skips a grant whose role template is absent (fail-safe: the grant
    /// contributes no permission) — this logs a `tracing::warn!` naming any such grants so
    /// an operator can tell a "dead" grant apart from one that's merely unused.
    async fn load_and_compile(policies: &dyn PolicyStore, grants: &dyn RoleGrantStore) -> Result<CompiledPolicies, AuthzError> {
        let observed_gen = policies.policy_gen().await?;
        let docs = policies.list_all().await?;
        let all_grants = grants.list_all().await?;

        let mut compiled = PolicyEngine::compile(&docs, &all_grants)?;
        compiled.r#gen = observed_gen;

        let skipped_role_keys: Vec<&str> = all_grants
            .iter()
            .filter(|g| compiled.policy_set.template(&PolicyId::new(&g.role_key)).is_none())
            .map(|g| g.role_key.as_str())
            .collect();
        if !skipped_role_keys.is_empty() {
            tracing::warn!(
                ?skipped_role_keys,
                total_grants = all_grants.len(),
                "policy_snapshot: grant(s) named a role template absent from the compiled policy set — skipped (fail-safe, contributes no permission)"
            );
        }

        Ok(compiled)
    }

    /// Spawns the background reload loop: every `poll` interval, reload if `policy_gen`
    /// advanced (D11); additionally force an unconditional reload once `ttl` has elapsed
    /// since the last successful (re)load, bounding staleness even with no gen movement
    /// visible on this replica (AC3). Exits cleanly once `shutdown` resolves. A reload
    /// error is logged (`tracing::warn!`) and the loop keeps running on the last-good
    /// snapshot — never poisoned by a transient store error.
    pub fn spawn_reload<S>(self: Arc<Self>, ttl: Duration, poll: Duration, shutdown: S) -> JoinHandle<()>
    where
        S: Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(async move {
            tokio::pin!(shutdown);
            loop {
                tokio::select! {
                    () = tokio::time::sleep(poll) => {
                        let ttl_elapsed = self.state.read().await.loaded_at.elapsed() >= ttl;
                        let result = if ttl_elapsed { self.reload_now().await } else { self.reload_if_stale().await };
                        if let Err(err) = result {
                            tracing::warn!(error = %err, "policy_snapshot: reload failed; keeping the last-good snapshot");
                        }
                    }
                    () = &mut shutdown => break,
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use paigasus_iam_core::authz::model::PolicyKind;
    use paigasus_iam_core::{GrantScope, PolicyDocument, PrincipalId, RoleGrant};
    use paigasus_kernel::Prn;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use uuid::Uuid;

    fn principal_prn(n: u128) -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).expect("static test prn parts are valid")
    }

    fn policy_doc(policy_id: &str, kind: PolicyKind, source: &str) -> PolicyDocument {
        let now = Utc::now();
        PolicyDocument {
            policy_id: policy_id.to_string(),
            kind,
            source: source.to_string(),
            description: "test fixture".to_string(),
            system: true,
            created_at: now,
            updated_at: now,
        }
    }

    fn org_admin_template() -> PolicyDocument {
        policy_doc("org_admin", PolicyKind::Template, r#"permit(principal == ?principal, action, resource in ?resource);"#)
    }

    fn role_grant(id: Uuid, role_key: &str) -> RoleGrant {
        RoleGrant {
            id,
            principal: PrincipalId::from_prn(principal_prn(1)),
            role_key: role_key.to_string(),
            scope: GrantScope::Root,
            linked_policy_id: format!("grant:{id}"),
            created_at: Utc::now(),
        }
    }

    fn grant_policy_id(id: Uuid) -> PolicyId {
        PolicyId::new(format!("grant:{id}"))
    }

    /// In-memory `PolicyStore` fake: a `Mutex<Vec<PolicyDocument>>` + an `AtomicU64`
    /// `policy_gen`. `bump_on_next_list` lets a test simulate a concurrent writer's bump
    /// landing exactly inside another caller's `load_and_compile` (see
    /// `reload_captures_gen_before_load_so_a_mid_load_bump_is_not_lost` below).
    struct FakePolicyStore {
        docs: Mutex<Vec<PolicyDocument>>,
        gen_counter: AtomicU64,
        bump_during_list: AtomicU64,
    }

    impl FakePolicyStore {
        fn new(docs: Vec<PolicyDocument>) -> Self {
            Self {
                docs: Mutex::new(docs),
                gen_counter: AtomicU64::new(0),
                bump_during_list: AtomicU64::new(0),
            }
        }

        /// Arms a one-shot side effect: the NEXT `list_all()` call bumps `gen` by `amount`
        /// as well, simulating a second writer's `bump_policy_gen` landing in the window
        /// between a caller's `policy_gen()` read and its `list_all()` read.
        fn bump_on_next_list(&self, amount: u64) {
            self.bump_during_list.store(amount, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl PolicyStore for FakePolicyStore {
        async fn list_all(&self) -> Result<Vec<PolicyDocument>, AuthzError> {
            let pending = self.bump_during_list.swap(0, Ordering::SeqCst);
            if pending > 0 {
                self.gen_counter.fetch_add(pending, Ordering::SeqCst);
            }
            Ok(self.docs.lock().unwrap().clone())
        }

        async fn put(&self, _doc: &PolicyDocument) -> Result<(), AuthzError> {
            unimplemented!("policy_snapshot tests never write through PolicyStore::put")
        }

        async fn delete(&self, _policy_id: &str) -> Result<(), AuthzError> {
            unimplemented!("policy_snapshot tests never write through PolicyStore::delete")
        }

        async fn policy_gen(&self) -> Result<u64, AuthzError> {
            Ok(self.gen_counter.load(Ordering::SeqCst))
        }

        async fn bump_policy_gen(&self) -> Result<u64, AuthzError> {
            Ok(self.gen_counter.fetch_add(1, Ordering::SeqCst) + 1)
        }
    }

    /// In-memory `RoleGrantStore` fake: a plain `Mutex<Vec<RoleGrant>>`. Bumping
    /// `policy_gen` after a `grant()` call is the TEST's job here — `RoleGrantStore` itself
    /// never touches the generation counter (only `PolicyStore::bump_policy_gen` does); the
    /// real `PgRoleGrantStore` adapter composes both via a shared `Generations` handle,
    /// which these two independent fakes don't need to replicate for these unit tests.
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

        async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(self.grants.lock().unwrap().clone())
        }

        async fn list_by_principal(&self, _p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError> {
            unimplemented!("policy_snapshot tests never query by principal")
        }

        async fn find(&self, id: Uuid) -> Result<Option<RoleGrant>, AuthzError> {
            Ok(self.grants.lock().unwrap().iter().find(|g| g.id == id).cloned())
        }
    }

    #[tokio::test]
    async fn initial_snapshot_compiles_seeded_policies_and_grants() {
        let base = policy_doc("base-allow-list", PolicyKind::Static, r#"permit(principal, action, resource);"#);
        let grant_id = Uuid::from_u128(100);
        let grant = role_grant(grant_id, "org_admin");

        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(vec![org_admin_template(), base]));
        let grants: Arc<dyn RoleGrantStore> = Arc::new(FakeRoleGrantStore::new(vec![grant]));

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        let compiled = snapshot.current().await;

        assert!(compiled.policy_set.policy(&PolicyId::new("base-allow-list")).is_some());
        assert!(compiled.policy_set.policy(&grant_policy_id(grant_id)).is_some());
        assert_eq!(compiled.r#gen, 0);
    }

    #[tokio::test]
    async fn reload_if_stale_recompiles_after_a_grant_and_gen_bump() {
        let policies_store = Arc::new(FakePolicyStore::new(vec![org_admin_template()]));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        let before = snapshot.current().await;
        assert_eq!(before.r#gen, 0);

        let grant_id = Uuid::from_u128(200);
        let grant = role_grant(grant_id, "org_admin");
        grants_store.grant(&grant).await.unwrap();
        policies_store.bump_policy_gen().await.unwrap();

        snapshot.reload_if_stale().await.expect("reload succeeds");
        let after = snapshot.current().await;

        assert!(!Arc::ptr_eq(&before, &after), "a stale reload must swap in a new compiled Arc");
        assert_eq!(after.r#gen, 1);
        assert!(before.policy_set.policy(&grant_policy_id(grant_id)).is_none());
        assert!(after.policy_set.policy(&grant_policy_id(grant_id)).is_some());
    }

    #[tokio::test]
    async fn reload_if_stale_is_a_no_op_without_a_gen_bump() {
        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(vec![org_admin_template()]));
        let grants: Arc<dyn RoleGrantStore> = Arc::new(FakeRoleGrantStore::new(vec![]));

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        let before = snapshot.current().await;

        snapshot.reload_if_stale().await.expect("no-op reload still returns Ok");
        let after = snapshot.current().await;

        assert!(Arc::ptr_eq(&before, &after), "no gen bump means no swap");
        assert_eq!(after.r#gen, before.r#gen);
    }

    /// Proves the "no lost update" semantics from the module docs: `load_and_compile`
    /// stamps the compiled `r#gen` with the value observed BEFORE its `list_all()` calls,
    /// not one read after they complete. A `FakePolicyStore::bump_on_next_list` simulates a
    /// second writer's bump landing exactly inside that window; the first reload must
    /// stamp the OLDER (pre-load) gen, and a follow-up `reload_if_stale` must still detect
    /// staleness and pick up the bump that landed mid-load.
    #[tokio::test]
    async fn reload_captures_gen_before_load_so_a_mid_load_bump_is_not_lost() {
        let policies_store = Arc::new(FakePolicyStore::new(vec![org_admin_template()]));
        let grants: Arc<dyn RoleGrantStore> = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        assert_eq!(snapshot.current().await.r#gen, 0);

        // An ordinary bump to gen 1 (some other change), then arm the "concurrent writer"
        // hook: the NEXT `list_all()` call — which happens inside the reload this triggers
        // — bumps gen again, from 1 to 2, simulating a second writer's change landing in
        // the gap between `reload_if_stale`'s gen read and its `list_all()` reads.
        policies_store.bump_policy_gen().await.unwrap();
        policies_store.bump_on_next_list(1);

        snapshot.reload_if_stale().await.expect("reload succeeds");
        let first_reload = snapshot.current().await;
        assert_eq!(first_reload.r#gen, 1, "must stamp the pre-load gen, not a gen read after the concurrent bump lands mid-load");
        assert_eq!(
            policies_store.policy_gen().await.unwrap(),
            2,
            "the simulated concurrent writer's bump did land, just after our gen read"
        );

        snapshot.reload_if_stale().await.expect("second reload succeeds");
        let second_reload = snapshot.current().await;
        assert_eq!(
            second_reload.r#gen, 2,
            "the follow-up reload must still see itself as stale and pick up the bump that would otherwise be lost"
        );
        assert!(!Arc::ptr_eq(&first_reload, &second_reload));
    }

    /// Proves the monotonic-write guard from the module docs: an older-gen compiled set
    /// must never overwrite a newer one already installed, even if the older reload's
    /// writer is the one that reaches the write lock LAST.
    ///
    /// Two real, concurrently-launched `reload_now`s racing for the write lock in that
    /// exact order is timing-dependent to pin deterministically; instead this drives the
    /// guard at the swap boundary directly (mirroring the mid-load-bump test's technique
    /// of arming a deterministic race window rather than depending on scheduler luck):
    /// compile two genuinely distinct `CompiledPolicies` — an "older" one from gen 1, a
    /// "newer" one from gen 2 — via the same private `load_and_compile` every real reload
    /// uses, then feed them to `install_if_newer` in the exact order a losing race would
    /// produce (newer installs first, older arrives and attempts to install second).
    #[tokio::test]
    async fn install_if_newer_rejects_an_older_gen_arriving_after_a_newer_one_is_installed() {
        let policies_store = Arc::new(FakePolicyStore::new(vec![org_admin_template()]));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();

        let snapshot = PolicySnapshot::new(policies.clone(), grants.clone()).await.expect("build succeeds");
        assert_eq!(snapshot.current().await.r#gen, 0);

        policies_store.bump_policy_gen().await.unwrap();
        let older = PolicySnapshot::load_and_compile(policies.as_ref(), grants.as_ref()).await.expect("compiles at gen 1");
        assert_eq!(older.r#gen, 1);

        policies_store.bump_policy_gen().await.unwrap();
        let newer = PolicySnapshot::load_and_compile(policies.as_ref(), grants.as_ref()).await.expect("compiles at gen 2");
        assert_eq!(newer.r#gen, 2);

        // The reload that observed the NEWER gen wins the race to the write lock first.
        snapshot.install_if_newer(newer).await;
        assert_eq!(snapshot.current().await.r#gen, 2);
        let installed_at_gen_2 = snapshot.current().await;

        // The reload that observed the OLDER gen arrives second — the guard must reject
        // it rather than regress the installed snapshot.
        snapshot.install_if_newer(older).await;
        let after = snapshot.current().await;

        assert_eq!(after.r#gen, 2, "an older-gen compiled set must never overwrite a newer installed one");
        assert!(Arc::ptr_eq(&installed_at_gen_2, &after), "the rejected older-gen install must leave the installed Arc untouched");
    }
}
