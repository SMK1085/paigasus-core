// SPDX-License-Identifier: Apache-2.0

//! Test helpers for the request side and the decider: an in-memory policy source, settings, chat
//! request bodies and a metrics reader.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use metrics_util::debugging::{DebugValue, Snapshotter};
use paigasus_gateway_core::{OnUnavailable, TenantMode};
use uuid::Uuid;

use super::{CacheSettings, PolicyCache, RequestToolPolicy};
use crate::config::ToolPolicySettings;
use crate::domain::tool_policy::{PolicySource, PolicyText, SourceError, TenantPolicies, TenantSettings};

pub(crate) const ORG_A: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000a1);
pub(crate) const ORG_A_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";
pub(crate) const TEAM_IN_A: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:team/0190a1b2-0000-7000-8000-0000000000c3";
pub(crate) const PROJECT_IN_A: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:project/0190a1c3-0000-7000-8000-0000000000d4";
/// A principal PRN: its Cedar type is `Pgs::Iam::Principal` (controller ruling).
pub(crate) const PRINCIPAL: &str = "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004";

/// The floor of these tests: it forbids `rm`.
pub(crate) const FLOOR: &str = r#"@id("floor-rm") forbid(principal, action == Pgs::Gateway::Action::"rm", resource);"#;
pub(crate) const PERMIT_ALL: &str = r#"@id("p") permit(principal, action, resource);"#;

/// An in-memory policy source. The tenant text applies to every org.
pub(crate) struct FakeSource {
    floor: Mutex<String>,
    tenant: Mutex<(String, u64)>,
    tenant_delay: Mutex<Duration>,
    floor_delay: Mutex<Duration>,
    tenant_calls: AtomicUsize,
    /// The settings that the source returns with the tenant set. `None`: a source with no settings.
    tenant_settings: Mutex<Option<TenantSettings>>,
    /// When set, only the tenant load fails (preflight F10).
    fail_tenant: AtomicBool,
    /// When set, the floor load fails.
    fail_floor: AtomicBool,
}

impl FakeSource {
    pub(crate) fn new(floor: &str, tenant: &str) -> Arc<Self> {
        Arc::new(FakeSource {
            floor: Mutex::new(floor.to_owned()),
            tenant: Mutex::new((tenant.to_owned(), 1)),
            tenant_delay: Mutex::new(Duration::ZERO),
            floor_delay: Mutex::new(Duration::ZERO),
            tenant_calls: AtomicUsize::new(0),
            tenant_settings: Mutex::new(None),
            fail_tenant: AtomicBool::new(false),
            fail_floor: AtomicBool::new(false),
        })
    }

    /// Replace the tenant text; the version changes.
    pub(crate) fn set_tenant(&self, text: &str) {
        let mut t = self.tenant.lock().unwrap();
        *t = (text.to_owned(), t.1 + 1);
    }

    /// Replace the settings of the tenant; the version changes (the IAM token covers the settings).
    pub(crate) fn set_settings(&self, settings: Option<TenantSettings>) {
        *self.tenant_settings.lock().unwrap() = settings;
        let mut t = self.tenant.lock().unwrap();
        t.1 += 1;
    }

    /// Fail only the tenant load. The floor load still works.
    pub(crate) fn set_fail_tenant(&self, fail: bool) {
        self.fail_tenant.store(fail, Ordering::SeqCst);
    }

    /// Fail the floor load.
    pub(crate) fn set_fail_floor(&self, fail: bool) {
        self.fail_floor.store(fail, Ordering::SeqCst);
    }

    pub(crate) fn set_tenant_delay(&self, d: Duration) {
        *self.tenant_delay.lock().unwrap() = d;
    }

    pub(crate) fn set_floor_delay(&self, d: Duration) {
        *self.floor_delay.lock().unwrap() = d;
    }

