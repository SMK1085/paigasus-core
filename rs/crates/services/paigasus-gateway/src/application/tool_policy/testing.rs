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
use paigasus_proto::paigasus::iam::v1 as pb;
use pb::{history_condition, history_predicate, literal_value};
use uuid::Uuid;

use super::{CacheSettings, PolicyCache, RequestToolPolicy};
use crate::application::decision_log::DecisionRecorder;
use crate::config::ToolPolicySettings;
use crate::domain::decision_log::{MemoryDecisionLog, OsEntropy};
use crate::domain::detectors::{BuiltInStage, DetectBatch, Detector, DetectorError, PipelineOutput};
use crate::domain::limits::SystemClock;
use crate::domain::tool_policy::{PolicySource, PolicyText, SourceError, TenantPolicies, TenantSettings};
use paigasus_kernel::detect::Finding;
use paigasus_kernel::tool_policy::history::{Digest, FieldPath, HistoryDigester, KeyId, Scalar};

pub(crate) const ORG_A: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000a1);
pub(crate) const ORG_A_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";
pub(crate) const TEAM_IN_A: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:team/0190a1b2-0000-7000-8000-0000000000c3";
pub(crate) const PROJECT_IN_A: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:project/0190a1c3-0000-7000-8000-0000000000d4";
/// A principal PRN: its Cedar type is `Pgs::Iam::Principal` (controller ruling).
pub(crate) const PRINCIPAL: &str = "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004";

/// The floor of these tests: it forbids `rm`.
pub(crate) const FLOOR: &str = r#"@id("floor-rm") forbid(principal, action == Pgs::Gateway::Action::"rm", resource);"#;
pub(crate) const PERMIT_ALL: &str = r#"@id("p") permit(principal, action, resource);"#;

/// The floor answer of [`FakeSource`]: its text, its conditions and a revision of the conditions.
#[derive(Clone)]
struct FloorFixture {
    text: String,
    conditions: Vec<pb::HistoryCondition>,
    revision: u64,
}

/// The tenant answer of [`FakeSource`], with its version. One lock holds all of it.
#[derive(Clone)]
struct TenantFixture {
    text: String,
    version: u64,
    /// The tenant catalog.
    catalog: Vec<String>,
    /// The history conditions of the tenant set (SMA-753).
    conditions: Vec<pb::HistoryCondition>,
    /// The settings that the source returns with the tenant set. `None`: a source with no settings.
    settings: Option<TenantSettings>,
}

/// An in-memory policy source. The tenant text applies to every org.
pub(crate) struct FakeSource {
    floor: Mutex<FloorFixture>,
    tenant: Mutex<TenantFixture>,
    tenant_delay: Mutex<Duration>,
    floor_delay: Mutex<Duration>,
    tenant_calls: AtomicUsize,
    /// When set, only the tenant load fails (preflight F10).
    fail_tenant: AtomicBool,
    /// When set, the floor load fails.
    fail_floor: AtomicBool,
}

/// A history digester with no key, as the gateway has with no `digest_key`: it makes no digest.
struct KeylessDigester;

impl HistoryDigester for KeylessDigester {
    fn current(&self, _org: Uuid, _path: &FieldPath, _value: &Scalar) -> Option<(KeyId, Digest)> {
        None
    }

    fn all(&self, _org: Uuid, _path: &FieldPath, _value: &Scalar) -> Vec<(KeyId, Digest)> {
        Vec::new()
    }
}

/// The keyless digester as the port that [`PolicyCache::new`] takes.
pub(crate) fn keyless() -> Arc<dyn HistoryDigester> {
    Arc::new(KeylessDigester)
}

/// The window limit that the fake source reports with every set.
pub(crate) const MAX_WINDOW_MS: u64 = 86_400_000;

/// A valid `formerly` condition: a call of `tool` with the Long argument `path` equal to 1, in the
/// session, within one minute.
pub(crate) fn formerly(slot: &str, tool: &str, path: &str) -> pb::HistoryCondition {
    let node = |n| Some(Box::new(pb::HistoryPredicate { node: Some(n) }));
    let tool = history_predicate::Node::Tool(pb::ToolMatch {
        name_key: tool.to_owned(),
        any_catalog_tool: false,
    });
    let field = history_predicate::Node::Field(pb::FieldComparison {
        path: vec![path.to_owned()],
        op: pb::ComparisonOp::Eq as i32,
        value: Some(pb::LiteralValue {
            value: Some(literal_value::Value::LongValue(1)),
        }),
    });
    let predicate = pb::HistoryPredicate {
        node: Some(history_predicate::Node::AllOf(Box::new(pb::PredicatePair { left: node(tool), right: node(field) }))),
    };
    pb::HistoryCondition {
        slot: slot.to_owned(),
        policy_id: "p".to_owned(),
        scope: pb::HistoryScope::Session as i32,
        window_ms: 60_000,
        shape: Some(history_condition::Shape::Formerly(pb::FormerlyShape { predicate: Some(predicate) })),
    }
}

