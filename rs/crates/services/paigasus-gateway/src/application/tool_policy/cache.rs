// SPDX-License-Identifier: Apache-2.0

//! The per-tenant tool-policy cache (SMA-752 § 6.3.3).
//!
//! One entry for the floor and one per organization. A load runs in a task that the cache spawns,
//! so a waiter that goes away (timeout, client disconnect) does not cancel it. Every waiter waits
//! at most `load_timeout`. The cache hands out `Arc`s: an invalidation or a refresh swaps the
//! entry, and it never changes a set that a request already holds (D30).
//!
//! Logs hold a code and, for a tenant, the organization id. They never hold the text of an error:
//! a `CompileError` message can hold raw policy names.
//!
//! SMA-753 spec P6: the cache also maps and validates the history conditions at load. The floor
//! entry holds the floor conditions unbound, because a digest literal depends on the org. A
//! compiled tenant set holds its `HistoryPolicy`: the floor and the tenant conditions, bound to
//! the org, with their plans. A malformed condition is a compile failure of its set. The cache
//! also keeps the last known catalog and field plan of each org (plan decision D-F).

use std::collections::{BTreeSet, HashMap};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::future::{BoxFuture, Shared};
use futures::{FutureExt, StreamExt};
use metrics::{counter, gauge, histogram};
use paigasus_gateway_core::{FieldPlan, HistoryPolicy, MalformedCondition, OnUnavailable, map_conditions};
use paigasus_kernel::tool_policy::history::{Catalog, Condition, HistoryDigester, Part};
use paigasus_kernel::tool_policy::{FloorPolicies, ToolPolicies, normalise};
use paigasus_observability::names;
use tokio::sync::watch;
use tokio::time::Instant;
use uuid::Uuid;

use crate::adapters::history::HmacHistoryDigester;
use crate::config::ToolPolicySettings;
use crate::domain::tool_policy::{PolicySource, PolicyText, SourceError, TenantPolicies, TenantSettings};

/// How many tenant reloads one refresh pass runs at the same time.
const REFRESH_CONCURRENCY: usize = 16;
/// How often one lookup joins a new load when an invalidation lands during the wait.
const MAX_ROUNDS: usize = 3;

/// The cache timings (the `[tool_policy]` config table, D31).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheSettings {
    /// How often the refresh task reloads the used entries.
    pub refresh: Duration,
    /// How long after its load an entry stays usable when the reload fails.
    pub max_stale: Duration,
    /// How long an unused tenant entry stays before the refresh evicts it.
    pub idle_evict: Duration,
    /// How long one lookup waits for a load.
    pub load_timeout: Duration,
}

impl CacheSettings {
    /// The cache timings of a `[tool_policy]` table.
    #[must_use]
    pub fn from_settings(s: &ToolPolicySettings) -> Self {
        CacheSettings {
            refresh: s.refresh,
            max_stale: s.max_stale,
            idle_evict: s.idle_evict,
            load_timeout: s.load_timeout,
        }
    }
}

/// The result of a lookup. `Unavailable` follows `on_policy_unavailable` in the caller (§ 6.4).
#[derive(Debug, Clone)]
pub enum Lookup<T> {
    Ready(T),
    Unavailable,
}

/// The compiled floor, and the floor alone as a policy set (P-4).
#[derive(Debug, Clone)]
pub struct FloorEntry {
    pub floor: Arc<FloorPolicies>,
    /// `ToolPolicies::compile(floor, "", &[])`: for tenant mode `off` and an unavailable tenant set.
    pub floor_only: Arc<ToolPolicies>,
    /// The history conditions of the floor, mapped and validated, but not bound: a digest literal
    /// depends on the org, and the floor serves every org (plan decision D-A).
    pub conditions: Arc<Vec<Condition>>,
}

impl FloorEntry {
    /// The floor has at least one history condition.
    #[must_use]
    pub fn has_conditions(&self) -> bool {
        !self.conditions.is_empty()
    }

    /// The slot names of the floor conditions. A tenant condition must not use one.
    fn slots(&self) -> BTreeSet<String> {
        self.conditions.iter().map(|c| c.slot.clone()).collect()
    }
}

/// A compiled tenant set: the merged policies and the history policy of the org (plan decision
/// D-F). Both come from one load and one floor.
#[derive(Debug, Clone)]
pub struct CompiledTenant {
    pub policies: Arc<ToolPolicies>,
    /// The floor and the tenant conditions, bound to the org, with their plans.
    pub history: Arc<HistoryPolicy>,
}

/// What the cache last knew of an org from a good tenant compile: the catalog and the field plan.
/// A floor condition uses them when the tenant set is off or unavailable (spec P7, design D7).
#[derive(Clone)]
struct LastKnown {
    catalog: Catalog,
    fields: Arc<FieldPlan>,
}

/// Something that clears the cache when the policies change (the NATS adapter takes this).
pub trait Invalidate: Send + Sync + 'static {
    /// Mark every entry as invalidated (D16).
    fn invalidate_all(&self);
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Key {
    Floor,
    Tenant(Uuid),
}

#[derive(Clone)]
struct TenantData {
    /// The text and the catalog as loaded. A new floor recompiles from them with no source call.
    source: Arc<TenantPolicies>,
    /// The floor that `compiled` was built against.
    floor: Arc<FloorPolicies>,
    /// `None` when the tenant set does not compile or a history condition is malformed.
    compiled: Option<CompiledTenant>,
}

#[derive(Clone)]
enum Data {
    Floor(FloorEntry),
    Tenant(TenantData),
}

struct Good {
    data: Data,
    version: String,
    loaded_at: Instant,
    /// The invalidation generation when the load started.
    gen_at_load: u64,
    /// The last refresh of this entry failed.
    stale: bool,
}

impl Good {
    fn usable(&self, now: Instant, gen_now: u64, max_stale: Duration) -> bool {
        self.gen_at_load >= gen_now && now.saturating_duration_since(self.loaded_at) <= max_stale
    }
}

#[derive(Clone)]
struct Inflight {
    done: Shared<BoxFuture<'static, bool>>,
    /// Set by the first waiter that times out, so a load counts one `timeout`.
    timed_out: Arc<AtomicBool>,
}

struct Entry {
    good: Option<Good>,
    inflight: Option<Inflight>,
    last_used: Instant,
}

impl Entry {
    fn new(now: Instant) -> Self {
        Entry {
            good: None,
            inflight: None,
            last_used: now,
        }
    }
}

struct Inner {
    /// Bumped by `invalidate_all`. An entry loaded under an older value is invalidated.
    gen_now: u64,
    floor: Entry,
    tenants: HashMap<Uuid, Entry>,
    /// The last good tenant compile of each org. An idle eviction removes the org here too.
    last_known: HashMap<Uuid, LastKnown>,
}

impl Inner {
    fn entry(&mut self, key: Key, now: Instant) -> &mut Entry {
        match key {
            Key::Floor => &mut self.floor,
            Key::Tenant(org) => self.tenants.entry(org).or_insert_with(|| Entry::new(now)),
        }
    }

    fn existing(&mut self, key: Key) -> Option<&mut Entry> {
        match key {
            Key::Floor => Some(&mut self.floor),
            Key::Tenant(org) => self.tenants.get_mut(&org),
        }
    }
}

enum Fetched {
    Floor(PolicyText),
    Tenant(Uuid, TenantPolicies),
}

/// Why a set does not compile. The log gets a fixed code and, for a condition, a fixed reason.
enum CompileFailure {
    Cedar,
    History(MalformedCondition),
}

impl CompileFailure {
    fn log(&self, key: Key) {
        match self {
            CompileFailure::Cedar => tracing::warn!(?key, code = "compile_error", "tool-policy set does not compile"),
            CompileFailure::History(m) => tracing::warn!(?key, code = "compile_error", index = m.index, reason = m.reason, "tool-policy history condition is malformed"),
        }
    }
}

/// The catalog of a tenant set as the history keys it: the normal form of each name.
fn catalog_of(source: &TenantPolicies) -> Catalog {
    Catalog::known(source.catalog.iter().map(|n| normalise(n)))
}

/// The policy cache.
pub struct PolicyCache {
    source: Arc<dyn PolicySource>,
    settings: CacheSettings,
    /// Binds the digest literals of the history conditions to an org (plan decision D-A).
    digester: Arc<dyn HistoryDigester>,
    inner: Mutex<Inner>,
}

impl PolicyCache {
    /// A cache with no history key: a digest literal gets no digest, so its slot is missing with
    /// reason `no_key`. [`Self::with_digester`] sets the key of the gateway.
    #[must_use]
    pub fn new(source: Arc<dyn PolicySource>, settings: CacheSettings) -> Arc<Self> {
        let now = Instant::now();
        Arc::new(PolicyCache {
            source,
            settings,
            digester: Arc::new(HmacHistoryDigester::keyless()),
            inner: Mutex::new(Inner {
                gen_now: 0,
                floor: Entry::new(now),
                tenants: HashMap::new(),
                last_known: HashMap::new(),
            }),
        })
    }