    pub(crate) fn tenant_calls(&self) -> usize {
        self.tenant_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl PolicySource for FakeSource {
    async fn floor(&self) -> Result<PolicyText, SourceError> {
        if self.fail_floor.load(Ordering::SeqCst) {
            return Err(SourceError::Io("floor down".to_owned()));
        }
        let text = self.floor.lock().unwrap().clone();
        let delay = *self.floor_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        Ok(PolicyText {
            version: format!("{}", text.len()),
            text,
        })
    }

    async fn tenant(&self, _org: Uuid) -> Result<TenantPolicies, SourceError> {
        self.tenant_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_tenant.load(Ordering::SeqCst) {
            return Err(SourceError::Io("tenant down".to_owned()));
        }
        let (text, version) = self.tenant.lock().unwrap().clone();
        let delay = *self.tenant_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        Ok(TenantPolicies {
            text,
            catalog: Vec::new(),
            version: version.to_string(),
            settings: *self.tenant_settings.lock().unwrap(),
            refused: false,
        })
    }
}

/// Settings with this default mode, no org rows, `closed`, and a load timeout of 1 s.
pub(crate) fn settings(default_mode: TenantMode) -> ToolPolicySettings {
    ToolPolicySettings {
        enabled: true,
        default_mode,
        on_policy_unavailable: OnUnavailable::Closed,
        refresh: Duration::from_secs(30),
        max_stale: Duration::from_secs(300),
        idle_evict: Duration::from_secs(3600),
        load_timeout: Duration::from_secs(1),
        orgs: HashMap::new(),
        settings_from_source: false,
    }
}

/// `settings(default_mode)` for a source that returns the tenant settings (the IAM source).
pub(crate) fn settings_from_source(default_mode: TenantMode) -> ToolPolicySettings {
    ToolPolicySettings {
        settings_from_source: true,
        ..settings(default_mode)
    }
}

/// The request-side service over a new cache of `source`.
pub(crate) fn policy(source: &Arc<FakeSource>, settings: ToolPolicySettings) -> RequestToolPolicy {
    let cache = PolicyCache::new(
        Arc::clone(source) as Arc<dyn PolicySource>,
        CacheSettings {
            refresh: settings.refresh,
            max_stale: settings.max_stale,
            idle_evict: settings.idle_evict,
            load_timeout: settings.load_timeout,
        },
    );
    RequestToolPolicy::new(cache, settings)
}

/// A chat request with these function tools. `extra` is more top-level members, each with a
/// leading comma (for example `,"tool_choice":"weird"`).
pub(crate) fn chat_request(tools: &[&str], extra: &str) -> Bytes {
    let tools: Vec<String> = tools.iter().map(|n| format!(r#"{{"type":"function","function":{{"name":"{n}","parameters":{{}}}}}}"#)).collect();
    let tools = if tools.is_empty() { String::new() } else { format!(r#","tools":[{}]"#, tools.join(",")) };
    Bytes::from(format!(r#"{{"model":"m","messages":[{{"role":"user","content":"hi"}}]{tools}{extra}}}"#))
}

/// One metric: its name, its labels and its value.
type Row = (String, Vec<(String, String)>, DebugValue);

/// One drained read of a `DebuggingRecorder`.
pub(crate) struct Snap(Vec<Row>);

impl Snap {
    pub(crate) fn take(snapshotter: &Snapshotter) -> Self {
        Snap(
            snapshotter
                .snapshot()
                .into_vec()
                .into_iter()
                .map(|(k, _, _, v)| (k.key().name().to_owned(), k.key().labels().map(|l| (l.key().to_owned(), l.value().to_owned())).collect(), v))
                .collect(),
        )
    }

    fn find(&self, name: &str, labels: &[(&str, &str)]) -> Option<&DebugValue> {
        self.0
            .iter()
            .find(|(n, l, _)| n == name && l.len() == labels.len() && labels.iter().all(|(k, v)| l.iter().any(|(lk, lv)| lk == k && lv == v)))
            .map(|(_, _, v)| v)
    }

    pub(crate) fn counter(&self, name: &str, labels: &[(&str, &str)]) -> u64 {
        match self.find(name, labels) {
            Some(DebugValue::Counter(n)) => *n,
            _ => 0,
        }
    }

    /// Every sample of a histogram with no labels.
    pub(crate) fn histogram(&self, name: &str) -> Vec<f64> {
        match self.find(name, &[]) {
            Some(DebugValue::Histogram(h)) => h.iter().map(|v| v.into_inner()).collect(),
            _ => Vec::new(),
        }
    }
}