/// A `sum_within` condition: the sum of the Long argument `path` over the calls of `tool` in the
/// session, within one minute, is at most `max`.
pub(crate) fn sum_within(slot: &str, tool: &str, path: &str, max: i64) -> pb::HistoryCondition {
    let predicate = pb::HistoryPredicate {
        node: Some(history_predicate::Node::Tool(pb::ToolMatch {
            name_key: tool.to_owned(),
            any_catalog_tool: false,
        })),
    };
    pb::HistoryCondition {
        shape: Some(history_condition::Shape::SumWithin(pb::SumWithinShape {
            predicate: Some(predicate),
            field_path: vec![path.to_owned()],
            op: pb::ComparisonOp::Le as i32,
            threshold: max,
        })),
        ..formerly(slot, tool, path)
    }
}

impl FakeSource {
    pub(crate) fn new(floor: &str, tenant: &str) -> Arc<Self> {
        Arc::new(FakeSource {
            floor: Mutex::new(FloorFixture {
                text: floor.to_owned(),
                conditions: Vec::new(),
                revision: 1,
            }),
            tenant: Mutex::new(TenantFixture {
                text: tenant.to_owned(),
                version: 1,
                catalog: Vec::new(),
                conditions: Vec::new(),
                settings: None,
            }),
            tenant_delay: Mutex::new(Duration::ZERO),
            floor_delay: Mutex::new(Duration::ZERO),
            tenant_calls: AtomicUsize::new(0),
            fail_tenant: AtomicBool::new(false),
            fail_floor: AtomicBool::new(false),
        })
    }

    /// Replace the floor conditions. The floor revision changes when the conditions change, so
    /// the next load reloads. The same conditions keep the revision.
    pub(crate) fn set_floor_conditions(&self, conditions: Vec<pb::HistoryCondition>) {
        let mut f = self.floor.lock().unwrap();
        if f.conditions != conditions {
            f.conditions = conditions;
            f.revision += 1;
        }
    }

    /// Replace the tenant conditions and the catalog; the version changes.
    pub(crate) fn set_tenant_conditions(&self, conditions: Vec<pb::HistoryCondition>, catalog: &[&str]) {
        let mut t = self.tenant.lock().unwrap();
        t.conditions = conditions;
        t.catalog = catalog.iter().map(|s| (*s).to_owned()).collect();
        t.version += 1;
    }

    /// Replace the tenant text; the version changes.
    pub(crate) fn set_tenant(&self, text: &str) {
        let mut t = self.tenant.lock().unwrap();
        t.text = text.to_owned();
        t.version += 1;
    }

    /// Replace the settings of the tenant; the version changes (the IAM token covers the settings).
    pub(crate) fn set_settings(&self, settings: Option<TenantSettings>) {
        let mut t = self.tenant.lock().unwrap();
        t.settings = settings;
        t.version += 1;
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
        // The answer is taken under one lock before the wait, so it holds one version.
        let f = self.floor.lock().unwrap().clone();
        let delay = *self.floor_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        Ok(PolicyText {
            version: format!("{}:{}", f.text.len(), f.revision),
            text: f.text,
            conditions: f.conditions,
            max_window_ms: MAX_WINDOW_MS,
        })
    }