    /// The cache with this digester. Call it at boot, right after [`Self::new`].
    ///
    /// # Panics
    ///
    /// When the cache is already shared (another `Arc` to it exists): the digester must be set
    /// before any load binds a condition.
    #[must_use]
    pub fn with_digester(self: Arc<Self>, digester: Arc<dyn HistoryDigester>) -> Arc<Self> {
        let Ok(mut cache) = Arc::try_unwrap(self) else {
            panic!("PolicyCache::with_digester runs before the cache is shared");
        };
        cache.digester = digester;
        Arc::new(cache)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // No code panics while it holds the lock, so a poisoned lock still holds a good value.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The compiled floor. The wait ends `load_timeout` from now.
    pub async fn floor(self: &Arc<Self>, on_unavailable: OnUnavailable) -> Lookup<FloorEntry> {
        self.floor_until(on_unavailable, Instant::now() + self.settings.load_timeout).await
    }

    /// The compiled floor. The wait ends at `deadline`; the `open` fallback still runs then.
    pub async fn floor_until(self: &Arc<Self>, on_unavailable: OnUnavailable, deadline: Instant) -> Lookup<FloorEntry> {
        match self.lookup(Key::Floor, None, on_unavailable, false, deadline).await {
            Ok(Data::Floor(entry)) => Lookup::Ready(entry),
            _ => Lookup::Unavailable,
        }
    }

    /// The tenant policy set, compiled against `floor`. A tenant set that does not compile is
    /// `Unavailable` for the tenant only; the floor stays usable (§ 6.1.2).
    /// The wait ends `load_timeout` from now.
    pub async fn tenant(self: &Arc<Self>, org: Uuid, floor: &FloorEntry, on_unavailable: OnUnavailable) -> Lookup<CompiledTenant> {
        self.tenant_until(org, floor, on_unavailable, Instant::now() + self.settings.load_timeout).await
    }

    /// The tenant policy set, as [`Self::tenant`]. The wait ends at `deadline`; the `open`
    /// fallback still runs then, so a caller with a shared deadline keeps the last good set.
    pub async fn tenant_until(self: &Arc<Self>, org: Uuid, floor: &FloorEntry, on_unavailable: OnUnavailable, deadline: Instant) -> Lookup<CompiledTenant> {
        match self.lookup(Key::Tenant(org), Some(floor.clone()), on_unavailable, false, deadline).await {
            Ok(Data::Tenant(data)) => self.tenant_set(org, &data, floor),
            _ => Lookup::Unavailable,
        }
    }

    /// The tenant policy set and the tenant settings, as [`Self::tenant_until`]. When the lookup
    /// serves an entry, the settings come from that same entry, so the set and the settings are
    /// always from one load. When it serves nothing, the settings are those of the stored entry
    /// that the failure fallback read, the last known value.
    ///
    /// `on_unavailable` is only the default: when the stored entry has settings, the failure
    /// fallback uses the fail mode of that entry, read under the same lock. A settings change
    /// that lands after the caller read its fail mode thus cannot pair a fallback decision with
    /// the settings of another load (CodeRabbit, PR 485).
    pub async fn tenant_with_settings_until(self: &Arc<Self>, org: Uuid, floor: &FloorEntry, on_unavailable: OnUnavailable, deadline: Instant) -> (Lookup<CompiledTenant>, Option<TenantSettings>) {
        match self.lookup(Key::Tenant(org), Some(floor.clone()), on_unavailable, true, deadline).await {
            Ok(Data::Tenant(data)) => (self.tenant_set(org, &data, floor), data.source.settings),
            Ok(Data::Floor(_)) => (Lookup::Unavailable, None),
            Err(seen) => (Lookup::Unavailable, seen),
        }
    }

    fn tenant_set(&self, org: Uuid, data: &TenantData, floor: &FloorEntry) -> Lookup<CompiledTenant> {
        if Arc::ptr_eq(&data.floor, &floor.floor) {
            return data.compiled.clone().map_or(Lookup::Unavailable, Lookup::Ready);
        }
        // The floor changed after the tenant set was compiled: compile again from the stored text.
        // This checks the slot names against the new floor and binds the new floor conditions.
        self.recompile(org, data, floor)
    }

    fn recompile(&self, org: Uuid, data: &TenantData, floor: &FloorEntry) -> Lookup<CompiledTenant> {
        let started = Instant::now();
        let compiled = self.compile_tenant(org, &data.source, floor);
        histogram!(names::GATEWAY_TOOL_POLICY_COMPILE_SECONDS).record(started.elapsed().as_secs_f64());
        let compiled = match compiled {
            Ok(c) => Some(c),
            Err(failure) => {
                counter!(names::GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL, "result" => "compile_error").increment(1);
                failure.log(Key::Tenant(org));
                None
            }
        };
        {
            let mut inner = self.lock();
            let mut stored_it = false;
            if let Some(Entry { good: Some(good), .. }) = inner.existing(Key::Tenant(org))
                && let Data::Tenant(stored) = &mut good.data
                && Arc::ptr_eq(&stored.source, &data.source)
            {
                stored.floor = Arc::clone(&floor.floor);
                stored.compiled.clone_from(&compiled);
                stored_it = true;
            }
            if stored_it && let Some(c) = &compiled {
                Self::remember(&mut inner, org, &data.source, c);
            }
        }
        compiled.map_or(Lookup::Unavailable, Lookup::Ready)
    }

    /// Compile a tenant set against `floor`: map and validate its conditions (a tenant slot must
    /// not equal a floor slot), compile the Cedar text, and bind the floor and the tenant
    /// conditions to `org`.
    fn compile_tenant(&self, org: Uuid, source: &TenantPolicies, floor: &FloorEntry) -> Result<CompiledTenant, CompileFailure> {
        let conditions = map_conditions(Part::Tenant, &source.conditions, source.max_window_ms, &floor.slots()).map_err(CompileFailure::History)?;
        let policies = ToolPolicies::compile(Arc::clone(&floor.floor), &source.text, &source.catalog).map_err(|_| CompileFailure::Cedar)?;
        let history = HistoryPolicy::build(&floor.conditions, &conditions, org, catalog_of(source), None, self.digester.as_ref());
        Ok(CompiledTenant {
            policies: Arc::new(policies),
            history: Arc::new(history),
        })
    }

    /// Keep the catalog and the field plan of a good tenant compile (plan decision D-F).
    fn remember(inner: &mut Inner, org: Uuid, source: &TenantPolicies, compiled: &CompiledTenant) {
        inner.last_known.insert(
            org,
            LastKnown {
                catalog: catalog_of(source),
                fields: Arc::clone(compiled.history.fields()),
            },
        );
    }

    /// The floor conditions alone, bound to `org`, for tenant mode `off` and an unavailable tenant
    /// set (spec P7, plan decision D-F). The catalog and the field plan are the last known ones of
    /// the org, so a floor `any_catalog_tool` condition still knows the catalog, and the history
    /// still stores the fields of the tenant conditions. With no good load of the org, the catalog
    /// is unknown.
    #[must_use]
    pub fn floor_history(&self, org: Uuid, floor: &FloorEntry) -> Arc<HistoryPolicy> {
        let known = self.lock().last_known.get(&org).cloned();
        let (catalog, stored) = match known {
            Some(k) => (k.catalog, Some(k.fields)),
            None => (Catalog::unknown(), None),
        };
        Arc::new(HistoryPolicy::build(&floor.conditions, &[], org, catalog, stored.as_deref(), self.digester.as_ref()))
    }

    /// The settings of the last stored answer for this tenant, whatever its age and its `stale`
    /// flag: the last known value (spec §9.3). An answer with a set that does not compile (or with
    /// a malformed history condition) is stored too, so its settings apply. It never starts a load. An evicted tenant, or a tenant
    /// that never loaded, has none. A source with no settings gives none.
    #[must_use]
    pub fn tenant_settings(&self, org: Uuid) -> Option<TenantSettings> {
        let mut inner = self.lock();
        match &inner.existing(Key::Tenant(org))?.good.as_ref()?.data {
            Data::Tenant(t) => t.source.settings,
            Data::Floor(_) => None,
        }
    }

    /// D16: mark every entry as invalidated. The next request reloads.
    pub fn invalidate_all(&self) {
        self.lock().gen_now += 1;
    }

    /// The generic lookup. `Err` means unavailable; see [`Self::fallback`] for its value and for
    /// `follow_entry`. Every wait ends at `deadline`, and the failure fallback runs after it, so
    /// the lookup returns at `deadline` at the latest.
    async fn lookup(self: &Arc<Self>, key: Key, floor: Option<FloorEntry>, on_unavailable: OnUnavailable, follow_entry: bool, deadline: Instant) -> Result<Data, Option<TenantSettings>> {
        for _ in 0..MAX_ROUNDS {
            let inflight = {
                let mut inner = self.lock();
                let now = Instant::now();
                let gen_now = inner.gen_now;
                let entry = inner.entry(key, now);
                entry.last_used = now;
                if let Some(good) = &entry.good
                    && good.usable(now, gen_now, self.settings.max_stale)
                {
                    return Ok(good.data.clone());
                }
                self.start_or_join(&mut inner, key, floor.clone(), now)
            };
            match tokio::time::timeout_at(deadline, inflight.done.clone()).await {
                // Look at the entry again: an invalidation can land during a load.
                Ok(true) => {}
                Ok(false) => break,
                Err(_) => {
                    self.count_timeout(&inflight);
                    break;
                }
            }
        }
        // The last round can end with a load that finished and is current. Read it before the
        // failure fallback, which under `closed` serves nothing.
        {
            let mut inner = self.lock();
            let now = Instant::now();
            let gen_now = inner.gen_now;
            if let Some(good) = inner.existing(key).and_then(|e| e.good.as_ref())
                && good.usable(now, gen_now, self.settings.max_stale)
            {
                return Ok(good.data.clone());
            }
        }
        self.fallback(key, on_unavailable, follow_entry)
    }

    /// A load failed, timed out or was too old. Under `open`, the last good entry still serves up
    /// to `max_stale` after its load. Under `closed`, nothing serves (§ 6.3.3).
    ///
    /// With `follow_entry`, a stored tenant entry with settings decides by its own fail mode, not
    /// by `on_unavailable`, and `Err` carries those settings: the decision and the settings come
    /// from one read under one lock. An entry with no settings (the file source) uses
    /// `on_unavailable`.
    fn fallback(&self, key: Key, on_unavailable: OnUnavailable, follow_entry: bool) -> Result<Data, Option<TenantSettings>> {
        let mut inner = self.lock();
        let now = Instant::now();
        let Some(good) = inner.existing(key).and_then(|e| e.good.as_ref()) else {
            return Err(None);
        };
        let seen = match &good.data {
            Data::Tenant(t) if follow_entry => t.source.settings,
            _ => None,
        };
        let on = seen.map_or(on_unavailable, |s| s.on_unavailable);
        if on == OnUnavailable::Open && now.saturating_duration_since(good.loaded_at) <= self.settings.max_stale {
            Ok(good.data.clone())
        } else {
            Err(seen)
        }
    }

    fn count_timeout(&self, inflight: &Inflight) {
        if !inflight.timed_out.swap(true, Ordering::SeqCst) {
            counter!(names::GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL, "result" => "timeout").increment(1);
        }
    }

    /// Join the load of this entry, or spawn one. The caller holds the lock.
    fn start_or_join(self: &Arc<Self>, inner: &mut Inner, key: Key, floor: Option<FloorEntry>, now: Instant) -> Inflight {
        let gen_now = inner.gen_now;
        let entry = inner.entry(key, now);
        if let Some(existing) = &entry.inflight {
            return existing.clone();
        }
        let timed_out = Arc::new(AtomicBool::new(false));
        let cache = Arc::clone(self);
        let flag = Arc::clone(&timed_out);
        let handle = tokio::spawn(async move {
            // The cap keeps a hung source from blocking every later load of this entry. It is one
            // refresh interval, so a hung call delays recovery for no longer than that. It is never
            // shorter than `load_timeout`: a load that a waiter may still accept must not be
            // cancelled first.
            let cap = cache.settings.refresh.min(cache.settings.max_stale).max(cache.settings.load_timeout);
            let run = AssertUnwindSafe(tokio::time::timeout(cap, cache.load(key, floor, gen_now))).catch_unwind().await;
            let ok = match run {
                Ok(Ok(ok)) => ok,
                Ok(Err(_elapsed)) => {
                    if !flag.swap(true, Ordering::SeqCst) {
                        counter!(names::GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL, "result" => "timeout").increment(1);
                    }
                    cache.mark_stale(key);
                    false
                }
                Err(_panic) => {
                    counter!(names::GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL, "result" => "source_error").increment(1);
                    tracing::error!(code = "load_panicked", "tool-policy load task panicked");
                    cache.mark_stale(key);
                    false
                }
            };
            if let Some(entry) = cache.lock().existing(key) {
                entry.inflight = None;
            }
            ok
        });
        let done = async move { handle.await.unwrap_or(false) }.boxed().shared();
        let inflight = Inflight { done, timed_out };
        entry.inflight = Some(inflight.clone());
        inflight
    }

    fn mark_stale(&self, key: Key) {
        if let Some(Entry { good: Some(good), .. }) = self.lock().existing(key) {
            good.stale = true;
        }
    }

    /// Load one entry. `true` means the entry holds a usable set that was current when the load started.
    async fn load(&self, key: Key, floor: Option<FloorEntry>, gen_at_load: u64) -> bool {
        let started = Instant::now();
        let fetched = match key {
            Key::Floor => self.source.floor().await.map(Fetched::Floor),
            Key::Tenant(org) => self.source.tenant(org).await.map(|t| Fetched::Tenant(org, t)),
        };
        histogram!(names::GATEWAY_TOOL_POLICY_LOAD_SECONDS).record(started.elapsed().as_secs_f64());
        let fetched = match fetched {
            Ok(f) => f,
            Err(err) => {
                counter!(names::GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL, "result" => "source_error").increment(1);
                let kind = match err {
                    SourceError::NotFound(_) => "not_found",
                    SourceError::Io(_) => "io",
                    SourceError::Malformed(_) => "malformed",
                };
                tracing::warn!(?key, kind, "tool-policy source failed");
                self.mark_stale(key);
                return false;
            }
        };
        let version = match &fetched {
            Fetched::Floor(p) => p.version.clone(),
            Fetched::Tenant(_, t) => t.version.clone(),
        };

        // An unchanged version needs no compile: the stored set stays.
        {
            let mut inner = self.lock();
            if let Some(Entry { good: Some(good), .. }) = inner.existing(key)
                && good.version == version
            {
                good.loaded_at = Instant::now();
                good.gen_at_load = gen_at_load;
                // A remembered tenant set that does not compile is still a compile failure, not a good load.
                if matches!(&good.data, Data::Tenant(t) if t.compiled.is_none()) {
                    good.stale = true;
                    counter!(names::GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL, "result" => "compile_error").increment(1);
                    return false;
                }
                good.stale = false;
                counter!(names::GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL, "result" => "ok").increment(1);
                return true;
            }
        }

        let compile_started = Instant::now();
        let built = self.build(key, fetched, floor);
        histogram!(names::GATEWAY_TOOL_POLICY_COMPILE_SECONDS).record(compile_started.elapsed().as_secs_f64());
        match built {
            Ok(data) => {
                let known = match (&data, key) {
                    (Data::Tenant(TenantData { source, compiled: Some(c), .. }), Key::Tenant(org)) => Some((org, Arc::clone(source), c.clone())),
                    _ => None,
                };
                let good = Good {
                    data,
                    version,
                    loaded_at: Instant::now(),
                    gen_at_load,
                    stale: false,
                };
                let mut inner = self.lock();
                // An entry that was evicted during the load stays evicted, and so does its org.
                if let Some(entry) = inner.existing(key) {
                    entry.good = Some(good);
                    if let Some((org, source, compiled)) = known {
                        Self::remember(&mut inner, org, &source, &compiled);
                    }
                }
                counter!(names::GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL, "result" => "ok").increment(1);
                true
            }
            Err(failed) => {
                counter!(names::GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL, "result" => "compile_error").increment(1);
                let mut inner = self.lock();
                if let Some(entry) = inner.existing(key) {
                    match (&mut entry.good, failed) {
                        // Store the bad tenant set, so each request does not compile it again, in two cases:
                        // - A first load.
                        // - An answer that holds the settings (the IAM source). The answer is the current
                        //   truth (final review I-1): its settings decide, and the set is unavailable, so the
                        //   new fail mode applies. A malformed history condition is such a failure too. The last good set must not
                        //   serve under the new settings, because the set and the settings come from one load
                        //   (spec §9.3). Without this, a change to `enforce` and `closed` that comes with a set
                        //   that does not compile would leave the old mode in effect.
                        (None, Some(data)) => Self::store_bad(&mut entry.good, data, version, gen_at_load),
                        (Some(_), Some(data)) if matches!(&data, Data::Tenant(t) if t.source.settings.is_some()) => {
                            Self::store_bad(&mut entry.good, data, version, gen_at_load);
                        }
                        // A bad new version from a source with no settings (the file source): keep the
                        // last good set.
                        (Some(good), _) => good.stale = true,
                        (None, None) => {}
                    }
                }
                false
            }
        }
    }

    /// Store a tenant set that does not compile as the entry, with `compiled: None`.
    fn store_bad(slot: &mut Option<Good>, data: Data, version: String, gen_at_load: u64) {
        *slot = Some(Good {
            data,
            version,
            loaded_at: Instant::now(),
            gen_at_load,
            stale: false,
        });
    }

    /// Compile a fetched set, and log a failure. On failure the `Err` holds the tenant data with
    /// `compiled: None`. A malformed floor condition makes the floor unavailable (`Err(None)`).
    fn build(&self, key: Key, fetched: Fetched, floor: Option<FloorEntry>) -> Result<Data, Option<Data>> {
        let fail = |failure: CompileFailure| failure.log(key);
        match fetched {
            Fetched::Floor(p) => {
                let compiled = map_conditions(Part::Floor, &p.conditions, p.max_window_ms, &BTreeSet::new())
                    .map_err(CompileFailure::History)
                    .and_then(|conditions| {
                        let floor = Arc::new(ToolPolicies::compile_floor(&p.text).map_err(|_| CompileFailure::Cedar)?);
                        let floor_only = Arc::new(ToolPolicies::compile(Arc::clone(&floor), "", &[]).map_err(|_| CompileFailure::Cedar)?);
                        Ok(FloorEntry {
                            floor,
                            floor_only,
                            conditions: Arc::new(conditions),
                        })
                    });
                compiled.map(Data::Floor).map_err(|failure| {
                    fail(failure);
                    None
                })
            }
            Fetched::Tenant(org, t) => {
                let Some(floor) = floor else {
                    fail(CompileFailure::Cedar);
                    return Err(None);
                };
                let source = Arc::new(t);
                let compiled = self.compile_tenant(org, &source, &floor).map_err(fail).ok();
                let data = TenantData { source, floor: floor.floor, compiled };
                if data.compiled.is_some() { Ok(Data::Tenant(data)) } else { Err(Some(Data::Tenant(data))) }
            }
        }
    }

    /// One refresh pass: evict idle tenants, reload the others, set the stale gauge.
    pub async fn refresh_once(self: &Arc<Self>) {
        self.evict_idle();

        if !self.refresh_entry(Key::Floor, None).await {
            counter!(names::GATEWAY_TOOL_POLICY_REFRESH_ERRORS_TOTAL).increment(1);
        }
        let (floor, orgs) = {
            let inner = self.lock();
            let floor = match &inner.floor.good {
                Some(Good { data: Data::Floor(f), .. }) => Some(f.clone()),
                _ => None,
            };
            (floor, inner.tenants.keys().copied().collect::<Vec<_>>())
        };
        // Tenant sets compile against the floor, so with no floor there is nothing to reload.
        if let Some(floor) = floor {
            futures::stream::iter(orgs)
                .for_each_concurrent(REFRESH_CONCURRENCY, |org| {
                    let floor = floor.clone();
                    async move {
                        if !self.refresh_entry(Key::Tenant(org), Some(floor)).await {
                            counter!(names::GATEWAY_TOOL_POLICY_REFRESH_ERRORS_TOTAL).increment(1);
                        }
                    }
                })
                .await;
        }

        let stale = {
            let inner = self.lock();
            std::iter::once(&inner.floor).chain(inner.tenants.values()).filter(|e| e.good.as_ref().is_some_and(|g| g.stale)).count()
        };
        #[allow(clippy::cast_precision_loss)]
        gauge!(names::GATEWAY_TOOL_POLICY_STALE_ENTRIES).set(stale as f64);
    }

    async fn refresh_entry(self: &Arc<Self>, key: Key, floor: Option<FloorEntry>) -> bool {
        let inflight = {
            let mut inner = self.lock();
            // The refresh does not count as a use, and it does not make an entry that was evicted.
            if inner.existing(key).is_none() {
                return true;
            }
            self.start_or_join(&mut inner, key, floor, Instant::now())
        };
        // A hung source must not stop the pass: the load goes on in the background and the next pass joins it.
        match tokio::time::timeout(self.settings.load_timeout, inflight.done.clone()).await {
            Ok(ok) => ok,
            Err(_) => {
                self.count_timeout(&inflight);
                false
            }
        }
    }

    fn evict_idle(&self) {
        let mut inner = self.lock();
        let now = Instant::now();
        let before = inner.tenants.len();
        let idle = self.settings.idle_evict;
        inner.tenants.retain(|_, e| e.inflight.is_some() || now.saturating_duration_since(e.last_used) <= idle);
        // The last known catalog and plan of an evicted org go with it (plan decision D-F).
        let Inner { tenants, last_known, .. } = &mut *inner;
        last_known.retain(|org, _| tenants.contains_key(org));
        let evicted = (before - inner.tenants.len()) as u64;
        if evicted > 0 {
            counter!(names::GATEWAY_TOOL_POLICY_EVICTIONS_TOTAL).increment(evicted);
        }
    }

    /// Run `refresh_once` every `refresh` until `shutdown` fires (or its sender goes away).
    pub async fn run_refresh(self: Arc<Self>, mut shutdown: watch::Receiver<()>) {
        let mut tick = tokio::time::interval_at(Instant::now() + self.settings.refresh, self.settings.refresh);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = tick.tick() => {}
            }
            tokio::select! {
                _ = shutdown.changed() => break,
                () = self.refresh_once() => {}
            }
        }
    }
}

