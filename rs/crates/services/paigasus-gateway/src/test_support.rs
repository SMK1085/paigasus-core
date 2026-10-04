// SPDX-License-Identifier: Apache-2.0

//! Unit-test helpers shared by `application` and `adapters::limits` (cfg(test) only).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use metrics_util::debugging::{DebugValue, Snapshotter};
use uuid::Uuid;

use crate::domain::limits::{Clock, LimitDecision, LimitPolicy, LimitStore, LimitStoreError, LimitTicket};
use crate::domain::{CallerContext, Credential};

pub(crate) const ORG_A: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000a1);
pub(crate) const ORG_A_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";
pub(crate) const TEAM_IN_A: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:team/0190a1b2-0000-7000-8000-0000000000c3";

fn value(snapshotter: &Snapshotter, name: &str, labels: &[(&str, &str)]) -> Option<DebugValue> {
    snapshotter.snapshot().into_vec().into_iter().find_map(|(key, _, _, value)| {
        let key = key.key();
        let have: Vec<(String, String)> = key.labels().map(|l| (l.key().to_owned(), l.value().to_owned())).collect();
        let same = key.name() == name && have.len() == labels.len() && labels.iter().all(|(k, v)| have.iter().any(|(hk, hv)| hk == k && hv == v));
        same.then_some(value)
    })
}

/// The value of the counter `name` with exactly `labels`, or `None` when it was never emitted.
pub(crate) fn counter(snapshotter: &Snapshotter, name: &str, labels: &[(&str, &str)]) -> Option<u64> {
    match value(snapshotter, name, labels)? {
        DebugValue::Counter(n) => Some(n),
        _ => None,
    }
}

pub(crate) fn gauge(snapshotter: &Snapshotter, name: &str, labels: &[(&str, &str)]) -> Option<f64> {
    match value(snapshotter, name, labels)? {
        DebugValue::Gauge(v) => Some(v.into_inner()),
        _ => None,
    }
}

pub(crate) fn at(rfc3339: &str) -> SystemTime {
    SystemTime::from(chrono::DateTime::parse_from_rfc3339(rfc3339).expect("a fixture instant"))
}

pub(crate) struct FixedClock(pub(crate) SystemTime);

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

/// A store that answers every check with `reply()` and records what it was asked.
pub(crate) struct ScriptedStore {
    reply: fn() -> Result<LimitDecision, LimitStoreError>,
    pub(crate) checks: AtomicUsize,
    pub(crate) orgs: Mutex<Vec<Option<Uuid>>>,
    pub(crate) charges: Mutex<Vec<u64>>,
}

impl ScriptedStore {
    pub(crate) fn new(reply: fn() -> Result<LimitDecision, LimitStoreError>) -> Arc<Self> {
        Arc::new(ScriptedStore {
            reply,
            checks: AtomicUsize::new(0),
            orgs: Mutex::new(Vec::new()),
            charges: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn charges(&self) -> Vec<u64> {
        self.charges.lock().expect("not poisoned").clone()
    }
}

#[async_trait::async_trait]
impl LimitStore for ScriptedStore {
    async fn check_and_admit(&self, _principal: &str, org: Option<Uuid>, _policy: &LimitPolicy, _now: SystemTime) -> Result<LimitDecision, LimitStoreError> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        self.orgs.lock().expect("not poisoned").push(org);
        (self.reply)()
    }

    fn charge(&self, _ticket: LimitTicket, tokens: u64, _now: SystemTime) {
        self.charges.lock().expect("not poisoned").push(tokens);
    }
}

/// An API-key caller in `scope`.
pub(crate) fn caller(scope: &str) -> CallerContext {
    CallerContext {
        principal_prn: "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0".to_owned(),
        scope_prn: scope.to_owned(),
        credential: Credential::ApiKey { key_id: "key-1".to_owned() },
    }
}