    async fn tenant(&self, _org: Uuid) -> Result<TenantPolicies, SourceError> {
        self.tenant_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_tenant.load(Ordering::SeqCst) {
            return Err(SourceError::Io("tenant down".to_owned()));
        }
        // The payload and its version are taken under one lock before the wait: a fixture change
        // during the wait reaches the next load, not this answer.
        let t = self.tenant.lock().unwrap().clone();
        let delay = *self.tenant_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        Ok(TenantPolicies {
            text: t.text,
            catalog: t.catalog,
            version: t.version.to_string(),
            settings: t.settings,
            conditions: t.conditions,
            max_window_ms: MAX_WINDOW_MS,
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
    policy_with(source, settings, Arc::new(DecisionRecorder::tracing()))
}

/// A recorder over a memory log, and the log.
pub(crate) fn memory_recorder() -> (Arc<MemoryDecisionLog>, Arc<DecisionRecorder>) {
    let log = Arc::new(MemoryDecisionLog::new(false));
    let rec = Arc::new(DecisionRecorder::new(log.clone(), Arc::new(SystemClock), Arc::new(OsEntropy), None));
    (log, rec)
}

/// [`policy`] with a chosen recorder.
pub(crate) fn policy_with(source: &Arc<FakeSource>, settings: ToolPolicySettings, recorder: Arc<DecisionRecorder>) -> RequestToolPolicy {
    let cache = PolicyCache::new(
        Arc::clone(source) as Arc<dyn PolicySource>,
        CacheSettings {
            refresh: settings.refresh,
            max_stale: settings.max_stale,
            idle_evict: settings.idle_evict,
            load_timeout: settings.load_timeout,
        },
        keyless(),
    );
    RequestToolPolicy::new(cache, settings, recorder)
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

/// The findings per subject that a test detector returns.
type Found = Result<Vec<Vec<Finding>>, DetectorError>;

/// A detector that gives the same findings for every subject (SMA-855 Task 18).
pub(crate) struct FixedDetector(pub(crate) &'static str, pub(crate) Vec<Finding>);

#[async_trait]
impl Detector for FixedDetector {
    fn name(&self) -> &str {
        self.0
    }
    async fn detect(&self, batch: Arc<DetectBatch>) -> Found {
        Ok(vec![self.1.clone(); batch.subjects.len()])
    }
}

/// A detector that always fails.
pub(crate) struct ErrDetector(pub(crate) &'static str);

#[async_trait]
impl Detector for ErrDetector {
    fn name(&self) -> &str {
        self.0
    }
    async fn detect(&self, _batch: Arc<DetectBatch>) -> Found {
        Err(DetectorError::Failed)
    }
}

/// A detector that sleeps, then gives no findings.
pub(crate) struct SlowDetector(pub(crate) &'static str, pub(crate) Duration);

#[async_trait]
impl Detector for SlowDetector {
    fn name(&self) -> &str {
        self.0
    }
    async fn detect(&self, batch: Arc<DetectBatch>) -> Found {
        tokio::time::sleep(self.1).await;
        Ok(vec![Vec::new(); batch.subjects.len()])
    }
}

/// A detector that keeps the batches it gets, and gives the same findings for every subject.
pub(crate) struct RecordingDetector(pub(crate) &'static str, pub(crate) Vec<Finding>, pub(crate) Mutex<Vec<Arc<DetectBatch>>>);

impl RecordingDetector {
    pub(crate) fn new(name: &'static str, findings: Vec<Finding>) -> Arc<Self> {
        Arc::new(Self(name, findings, Mutex::new(Vec::new())))
    }

    /// The batches that the detector got, in order.
    pub(crate) fn batches(&self) -> Vec<Arc<DetectBatch>> {
        self.2.lock().unwrap().clone()
    }
}

#[async_trait]
impl Detector for RecordingDetector {
    fn name(&self) -> &str {
        self.0
    }
    async fn detect(&self, batch: Arc<DetectBatch>) -> Found {
        let n = batch.subjects.len();
        self.2.lock().unwrap().push(batch);
        Ok(vec![self.1.clone(); n])
    }
}

/// A detector that panics with the canary as its payload.
pub(crate) struct PanicDetector(pub(crate) &'static str, pub(crate) &'static str);

#[async_trait]
impl Detector for PanicDetector {
    fn name(&self) -> &str {
        self.0
    }
    async fn detect(&self, _batch: Arc<DetectBatch>) -> Found {
        std::panic::panic_any(self.1.to_owned())
    }
}

/// A built-in stage that panics with the canary as its payload.
pub(crate) struct PanicStage(pub(crate) &'static str);

impl BuiltInStage for PanicStage {
    fn run(&self, _batch: &DetectBatch) -> PipelineOutput {
        std::panic::panic_any(self.0.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_floor_version_changes_with_the_condition_payload_not_its_count() {
        let source = FakeSource::new(FLOOR, PERMIT_ALL);
        source.set_floor_conditions(vec![formerly("a", "send", "amount")]);
        let first = source.floor().await.expect("floor").version;
        // The same answer keeps its version.
        source.set_floor_conditions(vec![formerly("a", "send", "amount")]);
        assert_eq!(source.floor().await.expect("floor").version, first);
        // One condition for another, with the same count: a new version.
        source.set_floor_conditions(vec![formerly("a", "send", "limit")]);
        assert_ne!(source.floor().await.expect("floor").version, first);
    }

    #[tokio::test(start_paused = true)]
    async fn a_delayed_tenant_load_returns_the_payload_of_its_version() {
        let source = FakeSource::new(FLOOR, PERMIT_ALL);
        source.set_tenant_delay(Duration::from_secs(1));
        let load = {
            let source = Arc::clone(&source);
            tokio::spawn(async move { source.tenant(ORG_A).await.expect("tenant") })
        };
        // Let the load take its snapshot, then change the fixture while it waits.
        tokio::task::yield_now().await;
        source.set_tenant_conditions(vec![formerly("a", "send", "amount")], &["send"]);
        let answer = load.await.expect("join");
        assert_eq!((answer.version.as_str(), answer.conditions.len(), answer.catalog.len()), ("1", 0, 0));
        // The next load sees the update under the new version.
        source.set_tenant_delay(Duration::ZERO);
        let next = source.tenant(ORG_A).await.expect("tenant");
        assert_eq!((next.version.as_str(), next.conditions.len(), next.catalog.len()), ("2", 1, 1));
    }
}
