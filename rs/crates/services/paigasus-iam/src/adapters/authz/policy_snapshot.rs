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
//! when the store's `policy_gen` DIFFERS from the currently-compiled `r#gen`;
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
//! **The generation counter is advisory, never load-bearing** (SMA-470 D-A/D-C). It lives in
//! Redis; the policies and grants themselves live in Postgres. So an unreadable `policy_gen`
//! must not stop a reload: [`PolicySnapshot::load_and_compile`] degrades a failed read to a
//! PROVISIONAL stamp — the caller's last-known generation, carried over — and compiles from
//! Postgres regardless. And because a counter that went BACKWARDS means Redis was reset
//! (`Generations::read` maps a missing key to `0`), `reload_if_stale` reloads on inequality
//! rather than on advance; requiring an advance is what let a `FLUSHALL` freeze the snapshot
//! until process restart.
//!
//! **Two guards keep that from becoming a recompile-per-decision storm.** A provisional stamp
//! was never observed, so comparing it against a live counter read would report permanent
//! staleness — every decision would trigger a full recompile. `reload_if_stale` therefore
//! suppresses request-driven reloads entirely while `SnapshotState::stamp_trusted` is false
//! and leaves refreshing to the TTL backstop, which is unconditional and so always converges
//! (in at most `ttl + poll`). Independently, a single-flight `reload_gate` caps concurrent
//! reloads at one — closing a herd that predates SMA-470, where every in-flight request
//! observing the same bump ran its own full `list_all` + Cedar compile.
//!
//! **Never poisons on a transient error:** every reload path (a manual call, or one iteration
//! of `spawn_reload`'s loop) only swaps in a new compiled snapshot on `Ok` from
//! `load_and_compile`; an `Err` (a transient Postgres hiccup, say) is logged and the previous
//! known-good snapshot keeps serving decisions.
//!
//! **Monotonic-write guard:** [`PolicySnapshot::reload_now`] does not swap in a freshly
//! compiled set unconditionally — [`PolicySnapshot::install_if_fresher`] installs it only if
//! it comes from a load that started strictly later than the one currently installed (SMA-470
//! D-B). Two reloads can race for the write lock and finish out of start order (the one that
//! started EARLIER can acquire the lock AFTER one that started later, e.g. if it was already
//! mid-compile when the second reload started); an unconditional swap would then regress the
//! in-memory snapshot to a stale policy set — transiently un-revoking a revoked grant, say —
//! until the next reload self-heals it. That self-heal window is small but security-adjacent,
//! so the swap itself is guarded rather than relied upon to self-correct.
//!
//! That guard used to compare `CompiledPolicies::r#gen` directly, but `r#gen` is a
//! Redis-sourced counter that can stall, reset, or fail to advance across a reload — exactly
//! the SMA-470 scenario, where a revoke's `policy_gen` bump is lost during a Redis outage.
//! Requiring `r#gen` to strictly advance meant [`PolicySnapshot::spawn_reload`]'s TTL
//! backstop — the mechanism this module docs above describe as bounding staleness even with
//! no visible gen movement — could recompile at the SAME gen forever and never install the
//! result, so the swallowed revoke was never picked up. The guard now orders installs on
//! `load_seq`, a process-local counter claimed once per load regardless of what `policy_gen`
//! reports, so a same-gen recompile can still install.

use cedar_policy::PolicyId;
use paigasus_iam_core::authz::engine::{CompiledPolicies, PolicyEngine};
use paigasus_iam_core::{AuthzError, PolicyStore, RoleGrantStore};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