impl Invalidate for PolicyCache {
    fn invalidate_all(&self) {
        PolicyCache::invalidate_all(self);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use async_trait::async_trait;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
    use paigasus_gateway_core::TenantMode;
    use paigasus_kernel::tool_policy::history::{Catalog, FieldPath, Literal, Part};
    use paigasus_proto::paigasus::iam::v1 as pb;

    use super::*;
    use crate::application::tool_policy::testing::{MAX_WINDOW_MS, formerly};

    const ORG: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000a1);
    const FLOOR: &str = r#"@id("f") forbid(principal, action == Pgs::Gateway::Action::"read_file", resource);"#;
    const TENANT: &str = r#"@id("t") permit(principal, action, resource);"#;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn settings() -> CacheSettings {
        CacheSettings {
            refresh: secs(30),
            max_stale: secs(300),
            idle_evict: secs(3600),
            load_timeout: secs(1),
        }
    }

    /// A source that counts calls, can fail, can be slow, and returns a chosen version.
    struct FakeSource {
        floor_calls: AtomicUsize,
        tenant_calls: AtomicUsize,
        fail: AtomicBool,
        delay: Mutex<Duration>,
        floor_delay: Mutex<Duration>,
        floor_text: Mutex<(String, String)>,
        tenant_text: Mutex<(String, String)>,
        tenant_settings: Mutex<Option<TenantSettings>>,
        /// The history conditions of the floor and of the tenant set, and the tenant catalog.
        floor_conditions: Mutex<Vec<pb::HistoryCondition>>,
        tenant_conditions: Mutex<Vec<pb::HistoryCondition>>,
        catalog: Mutex<Vec<String>>,
    }

    impl FakeSource {
        fn new() -> Arc<Self> {
            Arc::new(FakeSource {
                floor_calls: AtomicUsize::new(0),
                tenant_calls: AtomicUsize::new(0),
                fail: AtomicBool::new(false),
                delay: Mutex::new(Duration::ZERO),
                floor_delay: Mutex::new(Duration::ZERO),
                floor_text: Mutex::new((FLOOR.into(), "f1".into())),
                tenant_text: Mutex::new((TENANT.into(), "t1".into())),
                tenant_settings: Mutex::new(None),
                floor_conditions: Mutex::new(Vec::new()),
                tenant_conditions: Mutex::new(Vec::new()),
                catalog: Mutex::new(vec!["read_file".into()]),
            })
        }
        fn floor_calls(&self) -> usize {
            self.floor_calls.load(Ordering::SeqCst)
        }
        fn tenant_calls(&self) -> usize {
            self.tenant_calls.load(Ordering::SeqCst)
        }
        fn set_fail(&self, fail: bool) {
            self.fail.store(fail, Ordering::SeqCst);
        }
        fn set_delay(&self, d: Duration) {
            *self.delay.lock().unwrap() = d;
        }
        fn set_floor_delay(&self, d: Duration) {
            *self.floor_delay.lock().unwrap() = d;
        }
        fn set_floor(&self, text: &str, version: &str) {
            *self.floor_text.lock().unwrap() = (text.into(), version.into());
        }
        fn set_settings(&self, settings: TenantSettings) {
            *self.tenant_settings.lock().unwrap() = Some(settings);
        }
        fn set_tenant(&self, text: &str, version: &str) {
            *self.tenant_text.lock().unwrap() = (text.into(), version.into());
        }
        /// The floor conditions of the next floor answers. Set a new floor version with them.
        fn set_floor_conditions(&self, conditions: Vec<pb::HistoryCondition>) {
            *self.floor_conditions.lock().unwrap() = conditions;
        }
        /// The conditions and the catalog of the next tenant answers. Set a new version with them.
        fn set_tenant_conditions(&self, conditions: Vec<pb::HistoryCondition>, catalog: &[&str]) {
            *self.tenant_conditions.lock().unwrap() = conditions;
            *self.catalog.lock().unwrap() = catalog.iter().map(|s| (*s).to_owned()).collect();
        }
        async fn pause(&self, extra: Duration) -> Result<(), SourceError> {
            let d = (*self.delay.lock().unwrap()).max(extra);
            if !d.is_zero() {
                tokio::time::sleep(d).await;
            }
            if self.fail.load(Ordering::SeqCst) { Err(SourceError::Io("down".into())) } else { Ok(()) }
        }
    }

    #[async_trait]
    impl PolicySource for FakeSource {
        async fn floor(&self) -> Result<PolicyText, SourceError> {
            self.floor_calls.fetch_add(1, Ordering::SeqCst);
            // The content is read before the wait, like a source that has already read its data.
            let (text, version) = self.floor_text.lock().unwrap().clone();
            let extra = *self.floor_delay.lock().unwrap();
            let conditions = self.floor_conditions.lock().unwrap().clone();
            self.pause(extra).await?;
            Ok(PolicyText {
                text,
                version,
                conditions,
                max_window_ms: MAX_WINDOW_MS,
            })
        }
        async fn tenant(&self, _org: Uuid) -> Result<TenantPolicies, SourceError> {
            self.tenant_calls.fetch_add(1, Ordering::SeqCst);
            let (text, version) = self.tenant_text.lock().unwrap().clone();
            self.pause(Duration::ZERO).await?;
            Ok(TenantPolicies {
                text,
                catalog: self.catalog.lock().unwrap().clone(),
                version,
                settings: *self.tenant_settings.lock().unwrap(),
                conditions: self.tenant_conditions.lock().unwrap().clone(),
                max_window_ms: MAX_WINDOW_MS,
            })
        }
    }

    /// One drained read of the recorder. A `DebuggingRecorder` snapshot resets counters, gauges and
    /// histograms, so each `Snap` holds what happened since the previous one.
    type Row = (String, Vec<(String, String)>, DebugValue);
    struct Snap(Vec<Row>);

    fn snap(snapshotter: &Snapshotter) -> Snap {
        Snap(
            snapshotter
                .snapshot()
                .into_vec()
                .into_iter()
                .map(|(k, _, _, v)| (k.key().name().to_owned(), k.key().labels().map(|l| (l.key().to_owned(), l.value().to_owned())).collect(), v))
                .collect(),
        )
    }

    impl Snap {
        fn find(&self, name: &str, labels: &[(&str, &str)]) -> Option<&DebugValue> {
            self.0
                .iter()
                .find(|(n, l, _)| n == name && l.len() == labels.len() && labels.iter().all(|(k, v)| l.iter().any(|(lk, lv)| lk == k && lv == v)))
                .map(|(_, _, v)| v)
        }
        fn counter(&self, name: &str, labels: &[(&str, &str)]) -> u64 {
            match self.find(name, labels) {
                Some(DebugValue::Counter(n)) => *n,
                _ => 0,
            }
        }
        fn loads(&self, result: &str) -> u64 {
            self.counter(names::GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL, &[("result", result)])
        }
        fn hist_len(&self, name: &str) -> usize {
            match self.find(name, &[]) {
                Some(DebugValue::Histogram(h)) => h.len(),
                _ => 0,
            }
        }
        fn gauge(&self, name: &str) -> Option<f64> {
            match self.find(name, &[]) {
                Some(DebugValue::Gauge(v)) => Some(v.into_inner()),
                _ => None,
            }
        }
    }

    async fn ready_floor(cache: &Arc<PolicyCache>, ou: OnUnavailable) -> FloorEntry {
        match cache.floor(ou).await {
            Lookup::Ready(f) => f,
            Lookup::Unavailable => panic!("the floor must be ready"),
        }
    }

    async fn ready_tenant(cache: &Arc<PolicyCache>, floor: &FloorEntry, ou: OnUnavailable) -> Arc<ToolPolicies> {
        ready_compiled(cache, floor, ou).await.policies
    }

    async fn ready_compiled(cache: &Arc<PolicyCache>, floor: &FloorEntry, ou: OnUnavailable) -> CompiledTenant {
        match cache.tenant(ORG, floor, ou).await {
            Lookup::Ready(t) => t,
            Lookup::Unavailable => panic!("the tenant set must be ready"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_misses_share_one_load() {
        let src = FakeSource::new();
        src.set_delay(Duration::from_millis(100));
        let cache = PolicyCache::new(src.clone(), settings());
        let results = futures::future::join_all((0..8).map(|_| cache.floor(OnUnavailable::Closed))).await;
        assert!(results.iter().all(|r| matches!(r, Lookup::Ready(_))));
        assert_eq!(src.floor_calls(), 1, "eight waiters, one source call");
    }

    #[tokio::test(start_paused = true)]
    async fn a_dropped_waiter_does_not_cancel_the_load() {
        let src = FakeSource::new();
        src.set_delay(Duration::from_millis(100));
        let cache = PolicyCache::new(src.clone(), settings());
        // Poll the first lookup until it waits, then drop it.
        assert!(tokio::time::timeout(Duration::from_millis(10), cache.floor(OnUnavailable::Closed)).await.is_err());
        assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Ready(_)));
        assert_eq!(src.floor_calls(), 1, "the second lookup joined the load of the dropped waiter");
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_load_is_unavailable_after_load_timeout() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let src = FakeSource::new();
        src.set_delay(secs(2));
        let cache = PolicyCache::new(src.clone(), settings());
        let started = Instant::now();
        let (a, b) = tokio::join!(cache.floor(OnUnavailable::Closed), cache.floor(OnUnavailable::Closed));
        assert!(matches!((a, b), (Lookup::Unavailable, Lookup::Unavailable)));
        assert_eq!(started.elapsed(), secs(1), "the waiters give up at load_timeout");
        assert_eq!(snap(&snapshotter).loads("timeout"), 1, "one timeout per load, not per waiter");
        // The load goes on in the background.
        tokio::time::sleep(secs(2)).await;
        assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Ready(_)));
        assert_eq!(src.floor_calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_refresh_keeps_the_last_good_entry_until_max_stale() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let src = FakeSource::new();
        let cache = PolicyCache::new(src.clone(), settings());
        ready_floor(&cache, OnUnavailable::Closed).await;
        src.set_fail(true);
        tokio::time::advance(secs(30)).await;
        cache.refresh_once().await;
        let s = snap(&snapshotter);
        assert_eq!(s.counter(names::GATEWAY_TOOL_POLICY_REFRESH_ERRORS_TOTAL, &[]), 1);
        assert_eq!(s.gauge(names::GATEWAY_TOOL_POLICY_STALE_ENTRIES), Some(1.0));
        tokio::time::advance(secs(269)).await; // 299 s after the load
        assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Ready(_)), "still inside max_stale");
        tokio::time::advance(secs(2)).await; // 301 s
        assert!(matches!(cache.floor(OnUnavailable::Open).await, Lookup::Unavailable), "past max_stale, even under open");
        // Recovery: a good source clears the stale mark.
        src.set_fail(false);
        cache.refresh_once().await;
        assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Ready(_)));
        assert_eq!(snap(&snapshotter).gauge(names::GATEWAY_TOOL_POLICY_STALE_ENTRIES), Some(0.0));
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_reload_after_invalidate_is_unavailable_under_closed() {
        let src = FakeSource::new();
        let cache = PolicyCache::new(src.clone(), settings());
        ready_floor(&cache, OnUnavailable::Closed).await;
        src.set_fail(true);
        cache.invalidate_all();
        assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Unavailable), "closed: unavailable at once");
        assert!(matches!(cache.floor(OnUnavailable::Open).await, Lookup::Ready(_)), "open: the last good entry, inside max_stale");
        tokio::time::advance(secs(301)).await;
        assert!(matches!(cache.floor(OnUnavailable::Open).await, Lookup::Unavailable), "open: not past max_stale");
    }

    #[tokio::test(start_paused = true)]
    async fn an_invalidate_does_not_change_a_set_a_request_already_holds() {
        let src = FakeSource::new();
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        let before = ready_tenant(&cache, &floor, OnUnavailable::Closed).await;
        src.set_tenant(r#"@id("t2") permit(principal, action, resource);"#, "t2");
        cache.invalidate_all();
        let after = ready_tenant(&cache, &floor, OnUnavailable::Closed).await;
        assert!(!Arc::ptr_eq(&before, &after), "the invalidate loaded a new set");
        assert_eq!(src.tenant_calls(), 2);
        // The set held before is a separate Arc, and the cache replaced its entry, not its content.
        assert_eq!(Arc::strong_count(&before), 1, "the cache no longer holds the old set");
    }

    #[tokio::test(start_paused = true)]
    async fn an_idle_entry_is_evicted() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let src = FakeSource::new();
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        ready_tenant(&cache, &floor, OnUnavailable::Closed).await;
        tokio::time::advance(secs(3601)).await;
        cache.refresh_once().await;
        assert_eq!(snap(&snapshotter).counter(names::GATEWAY_TOOL_POLICY_EVICTIONS_TOTAL, &[]), 1);
        assert_eq!(src.tenant_calls(), 1, "an evicted entry is not reloaded");
        assert!(cache.lock().tenants.is_empty());
        // A used entry stays.
        ready_tenant(&cache, &floor, OnUnavailable::Closed).await;
        tokio::time::advance(secs(60)).await;
        cache.refresh_once().await;
        assert_eq!(snap(&snapshotter).counter(names::GATEWAY_TOOL_POLICY_EVICTIONS_TOTAL, &[]), 0, "no new eviction");
        assert_eq!(cache.lock().tenants.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn an_unchanged_version_is_not_compiled_again() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let src = FakeSource::new();
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        ready_tenant(&cache, &floor, OnUnavailable::Closed).await;
        let compiles = snap(&snapshotter).hist_len(names::GATEWAY_TOOL_POLICY_COMPILE_SECONDS);
        assert_eq!(compiles, 2, "one floor compile, one tenant compile");
        tokio::time::advance(secs(30)).await;
        cache.refresh_once().await;
        assert_eq!((src.floor_calls(), src.tenant_calls()), (2, 2), "the refresh asked the source");
        assert_eq!(snap(&snapshotter).hist_len(names::GATEWAY_TOOL_POLICY_COMPILE_SECONDS), 0, "no new compile");
        // A changed version compiles.
        src.set_tenant(r#"@id("t2") permit(principal, action, resource);"#, "t2");
        cache.refresh_once().await;
        assert_eq!(snap(&snapshotter).hist_len(names::GATEWAY_TOOL_POLICY_COMPILE_SECONDS), 1, "a changed version compiles once");
    }

    #[tokio::test(start_paused = true)]
    async fn a_miss_whose_load_fails_is_unavailable() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let src = FakeSource::new();
        src.set_fail(true);
        let cache = PolicyCache::new(src.clone(), settings());
        assert!(matches!(cache.floor(OnUnavailable::Open).await, Lookup::Unavailable), "open has no last good entry");
        assert_eq!(snap(&snapshotter).loads("source_error"), 1);
        // Once the source works again, the next lookup loads.
        src.set_fail(false);
        assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Ready(_)));
    }

    #[tokio::test(start_paused = true)]
    async fn a_tenant_that_fails_to_compile_keeps_the_floor() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let src = FakeSource::new();
        src.set_tenant("permit((", "bad");
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        assert!(matches!(cache.tenant(ORG, &floor, OnUnavailable::Open).await, Lookup::Unavailable));
        assert_eq!(snap(&snapshotter).loads("compile_error"), 1);
        assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Ready(_)), "the floor stays ready (§ 6.1.2)");
        // The bad set is remembered: a second request neither calls the source nor compiles again.
        assert!(matches!(cache.tenant(ORG, &floor, OnUnavailable::Closed).await, Lookup::Unavailable));
        assert_eq!(src.tenant_calls(), 1);
        assert_eq!(snap(&snapshotter).loads("compile_error"), 0, "not compiled again");
        // A fixed set recovers on the next refresh.
        src.set_tenant(TENANT, "good");
        tokio::time::advance(secs(30)).await;
        cache.refresh_once().await;
        ready_tenant(&cache, &floor, OnUnavailable::Closed).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_floor_recompiles_the_tenant_without_a_source_call() {
        let src = FakeSource::new();
        let cache = PolicyCache::new(src.clone(), settings());
        let old_floor = ready_floor(&cache, OnUnavailable::Closed).await;
        tokio::time::advance(secs(299)).await;
        let old_set = ready_tenant(&cache, &old_floor, OnUnavailable::Closed).await;
        assert_eq!(src.tenant_calls(), 1);
        // The floor expires at 301 s, the tenant entry (loaded at 299 s) does not.
        src.set_floor(r#"@id("f2") forbid(principal, action == Pgs::Gateway::Action::"send_email", resource);"#, "f2");
        tokio::time::advance(secs(2)).await;
        let new_floor = ready_floor(&cache, OnUnavailable::Closed).await;
        assert!(!Arc::ptr_eq(&old_floor.floor, &new_floor.floor));
        assert_eq!(src.floor_calls(), 2);
        let new_set = ready_tenant(&cache, &new_floor, OnUnavailable::Closed).await;
        assert_eq!(src.tenant_calls(), 1, "no second tenant source call");
        assert!(!Arc::ptr_eq(&old_set, &new_set), "the tenant set is compiled against the new floor");
        assert!(Arc::ptr_eq(new_set.floor(), &new_floor.floor));
        assert!(Arc::ptr_eq(old_set.floor(), &old_floor.floor), "the old set keeps the old floor");
    }

    #[tokio::test(start_paused = true)]
    async fn the_floor_only_set_is_built_on_the_floor() {
        let cache = PolicyCache::new(FakeSource::new(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        assert!(Arc::ptr_eq(floor.floor_only.floor(), &floor.floor), "P-4: the floor-only set uses the same floor");
    }

    #[tokio::test(start_paused = true)]
    async fn an_unchanged_floor_keeps_its_arcs() {
        let cache = PolicyCache::new(FakeSource::new(), settings());
        let first = ready_floor(&cache, OnUnavailable::Closed).await;
        tokio::time::advance(secs(30)).await;
        cache.refresh_once().await;
        let second = ready_floor(&cache, OnUnavailable::Closed).await;
        assert!(Arc::ptr_eq(&first.floor, &second.floor), "so no tenant set compiles again");
    }

    #[tokio::test(start_paused = true)]
    async fn run_refresh_reloads_every_interval_and_stops_on_shutdown() {
        let src = FakeSource::new();
        let cache = PolicyCache::new(src.clone(), settings());
        ready_floor(&cache, OnUnavailable::Closed).await;
        let (tx, rx) = watch::channel(());
        let task = tokio::spawn(Arc::clone(&cache).run_refresh(rx));
        tokio::time::sleep(secs(95)).await;
        assert_eq!(src.floor_calls(), 1 + 3, "passes at 30, 60 and 90 s");
        tx.send(()).unwrap();
        task.await.unwrap();
        tokio::time::sleep(secs(60)).await;
        assert_eq!(src.floor_calls(), 4, "no pass after shutdown");
    }

    #[tokio::test(start_paused = true)]
    async fn the_invalidate_trait_clears_the_cache() {
        let src = FakeSource::new();
        let cache = PolicyCache::new(src.clone(), settings());
        ready_floor(&cache, OnUnavailable::Closed).await;
        let as_trait: Arc<dyn Invalidate> = cache.clone();
        as_trait.invalidate_all();
        ready_floor(&cache, OnUnavailable::Closed).await;
        assert_eq!(src.floor_calls(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_hung_floor_call_does_not_stop_the_refresh_pass() {
        let src = FakeSource::new();
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        ready_tenant(&cache, &floor, OnUnavailable::Closed).await;
        src.set_floor_delay(secs(3600));
        tokio::time::advance(secs(30)).await;
        let started = Instant::now();
        cache.refresh_once().await;
        assert_eq!(started.elapsed(), secs(1), "the pass waits load_timeout for the hung floor and goes on");
        assert_eq!(src.tenant_calls(), 2, "the tenant was refreshed after the floor wait");
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_load_starts_after_the_cap_of_a_hung_call() {
        let src = FakeSource::new();
        let cache = PolicyCache::new(src.clone(), settings());
        ready_floor(&cache, OnUnavailable::Closed).await;
        src.set_floor_delay(secs(3600));
        cache.invalidate_all();
        assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Unavailable));
        tokio::time::advance(secs(10)).await;
        assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Unavailable), "the hung call is still in flight");
        assert_eq!(src.floor_calls(), 2, "the lookups joined the one hung call");
        // The cap is one refresh interval (30 s) from the start of the hung call.
        tokio::time::advance(secs(25)).await;
        src.set_floor_delay(Duration::ZERO);
        // Let the capped task run its timeout branch.
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Ready(_)), "a new load started and the source answered");
        assert_eq!(src.floor_calls(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn the_cap_is_never_shorter_than_load_timeout() {
        // `refresh_secs = 1` with `load_timeout = 4 s` passes validation. A 1.5 s load is inside
        // the waiter deadline, so the background cap must not cancel it at one refresh interval.
        let src = FakeSource::new();
        let s = CacheSettings {
            refresh: secs(1),
            load_timeout: secs(4),
            ..settings()
        };
        let cache = PolicyCache::new(src.clone(), s);
        src.set_floor_delay(Duration::from_millis(1500));
        assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Ready(_)), "the load ends inside load_timeout");
        assert_eq!(src.floor_calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn an_invalidate_during_a_load_is_not_lost() {
        for ou in [OnUnavailable::Closed, OnUnavailable::Open] {
            let src = FakeSource::new();
            let cache = PolicyCache::new(src.clone(), settings());
            let floor = ready_floor(&cache, ou).await;
            src.set_delay(Duration::from_millis(300));
            let change = async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                src.set_tenant(r#"@id("t2") permit(principal, action, resource);"#, "t2");
                cache.invalidate_all();
            };
            let (set, ()) = tokio::join!(cache.tenant(ORG, &floor, ou), change);
            assert!(matches!(set, Lookup::Ready(_)), "the second load finishes inside load_timeout");
            assert_eq!(src.tenant_calls(), 2, "the load that began before the invalidate is not served: {ou:?}");
            assert_eq!(cache.lock().tenants[&ORG].good.as_ref().map(|g| g.version.clone()).as_deref(), Some("t2"));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_last_round_load_is_read_when_it_finishes_current() {
        for ou in [OnUnavailable::Closed, OnUnavailable::Open] {
            let src = FakeSource::new();
            let cache = PolicyCache::new(src.clone(), settings());
            let floor = ready_floor(&cache, ou).await;
            src.set_delay(Duration::from_millis(300));
            let change = async {
                // Two invalidations land during the first two of three loads.
                tokio::time::sleep(Duration::from_millis(100)).await;
                cache.invalidate_all();
                tokio::time::sleep(Duration::from_millis(300)).await;
                cache.invalidate_all();
            };
            let (set, ()) = tokio::join!(cache.tenant(ORG, &floor, ou), change);
            assert!(matches!(set, Lookup::Ready(_)), "the third load is current: {ou:?}");
            assert_eq!(src.tenant_calls(), 3, "{ou:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_remembered_bad_set_is_a_compile_error_on_each_refresh() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let src = FakeSource::new();
        src.set_tenant("permit((", "bad");
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        assert!(matches!(cache.tenant(ORG, &floor, OnUnavailable::Closed).await, Lookup::Unavailable));
        snap(&snapshotter);
        tokio::time::advance(secs(30)).await;
        cache.refresh_once().await;
        let s = snap(&snapshotter);
        assert_eq!(s.loads("compile_error"), 1, "the same bad version is a compile error again");
        assert_eq!(s.loads("ok"), 1, "only the floor reload is ok");
        assert_eq!(s.gauge(names::GATEWAY_TOOL_POLICY_STALE_ENTRIES), Some(1.0));
    }
    const ENFORCE_OPEN: TenantSettings = TenantSettings {
        mode: TenantMode::Enforce,
        on_unavailable: OnUnavailable::Open,
    };

    #[tokio::test(start_paused = true)]
    async fn tenant_settings_are_none_before_the_first_load() {
        let src = FakeSource::new();
        src.set_settings(ENFORCE_OPEN);
        let cache = PolicyCache::new(src.clone(), settings());
        assert_eq!(cache.tenant_settings(ORG), None);
        assert_eq!(src.tenant_calls(), 0, "the read starts no load");
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        assert_eq!(cache.tenant_settings(ORG), None, "the floor load is not a tenant load");
        assert!(matches!(cache.tenant(ORG, &floor, OnUnavailable::Closed).await, Lookup::Ready(_)));
        assert_eq!(cache.tenant_settings(ORG), Some(ENFORCE_OPEN));
    }

    #[tokio::test(start_paused = true)]
    async fn tenant_settings_survive_a_failed_reload() {
        let src = FakeSource::new();
        src.set_settings(ENFORCE_OPEN);
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        assert!(matches!(cache.tenant(ORG, &floor, OnUnavailable::Closed).await, Lookup::Ready(_)));
        src.set_fail(true);
        cache.invalidate_all();
        assert!(matches!(cache.tenant(ORG, &floor, OnUnavailable::Closed).await, Lookup::Unavailable));
        assert_eq!(cache.tenant_settings(ORG), Some(ENFORCE_OPEN), "the last known value");
        tokio::time::advance(secs(400)).await;
        assert_eq!(cache.tenant_settings(ORG), Some(ENFORCE_OPEN), "whatever its age");
    }

    #[tokio::test(start_paused = true)]
    async fn an_evicted_tenant_has_no_settings() {
        let src = FakeSource::new();
        src.set_settings(ENFORCE_OPEN);
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        assert!(matches!(cache.tenant(ORG, &floor, OnUnavailable::Closed).await, Lookup::Ready(_)));
        tokio::time::advance(secs(3601)).await;
        cache.refresh_once().await;
        assert_eq!(cache.tenant_settings(ORG), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_bad_set_with_new_settings_replaces_the_last_good_set() {
        // A new answer with new settings and a set that does not compile: the new settings decide,
        // and the set is unavailable (I-1).
        const LOG_ONLY_OPEN: TenantSettings = TenantSettings {
            mode: TenantMode::LogOnly,
            on_unavailable: OnUnavailable::Open,
        };
        const ENFORCE_CLOSED: TenantSettings = TenantSettings {
            mode: TenantMode::Enforce,
            on_unavailable: OnUnavailable::Closed,
        };
        let src = FakeSource::new();
        src.set_settings(LOG_ONLY_OPEN);
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        ready_tenant(&cache, &floor, OnUnavailable::Open).await;
        src.set_settings(ENFORCE_CLOSED);
        src.set_tenant("permit((", "t2:enforce:closed");
        cache.invalidate_all();
        let deadline = Instant::now() + secs(5);
        let (set, got) = cache.tenant_with_settings_until(ORG, &floor, OnUnavailable::Open, deadline).await;
        assert!(matches!(set, Lookup::Unavailable), "not the last good set");
        assert_eq!(got, Some(ENFORCE_CLOSED), "the settings of the new answer");
        assert_eq!(cache.tenant_settings(ORG), Some(ENFORCE_CLOSED));
        // The bad answer is stored: the next lookup makes no source call.
        assert!(matches!(cache.tenant(ORG, &floor, OnUnavailable::Open).await, Lookup::Unavailable));
        assert_eq!(src.tenant_calls(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_bad_set_with_no_settings_keeps_the_last_good_set() {
        // The file source has no settings: a bad new version keeps the last good set.
        let src = FakeSource::new();
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        let good = ready_tenant(&cache, &floor, OnUnavailable::Closed).await;
        src.set_tenant("permit((", "bad");
        tokio::time::advance(secs(30)).await;
        cache.refresh_once().await;
        let again = ready_tenant(&cache, &floor, OnUnavailable::Closed).await;
        assert!(Arc::ptr_eq(&good, &again), "the last good set still serves");
    }

    #[tokio::test(start_paused = true)]
    async fn the_fallback_follows_the_fail_mode_of_the_stored_entry() {
        // CodeRabbit, PR 485: the caller reads its fail mode before the floor wait. A load that
        // lands after that read stores new settings. When the next reload fails, the fallback must
        // use the fail mode of the stored entry, not the caller's older value.
        const ENFORCE_CLOSED: TenantSettings = TenantSettings {
            mode: TenantMode::Enforce,
            on_unavailable: OnUnavailable::Closed,
        };
        let deadline = || Instant::now() + secs(5);
        // Closed -> Open: the caller still has `closed`, the entry says `open`. The last good set serves.
        let src = FakeSource::new();
        src.set_settings(ENFORCE_OPEN);
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        let good = ready_tenant(&cache, &floor, OnUnavailable::Closed).await;
        src.set_fail(true);
        cache.invalidate_all();
        let (set, got) = cache.tenant_with_settings_until(ORG, &floor, OnUnavailable::Closed, deadline()).await;
        assert!(matches!(&set, Lookup::Ready(t) if Arc::ptr_eq(&t.policies, &good)), "the entry's `open` keeps the last good set");
        assert_eq!(got, Some(ENFORCE_OPEN));
        // Open -> Closed: the caller still has `open`, the entry says `closed`. Nothing serves.
        let src = FakeSource::new();
        src.set_settings(ENFORCE_CLOSED);
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        ready_tenant(&cache, &floor, OnUnavailable::Closed).await;
        src.set_fail(true);
        cache.invalidate_all();
        let (set, got) = cache.tenant_with_settings_until(ORG, &floor, OnUnavailable::Open, deadline()).await;
        assert!(matches!(set, Lookup::Unavailable), "the entry's `closed` serves nothing");
        assert_eq!(got, Some(ENFORCE_CLOSED));
    }

    #[tokio::test(start_paused = true)]
    async fn the_set_and_the_settings_come_from_one_entry() {
        const LOG_ONLY_CLOSED: TenantSettings = TenantSettings {
            mode: TenantMode::LogOnly,
            on_unavailable: OnUnavailable::Closed,
        };
        let src = FakeSource::new();
        src.set_settings(ENFORCE_OPEN);
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        let deadline = || Instant::now() + secs(5);
        let (set, got) = cache.tenant_with_settings_until(ORG, &floor, OnUnavailable::Closed, deadline()).await;
        assert!(matches!(set, Lookup::Ready(_)));
        assert_eq!(got, Some(ENFORCE_OPEN), "the settings of the load that gave the set");
        // I-1: an answer with a set that does not compile gives no set and its own settings.
        src.set_settings(LOG_ONLY_CLOSED);
        src.set_tenant("permit((", "t2:log_only:closed");
        cache.invalidate_all();
        let (set, got) = cache.tenant_with_settings_until(ORG, &floor, OnUnavailable::Open, deadline()).await;
        assert!(matches!(set, Lookup::Unavailable));
        assert_eq!(got, Some(LOG_ONLY_CLOSED), "the settings of the bad answer");
        // A failed reload under `closed` serves no entry: the settings are the last known value.
        src.set_fail(true);
        cache.invalidate_all();
        let (set, got) = cache.tenant_with_settings_until(ORG, &floor, OnUnavailable::Closed, deadline()).await;
        assert!(matches!(set, Lookup::Unavailable));
        assert_eq!(got, Some(LOG_ONLY_CLOSED), "the last known value");
        assert_eq!(got, cache.tenant_settings(ORG));
    }

    const F2: &str = r#"@id("f2") forbid(principal, action == Pgs::Gateway::Action::"send_email", resource);"#;

    fn path(name: &str) -> FieldPath {
        FieldPath::new(vec![name.to_owned()]).expect("a valid path")
    }

    fn malformed(slot: &str) -> pb::HistoryCondition {
        pb::HistoryCondition {
            window_ms: 0,
            ..formerly(slot, "send", "amount")
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_floor_entry_knows_whether_it_has_conditions() {
        let src = FakeSource::new();
        let cache = PolicyCache::new(src.clone(), settings());
        assert!(!ready_floor(&cache, OnUnavailable::Closed).await.has_conditions(), "a floor with no condition");
        src.set_floor_conditions(vec![formerly("floor_a", "send", "amount")]);
        src.set_floor(FLOOR, "f2");
        cache.invalidate_all();
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        assert!(floor.has_conditions(), "a floor with one condition");
        assert_eq!(floor.conditions.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_tenant_set_with_conditions_compiles_with_its_history_policy() {
        let src = FakeSource::new();
        src.set_floor_conditions(vec![formerly("floor_a", "rm", "force")]);
        src.set_tenant_conditions(vec![formerly("t_a", "send", "amount")], &["send"]);
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        let compiled = ready_compiled(&cache, &floor, OnUnavailable::Closed).await;
        let history = &compiled.history;
        let slots: Vec<(&str, Part)> = history.conditions().iter().map(|c| (c.slot.as_str(), c.part)).collect();
        assert_eq!(slots, [("floor_a", Part::Floor), ("t_a", Part::Tenant)], "the floor first, then the tenant");
        assert_eq!(history.catalog(), &Catalog::known(["send".to_owned()]));
        assert_eq!(history.fields().paths_for("send"), [path("amount")].into_iter().collect());
        assert_eq!(history.fields().paths_for("rm"), [path("force")].into_iter().collect());
        assert!(history.read().is_some(), "a set with conditions reads the history");
        // The bundle is stored: a second lookup gives the same `Arc`s and makes no source call.
        let again = ready_compiled(&cache, &floor, OnUnavailable::Closed).await;
        assert!(Arc::ptr_eq(&again.history, &compiled.history) && Arc::ptr_eq(&again.policies, &compiled.policies));
        assert_eq!(src.tenant_calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_malformed_tenant_condition_makes_the_tenant_unavailable_and_keeps_the_settings() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let src = FakeSource::new();
        src.set_settings(ENFORCE_OPEN);
        src.set_tenant_conditions(vec![malformed("t_a")], &["send"]);
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        let (set, got) = cache.tenant_with_settings_until(ORG, &floor, OnUnavailable::Open, Instant::now() + secs(5)).await;
        assert!(matches!(set, Lookup::Unavailable), "a malformed condition is a compile failure of the set");
        assert_eq!(got, Some(ENFORCE_OPEN), "the settings of the answer stay");
        assert_eq!(cache.tenant_settings(ORG), Some(ENFORCE_OPEN));
        assert_eq!(snap(&snapshotter).loads("compile_error"), 1);
        assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Ready(_)), "the floor stays ready");
    }

    #[tokio::test(start_paused = true)]
    async fn a_malformed_floor_condition_makes_the_floor_unavailable() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let src = FakeSource::new();
        src.set_floor_conditions(vec![malformed("floor_a")]);
        let cache = PolicyCache::new(src.clone(), settings());
        assert!(matches!(cache.floor(OnUnavailable::Open).await, Lookup::Unavailable));
        assert_eq!(snap(&snapshotter).loads("compile_error"), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_tenant_slot_equal_to_a_floor_slot_makes_the_tenant_malformed_not_the_floor() {
        let src = FakeSource::new();
        src.set_floor_conditions(vec![formerly("same", "rm", "force")]);
        src.set_tenant_conditions(vec![formerly("same", "send", "amount")], &["send"]);
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        assert!(floor.has_conditions(), "the floor loads with its condition");
        assert!(matches!(cache.tenant(ORG, &floor, OnUnavailable::Open).await, Lookup::Unavailable), "the tenant set is malformed");
        assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Ready(_)), "the floor stays ready");
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_floor_with_a_clashing_slot_makes_the_stored_tenant_unavailable() {
        let src = FakeSource::new();
        src.set_tenant_conditions(vec![formerly("same", "send", "amount")], &["send"]);
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        tokio::time::advance(secs(299)).await;
        ready_compiled(&cache, &floor, OnUnavailable::Closed).await;
        // The new floor has a condition with the slot of the tenant condition. The floor expires at
        // 301 s, the tenant entry (loaded at 299 s) does not, so the tenant set is recompiled.
        src.set_floor_conditions(vec![formerly("same", "rm", "force")]);
        src.set_floor(F2, "f2");
        tokio::time::advance(secs(2)).await;
        let clashing = ready_floor(&cache, OnUnavailable::Closed).await;
        assert!(!Arc::ptr_eq(&floor.floor, &clashing.floor));
        assert!(
            matches!(cache.tenant(ORG, &clashing, OnUnavailable::Open).await, Lookup::Unavailable),
            "the recompile checks the slots again"
        );
        assert_eq!(src.tenant_calls(), 1, "a recompile, not a source call");
        // A floor with no clash compiles the stored tenant set again. The invalidation reloads the
        // tenant once; its version is unchanged, so the stored text is compiled against the new floor.
        src.set_floor_conditions(vec![formerly("other", "rm", "force")]);
        src.set_floor(FLOOR, "f3");
        cache.invalidate_all();
        let fixed = ready_floor(&cache, OnUnavailable::Closed).await;
        let compiled = ready_compiled(&cache, &fixed, OnUnavailable::Closed).await;
        let slots: Vec<&str> = compiled.history.conditions().iter().map(|c| c.slot.as_str()).collect();
        assert_eq!(slots, ["other", "same"]);
        assert_eq!(src.tenant_calls(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn the_floor_history_uses_the_last_known_catalog_and_paths_of_the_org() {
        let src = FakeSource::new();
        src.set_floor_conditions(vec![formerly("floor_a", "rm", "force")]);
        src.set_tenant_conditions(vec![formerly("t_a", "send", "amount")], &["send"]);
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        ready_compiled(&cache, &floor, OnUnavailable::Closed).await;
        // The source fails: the tenant set is unavailable under `closed`.
        src.set_fail(true);
        cache.invalidate_all();
        assert!(matches!(cache.tenant(ORG, &floor, OnUnavailable::Closed).await, Lookup::Unavailable));
        let history = cache.floor_history(ORG, &floor);
        assert_eq!(history.catalog(), &Catalog::known(["send".to_owned()]), "the last known catalog of the org");
        assert_eq!(history.fields().paths_for("send"), [path("amount")].into_iter().collect(), "the last known paths still store");
        let slots: Vec<&str> = history.conditions().iter().map(|c| c.slot.as_str()).collect();
        assert_eq!(slots, ["floor_a"], "the floor conditions only");
        assert!(history.has_part(Part::Floor) && !history.has_part(Part::Tenant));
        // An idle eviction forgets the org: the catalog is unknown again.
        tokio::time::advance(secs(3601)).await;
        cache.refresh_once().await;
        assert_eq!(cache.floor_history(ORG, &floor).catalog(), &Catalog::unknown());
    }

    #[tokio::test(start_paused = true)]
    async fn an_org_with_no_good_load_has_an_unknown_catalog() {
        let src = FakeSource::new();
        src.set_floor_conditions(vec![formerly("floor_a", "rm", "force")]);
        src.set_tenant_conditions(vec![malformed("t_a")], &["send"]);
        let cache = PolicyCache::new(src.clone(), settings());
        let floor = ready_floor(&cache, OnUnavailable::Closed).await;
        assert_eq!(cache.floor_history(ORG, &floor).catalog(), &Catalog::unknown(), "never loaded");
        assert!(matches!(cache.tenant(ORG, &floor, OnUnavailable::Closed).await, Lookup::Unavailable));
        let history = cache.floor_history(ORG, &floor);
        assert_eq!(history.catalog(), &Catalog::unknown(), "a set that does not compile is not a good load");
        assert!(history.fields().paths_for("send").is_empty(), "no tenant path is known");
        assert_eq!(history.fields().paths_for("rm"), [path("force")].into_iter().collect(), "the floor paths");
    }

    /// `formerly` with the string literal `"x"` in place of the Long literal.
    fn with_string_literal(mut c: pb::HistoryCondition) -> pb::HistoryCondition {
        use pb::{history_condition::Shape as PbShape, history_predicate::Node, literal_value::Value};
        if let Some(PbShape::Formerly(f)) = &mut c.shape
            && let Some(Node::AllOf(pair)) = f.predicate.as_mut().and_then(|p| p.node.as_mut())
            && let Some(Node::Field(field)) = pair.right.as_mut().and_then(|p| p.node.as_mut())
        {
            field.value = Some(pb::LiteralValue {
                value: Some(Value::StringValue("x".into())),
            });
        }
        c
    }

    /// The literals of the field tests of every condition of `history`.
    fn literals(history: &HistoryPolicy) -> Vec<Literal> {
        history
            .conditions()
            .iter()
            .flat_map(|c| c.shape.predicate().field_tests().into_iter().map(|(_, t)| t.literal.clone()))
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn with_digester_binds_the_digest_literals_and_keyless_gives_no_digest() {
        let make = || {
            let src = FakeSource::new();
            src.set_floor_conditions(vec![with_string_literal(formerly("floor_a", "rm", "force"))]);
            src.set_tenant_conditions(vec![with_string_literal(formerly("t_a", "send", "to"))], &["send"]);
            src
        };
        // The default cache has no key: a bound literal has no digest (reason `no_key` at
        // evaluation, plan decision D-A).
        let keyless = PolicyCache::new(make(), settings());
        let floor = ready_floor(&keyless, OnUnavailable::Closed).await;
        assert!(
            matches!(&floor.conditions[0].shape.predicate().field_tests()[0].1.literal, Literal::Unbound(_)),
            "the floor entry is never bound"
        );
        let none = literals(&ready_compiled(&keyless, &floor, OnUnavailable::Closed).await.history);
        assert_eq!(none.len(), 2);
        assert!(none.iter().all(|l| matches!(l, Literal::Digests(d) if d.is_empty())), "{none:?}");
        // With a key, the tenant history and the floor history are bound to the org.
        let key = crate::adapters::history::HistoryKey::new(secrecy::SecretString::from("0123456789abcdef0123456789abcdef-one".to_owned())).expect("a valid key");
        let digester = HmacHistoryDigester::new(Some(key), None).expect("a digester");
        let keyed = PolicyCache::new(make(), settings()).with_digester(Arc::new(digester));
        let floor = ready_floor(&keyed, OnUnavailable::Closed).await;
        let bound = literals(&ready_compiled(&keyed, &floor, OnUnavailable::Closed).await.history);
        assert_eq!(bound.len(), 2);
        assert!(bound.iter().all(|l| matches!(l, Literal::Digests(d) if d.len() == 1)), "{bound:?}");
        assert!(literals(&keyed.floor_history(ORG, &floor)).iter().all(|l| matches!(l, Literal::Digests(d) if d.len() == 1)));
    }
}