/// The mutable half of [`PolicySnapshot`]'s state: the current compiled policy set plus
/// when it was last (re)loaded — bundled behind one lock so a reload's swap and its
/// TTL-clock reset are always observed together by [`PolicySnapshot::spawn_reload`]'s TTL
/// check.
struct SnapshotState {
    compiled: Arc<CompiledPolicies>,
    loaded_at: Instant,
    /// The `load_seq` of the load whose result is currently installed (SMA-470). The
    /// monotonic-write guard compares THIS, not `compiled.r#gen`: the generation is a
    /// Redis-sourced counter that can stall, reset, or repeat, so it cannot order two loads
    /// — and requiring it to strictly advance is what made the TTL backstop unable to install
    /// a same-generation recompile (defect D-B).
    installed_seq: u64,
    /// `false` when the installed `compiled.r#gen` is a PROVISIONAL stamp — the counter was
    /// unreadable at load time, so the value was carried over rather than observed. The
    /// compiled policy set itself is still fresh (it came from Postgres); only the stamp is
    /// a guess, so it must not be compared against a live counter read (SMA-470 §3.4).
    stamp_trusted: bool,
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
    /// Hands out a strictly increasing token per load. Claimed immediately BEFORE the first
    /// Postgres read (see [`Self::load_and_compile`]) so it orders loads by when they read
    /// their data — which is the property the monotonic-write guard actually needs.
    load_seq: AtomicU64,
    /// Single-flight gate: at most one reload runs at a time. [`Self::reload_if_stale`]
    /// `try_lock`s and gives up immediately if a reload is already in flight, deciding against
    /// the current snapshot instead. Without this, every in-flight request observing the same
    /// staleness runs its own full recompile (a pre-existing herd, SMA-470 §3.4 guard 1).
    ///
    /// [`Self::reload_now`] deliberately takes the SAME gate with a blocking `lock` rather
    /// than a `try_lock`: it is the TTL backstop, the only thing that can refresh a
    /// provisional stamp, so it must never be skipped merely because a request-driven reload
    /// happened to be in flight. `tokio::sync::Mutex` is FIFO-fair and `reload_if_stale` never
    /// queues on it, so the backstop waits for at most one in-flight reload.
    ///
    /// Both callers hold the gate across `load_and_compile` AND `install_if_fresher`, so
    /// in-process loads claim their `load_seq` and install in the same order — the
    /// monotonic-write guard is left as a defence for the direct (test/`new`) call paths.
    reload_gate: AsyncMutex<()>,
}

impl PolicySnapshot {
    /// Loads every policy/template + role grant, compiles them, and stamps the result with
    /// the `policy_gen` observed at the START of this load (see the module docs' "no
    /// lost update" note). Builds the `load_seq` counter first so this initial load claims
    /// seq `1` through the exact same [`Self::load_and_compile`] path every later reload
    /// uses — no throwaway empty compile, no double install.
    pub async fn new(policies: Arc<dyn PolicyStore>, grants: Arc<dyn RoleGrantStore>) -> Result<Self, AuthzError> {
        let load_seq = AtomicU64::new(0);
        // `fallback_gen = 0`: nothing is installed yet, so there is no last-known generation
        // to carry over. Booting with an unreadable counter therefore stamps 0 provisionally
        // — which is exactly right, because 0 is also what a never-written counter reads as.
        let (compiled, seq, trusted) = Self::load_and_compile(policies.as_ref(), grants.as_ref(), &load_seq, 0).await?;
        Ok(Self {
            policies,
            grants,
            state: RwLock::new(SnapshotState {
                compiled: Arc::new(compiled),
                loaded_at: Instant::now(),
                installed_seq: seq,
                stamp_trusted: trusted,
            }),
            load_seq,
            reload_gate: AsyncMutex::new(()),
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

    /// Reloads iff the store's `policy_gen` differs from the currently-compiled `r#gen`;
    /// otherwise a cheap no-op (one `policy_gen` read, no recompile, no swap).
    ///
    /// **Inequality, not advance** (SMA-470 D-C): a counter that went BACKWARDS means Redis
    /// was reset (`Generations::read` maps a missing key to 0), so the installed stamp is
    /// meaningless and re-stamping is correct. It settles after one reload, because the new
    /// stamp then equals the store's value.
    ///
    /// Two guards keep that from becoming a recompile-per-decision storm: a provisional stamp
    /// (the counter was unreadable at the last load) suppresses request-driven reloads
    /// entirely, and the single-flight gate caps concurrent reloads at one.
    ///
    /// Still returns `Err` when `policy_gen()` itself errors — every caller
    /// (`CedarAuthorizer::is_authorized`, [`Self::spawn_reload`]) logs and swallows it, so a
    /// Redis outage degrades a decision to "evaluated against the last-known-good snapshot",
    /// never to a failure (fail-open, D1).
    pub async fn reload_if_stale(&self) -> Result<(), AuthzError> {
        // Scoped so the read guard is released BEFORE `install_if_fresher` takes the write
        // guard below — `tokio::sync::RwLock` is not reentrant, and a writer queued in between
        // would deadlock a read-then-write held across the same task.
        let (current_gen, trusted) = {
            let state = self.state.read().await;
            (state.compiled.r#gen, state.stamp_trusted)
        };
        if !trusted {
            // The installed stamp is a carried-over guess; comparing it to a live read would
            // report permanent staleness. The TTL backstop owns refreshing until the counter
            // is readable again.
            return Ok(());
        }
        let store_gen = self.policies.policy_gen().await?;
        if store_gen == current_gen {
            return Ok(());
        }
        // Give up immediately if a reload is already in flight rather than queueing behind it:
        // this call's decision is served from the current snapshot either way.
        let Ok(_guard) = self.reload_gate.try_lock() else {
            return Ok(());
        };
        let (compiled, seq, stamp_trusted) = Self::load_and_compile(self.policies.as_ref(), self.grants.as_ref(), &self.load_seq, current_gen).await?;
        self.install_if_fresher(compiled, seq, stamp_trusted).await;
        Ok(())
    }

    /// Unconditionally reloads and swaps in the new compiled snapshot. Not exposed directly —
    /// [`Self::spawn_reload`]'s TTL branch calls it as the max-staleness backstop (AC3), and it
    /// is the ONLY path that can refresh a provisional generation stamp back to authoritative.
    ///
    /// Takes the single-flight gate with a BLOCKING `lock`, unlike [`Self::reload_if_stale`]'s
    /// `try_lock`: skipping the backstop because some request-driven reload happened to be
    /// mid-flight would leave the snapshot suppressed for another whole `ttl`
    /// (see [`Self::reload_gate`]).
    ///
    /// [`Self::reload_if_stale`] deliberately does NOT delegate here — it inlines the same
    /// load-and-install because it already holds `reload_gate`, and `tokio::sync::Mutex` is not
    /// reentrant, so calling this from under the gate would deadlock the reload path outright.
    async fn reload_now(&self) -> Result<(), AuthzError> {
        let _guard = self.reload_gate.lock().await;
        // The read guard is a statement-scoped temporary — dropped at the `;`, well before
        // `install_if_fresher` takes the write lock.
        let fallback_gen = self.state.read().await.compiled.r#gen;
        let (compiled, seq, trusted) = Self::load_and_compile(self.policies.as_ref(), self.grants.as_ref(), &self.load_seq, fallback_gen).await?;
        self.install_if_fresher(compiled, seq, trusted).await;
        Ok(())
    }

    /// Installs `compiled` under the write lock iff `seq` is fresher than the installed load's
    /// — the monotonic-write guard. If it isn't (this load lost a race against one that
    /// started later), `compiled` is dropped and `state` is left completely untouched.
    ///
    /// `loaded_at` moves only on an actual install, so a losing no-op reload can never mask
    /// this replica's true staleness from [`Self::spawn_reload`]'s TTL backstop.
    ///
    /// `trusted` records whether this load actually OBSERVED its generation or carried one
    /// over (SMA-470 §3.4). This is also the only place that sees the old and the new flag
    /// together, so it owns the operator-visible transition logging — exactly one line per
    /// state change, rather than one per reload attempt.
    async fn install_if_fresher(&self, compiled: CompiledPolicies, seq: u64, trusted: bool) {
        let mut state = self.state.write().await;
        if seq > state.installed_seq {
            match (state.stamp_trusted, trusted) {
                (true, false) => tracing::warn!("policy_snapshot: policy_gen unreadable — serving a Postgres-compiled snapshot on a provisional generation stamp (fail-open, SMA-470)"),
                (false, true) => tracing::info!("policy_snapshot: policy_gen readable again — the generation stamp is authoritative"),
                _ => {}
            }
            state.stamp_trusted = trusted;
            state.compiled = Arc::new(compiled);
            state.loaded_at = Instant::now();
            state.installed_seq = seq;
        } else {
            tracing::debug!(rejected_seq = seq, installed_seq = state.installed_seq, "policy_snapshot: discarding an out-of-order reload");
        }
    }

    /// Reads `policy_gen` (the pre-load stamp — see the module docs), then `list_all`s
    /// policies and grants and [`PolicyEngine::compile`]s them. `PolicyEngine::compile`
    /// silently skips a grant whose role template is absent (fail-safe: the grant
    /// contributes no permission) — this logs a `tracing::warn!` naming any such grants so
    /// an operator can tell a "dead" grant apart from one that's merely unused.
    ///
    /// Claims `load_seq`'s next token immediately before the first store read, so it orders
    /// loads by when they read their data — which is the property the monotonic-write guard
    /// in [`Self::install_if_fresher`] needs, and the generation counter cannot provide
    /// (SMA-470 D-B). Claiming it any earlier would put the `policy_gen` read — the step
    /// that stalls during exactly the Redis outage this guards against — between the token
    /// and the data it labels.
    ///
    /// **A failed `policy_gen` read never fails the load** (SMA-470 D-A). The counter lives in
    /// Redis; the policy set lives in Postgres. Propagating the error meant a Redis outage
    /// froze the snapshot for as long as Redis was down — including the TTL backstop, so a
    /// revoke committed during the outage was never picked up. Instead the load falls back to
    /// `fallback_gen` (the caller's last-known generation) and reports `trusted = false`, so
    /// the caller knows the stamp it installs is a carry-over rather than an observation.
    ///
    /// The per-attempt line is `debug!`, not `warn!`: at the default `refresh_interval_secs`
    /// of 1 this runs once a second per replica for the whole outage. The one-per-transition
    /// `warn!`/`info!` lives in [`Self::install_if_fresher`], the only place that sees both
    /// the old and the new flag.
    async fn load_and_compile(policies: &dyn PolicyStore, grants: &dyn RoleGrantStore, load_seq: &AtomicU64, fallback_gen: u64) -> Result<(CompiledPolicies, u64, bool), AuthzError> {
        let seq = load_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let (observed_gen, trusted) = match policies.policy_gen().await {
            Ok(g) => (g, true),
            Err(err) => {
                tracing::debug!(error = %err, "policy_snapshot: policy_gen unreadable — compiling from Postgres anyway and stamping the last-known generation (fail-open, SMA-470)");
                (fallback_gen, false)
            }
        };
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

        Ok((compiled, seq, trusted))
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
    use paigasus_iam_core::{GrantScope, PolicyDocument, PrincipalId, PutOutcome, RoleGrant, Transaction};
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

        async fn put_in(&self, _tx: &dyn Transaction, _doc: &PolicyDocument) -> Result<PutOutcome, AuthzError> {
            unimplemented!("policy_snapshot tests never write through PolicyStore::put_in")
        }

        async fn delete_in(&self, _tx: &dyn Transaction, _policy_id: &str) -> Result<bool, AuthzError> {
            unimplemented!("policy_snapshot tests never write through PolicyStore::delete_in")
        }

        async fn policy_gen(&self) -> Result<u64, AuthzError> {
            Ok(self.gen_counter.load(Ordering::SeqCst))
        }

        async fn bump_policy_gen(&self) -> Result<u64, AuthzError> {
            Ok(self.gen_counter.fetch_add(1, Ordering::SeqCst) + 1)
        }
    }

    /// A `PolicyStore` fake whose `policy_gen()` returns `Ok(first)` for the first `ok_calls`
    /// calls and errors afterwards — simulating a Redis-backed counter that was readable at
    /// construction and then went away. Erroring from the very first call would make
    /// "stamped the last-known gen" and "stamped 0" indistinguishable.
    ///
    /// `heal` re-arms it with a NEW value, which is how the flapping case (`policy_gen`
    /// readable again, reporting a generation that differs from the provisional stamp) is
    /// reached — see `a_provisional_stamp_suppresses_reloads_even_once_the_counter_returns`.
    struct FlakyGenPolicyStore {
        docs: Mutex<Vec<PolicyDocument>>,
        first: AtomicU64,
        ok_calls: AtomicU64,
    }

    impl FlakyGenPolicyStore {
        fn new(docs: Vec<PolicyDocument>, first: u64, ok_calls: u64) -> Self {
            Self {
                docs: Mutex::new(docs),
                first: AtomicU64::new(first),
                ok_calls: AtomicU64::new(ok_calls),
            }
        }

        /// Redis comes back, reporting `value` for the next `ok_calls` reads.
        fn heal(&self, value: u64, ok_calls: u64) {
            self.first.store(value, Ordering::SeqCst);
            self.ok_calls.store(ok_calls, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl PolicyStore for FlakyGenPolicyStore {
        async fn list_all(&self) -> Result<Vec<PolicyDocument>, AuthzError> {
            Ok(self.docs.lock().unwrap().clone())
        }

        async fn put(&self, _doc: &PolicyDocument) -> Result<(), AuthzError> {
            unimplemented!("never written in these tests")
        }

        async fn delete(&self, _policy_id: &str) -> Result<(), AuthzError> {
            unimplemented!("never written in these tests")
        }

        async fn put_in(&self, _tx: &dyn Transaction, _doc: &PolicyDocument) -> Result<PutOutcome, AuthzError> {
            unimplemented!("never written in these tests")
        }

        async fn delete_in(&self, _tx: &dyn Transaction, _policy_id: &str) -> Result<bool, AuthzError> {
            unimplemented!("never written in these tests")
        }

        async fn policy_gen(&self) -> Result<u64, AuthzError> {
            if self.ok_calls.load(Ordering::SeqCst) > 0 {
                self.ok_calls.fetch_sub(1, Ordering::SeqCst);
                Ok(self.first.load(Ordering::SeqCst))
            } else {
                Err(AuthzError::Backend("simulated generations-redis outage".into()))
            }
        }

        async fn bump_policy_gen(&self) -> Result<u64, AuthzError> {
            Err(AuthzError::Backend("simulated generations-redis outage".into()))
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

    /// SMA-470 D-B: the TTL backstop (`spawn_reload`'s `ttl_elapsed` branch calls
    /// `reload_now` unconditionally) must actually INSTALL a recompile when the generation
    /// counter has not moved — the exact case the module docs say the backstop exists for.
    /// Before this fix `install_if_newer` required a strictly greater gen, so the backstop
    /// recompiled and discarded on every poll, forever, and a grant/revoke whose bump was
    /// swallowed was never picked up.
    #[tokio::test]
    async fn ttl_backstop_installs_a_same_gen_recompile() {
        let policies_store = Arc::new(FakePolicyStore::new(vec![org_admin_template()]));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        assert_eq!(snapshot.current().await.r#gen, 0);

        // A grant lands in Postgres but its `policy_gen` bump is LOST (Redis down) — SMA-470's
        // scenario, in the grant direction so the effect is easy to observe.
        let grant_id = Uuid::from_u128(700);
        grants_store.grant(&role_grant(grant_id, "org_admin")).await.unwrap();

        snapshot.reload_now().await.expect("reload_now succeeds");

        let after = snapshot.current().await;
        assert!(
            after.policy_set.policy(&grant_policy_id(grant_id)).is_some(),
            "the TTL backstop must install a same-gen recompile — otherwise a swallowed bump is never recovered"
        );
    }

    /// SMA-470: the backstop must also reset the TTL clock when it installs, so
    /// `spawn_reload` stops re-entering the `ttl_elapsed` branch on every poll.
    #[tokio::test]
    async fn a_same_gen_backstop_install_resets_the_ttl_clock() {
        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(vec![org_admin_template()]));
        let grants: Arc<dyn RoleGrantStore> = Arc::new(FakeRoleGrantStore::new(vec![]));

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        let before = snapshot.state.read().await.loaded_at;

        tokio::time::sleep(Duration::from_millis(5)).await;
        snapshot.reload_now().await.expect("reload_now succeeds");

        assert!(snapshot.state.read().await.loaded_at > before, "an installed recompile must refresh loaded_at");
    }

    /// SMA-470: the monotonic-write guard's intent is preserved, now expressed over the load
    /// sequence rather than the generation — a load that STARTED earlier must never overwrite
    /// one that started later, even if it reaches the write lock last. This replaces the old
    /// `install_if_newer_rejects_an_older_gen_arriving_after_a_newer_one_is_installed`.
    ///
    /// Deliberately does NOT bump `policy_gen` between the two loads — both `older` and
    /// `newer` observe the SAME generation (0), the swallowed-bump scenario SMA-470 is about.
    /// This is the case where the two orderings diverge: a gen-based guard sees `newer.r#gen
    /// == older.r#gen == 0`, so `newer.r#gen > 0` is false and it would reject the newer load
    /// outright, never installing the grant at all — the guard fails silently rather than
    /// merely picking the wrong winner. The seq-based guard installs `newer` correctly (`3 >
    /// 1`) and then correctly rejects `older` arriving second (`2 > 3` is false), which is
    /// what this test proves.
    #[tokio::test]
    async fn install_if_fresher_rejects_an_older_load_arriving_after_a_newer_one() {
        let policies_store = Arc::new(FakePolicyStore::new(vec![org_admin_template()]));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");

        // The "older" load starts first and so claims the lower seq — still at gen 0, no
        // grant yet.
        let (older, older_seq, older_trusted) = PolicySnapshot::load_and_compile(policies_store.as_ref(), grants_store.as_ref(), &snapshot.load_seq, 0)
            .await
            .expect("compiles");

        // A grant lands, but its `policy_gen` bump is LOST (Redis down) — the "newer" load
        // starts after it and claims the higher seq, still observing gen 0.
        let grant_id = Uuid::from_u128(900);
        grants_store.grant(&role_grant(grant_id, "org_admin")).await.unwrap();
        let (newer, newer_seq, newer_trusted) = PolicySnapshot::load_and_compile(policies_store.as_ref(), grants_store.as_ref(), &snapshot.load_seq, 0)
            .await
            .expect("compiles");
        assert_eq!(newer.r#gen, older.r#gen, "both loads must observe the same generation — this is the case seq-ordering exists for");
        assert!(newer_seq > older_seq);

        // The newer load wins the race to the write lock.
        snapshot.install_if_fresher(newer, newer_seq, newer_trusted).await;
        let installed = snapshot.current().await;
        assert!(installed.policy_set.policy(&grant_policy_id(grant_id)).is_some());

        // The older load arrives second — the guard must reject it rather than regress.
        snapshot.install_if_fresher(older, older_seq, older_trusted).await;
        let after = snapshot.current().await;

        assert!(
            after.policy_set.policy(&grant_policy_id(grant_id)).is_some(),
            "an older-seq load must never overwrite a newer installed one"
        );
        assert!(Arc::ptr_eq(&installed, &after), "the rejected install must leave the installed Arc untouched");
    }

    /// SMA-470 D-A: a Redis outage must not stop the snapshot reloading. Policies and grants
    /// live in POSTGRES; only the generation stamp comes from Redis, so an unreadable counter
    /// must degrade to a provisional stamp, never to a failed reload.
    #[tokio::test]
    async fn reload_survives_an_unreadable_policy_gen() {
        // One successful read (consumed by `new()`), then the counter goes away.
        let policies_store = Arc::new(FlakyGenPolicyStore::new(vec![org_admin_template()], 5, 1));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        assert_eq!(snapshot.current().await.r#gen, 5);

        // A grant lands while Redis is down — its bump is swallowed.
        let grant_id = Uuid::from_u128(710);
        grants_store.grant(&role_grant(grant_id, "org_admin")).await.unwrap();

        // The TTL backstop still installs fresh Postgres data.
        snapshot.reload_now().await.expect("a Redis outage must not fail the reload");

        let after = snapshot.current().await;
        assert!(
            after.policy_set.policy(&grant_policy_id(grant_id)).is_some(),
            "must compile fresh Postgres data with the counter unreadable"
        );
        assert_eq!(after.r#gen, 5, "must stamp the LAST-KNOWN gen, not 0");
    }

    /// SMA-470 D-C: after a Redis data loss the counter reads back 0 (`Generations::read` maps
    /// a missing key to 0). `reload_if_stale` must treat any INEQUALITY as staleness — a
    /// regression means the counter was reset and our stamp is meaningless. Before this fix
    /// `0 <= N` short-circuited and the backstop's `0 > N` install was rejected, freezing the
    /// snapshot until process restart.
    #[tokio::test]
    async fn gen_regression_after_a_redis_flush_still_reloads() {
        let policies_store = Arc::new(FakePolicyStore::new(vec![org_admin_template()]));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();

        for _ in 0..3 {
            policies_store.bump_policy_gen().await.unwrap();
        }
        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        assert_eq!(snapshot.current().await.r#gen, 3);

        // Redis is flushed: the counter restarts from 0, then a later grant bumps it to 1.
        policies_store.gen_counter.store(0, Ordering::SeqCst);
        let grant_id = Uuid::from_u128(800);
        grants_store.grant(&role_grant(grant_id, "org_admin")).await.unwrap();
        policies_store.bump_policy_gen().await.unwrap();

        snapshot.reload_if_stale().await.expect("reload succeeds");

        let after = snapshot.current().await;
        assert!(after.policy_set.policy(&grant_policy_id(grant_id)).is_some(), "a reset counter must not freeze the snapshot");
        assert_eq!(after.r#gen, 1, "the snapshot re-stamps to the store's post-reset value");

        // And it settles: equal gens are a cheap no-op, not a reload loop.
        let installed = snapshot.current().await;
        snapshot.reload_if_stale().await.expect("second call succeeds");
        assert!(Arc::ptr_eq(&installed, &snapshot.current().await), "equal gens must not reload");
    }

    /// SMA-470 §3.4 guard 2: while the stamp is PROVISIONAL (the counter was unreadable at the
    /// last load), request-driven reloads are suppressed and refreshing is left to the TTL
    /// backstop. Without this, a flapping Redis — `reload_if_stale`'s read succeeding with N
    /// while `load_and_compile`'s read fails and stamps M != N — yields permanent inequality,
    /// i.e. a full policy recompile on EVERY authorization decision, indefinitely.
    #[tokio::test]
    async fn a_provisional_stamp_suppresses_request_driven_reloads() {
        // `new()` consumes one Ok(5); `reload_now` below then fails its gen read and stamps
        // provisionally; the next `policy_gen()` (from `reload_if_stale`) also errors.
        let policies_store = Arc::new(FlakyGenPolicyStore::new(vec![org_admin_template()], 5, 1));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        snapshot.reload_now().await.expect("backstop reload succeeds with a provisional stamp");
        assert!(!snapshot.state.read().await.stamp_trusted, "the stamp is provisional after an unreadable gen read");

        let installed = snapshot.current().await;
        snapshot.reload_if_stale().await.expect("must not error");
        assert!(
            Arc::ptr_eq(&installed, &snapshot.current().await),
            "a provisional stamp must suppress request-driven reloads — the backstop owns refreshing"
        );
    }

    /// The FLAPPING half of §3.4 guard 2, and the reason suppressing is safe.
    ///
    /// `a_provisional_stamp_suppresses_request_driven_reloads` above only ever sees a
    /// permanently-down counter, so it would also pass against an implementation that merely
    /// swallowed `reload_if_stale`'s `policy_gen` error. The scenario the guard actually
    /// exists for is Redis coming BACK with a value that differs from the provisional stamp:
    /// there, an unguarded `reload_if_stale` sees genuine inequality and recompiles the whole
    /// policy set on every single decision.
    ///
    /// Phase 2 is the safety argument for suppressing at all: the TTL backstop is
    /// unconditional and takes the reload gate with a blocking `lock`, so it always runs, and
    /// the first backstop pass that observes a readable counter re-trusts the stamp and hands
    /// request-driven reloads back. A provisional stamp is therefore bounded by one `ttl`, not
    /// permanent.
    #[tokio::test]
    async fn a_flapping_counter_is_suppressed_until_the_backstop_re_trusts_the_stamp() {
        let policies_store = Arc::new(FlakyGenPolicyStore::new(vec![org_admin_template()], 5, 1));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();

        // gen 5, trusted → the counter goes away → the backstop stamps 5 provisionally.
        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        snapshot.reload_now().await.expect("backstop reload succeeds with a provisional stamp");
        assert!(!snapshot.state.read().await.stamp_trusted);

        // Phase 1 — Redis is back, reporting 9. That differs from the provisional 5, so an
        // unguarded `reload_if_stale` would recompile here (and on every later decision).
        policies_store.heal(9, 100);
        let grant_id = Uuid::from_u128(720);
        grants_store.grant(&role_grant(grant_id, "org_admin")).await.unwrap();

        let installed = snapshot.current().await;
        snapshot.reload_if_stale().await.expect("must not error");
        assert!(
            Arc::ptr_eq(&installed, &snapshot.current().await),
            "a readable-but-differing counter must not reload while the stamp is provisional — that is the recompile-per-decision storm"
        );

        // Phase 2 — the backstop runs, observes the counter, and re-trusts the stamp.
        snapshot.reload_now().await.expect("the backstop always runs");
        let after = snapshot.current().await;
        assert!(snapshot.state.read().await.stamp_trusted, "an observed gen must restore the stamp to authoritative");
        assert_eq!(after.r#gen, 9, "the backstop re-stamps to the counter's live value");
        assert!(
            after.policy_set.policy(&grant_policy_id(grant_id)).is_some(),
            "and installs the Postgres data written during the outage"
        );

        // Request-driven reloads are live again, and settled: equal gens are a no-op.
        snapshot.reload_if_stale().await.expect("reload_if_stale is live again");
        assert!(Arc::ptr_eq(&after, &snapshot.current().await), "a re-trusted stamp equal to the store's gen must not reload");
    }
}
