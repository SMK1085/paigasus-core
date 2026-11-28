// SPDX-License-Identifier: Apache-2.0

//! The detector pipeline (SMA-855 § 6.6.2). It runs the built-in stage, then the webhooks at the
//! same time, then the judge, all inside the one future of its caller: no task is spawned (A8,
//! D30). It never fails. An error, a timeout, a panic of any detector (the built-in stage, a
//! webhook or the judge) and a finding that fails the D37 check each become a `detector_error`
//! finding (D22). A panic of one webhook does not drop the findings of the other webhooks.

use std::fmt;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::FutureExt;
use futures::future::join_all;
use metrics::histogram;
use paigasus_kernel::detect::{DETECTOR_BUILT_IN, DETECTOR_ERROR, DETECTOR_KERNEL, Finding, INJECTION_MARKERS, INVALID_ARGS, SECRET_IN_ARGS, UNDECLARED_TOOL, UNSEEN_HOST, detector_error};
use paigasus_observability::names;
use tokio::time::Instant;

use super::count_webhook_failure;
use crate::domain::detectors::{ADAPTER, BuiltInStage, DetectBatch, Detector, DetectorError, PipelineOutput, Side, Stage, WebhookFailure, failure_finding, valid_version};

/// The detector names that are always known (D37): the kernel, the built-in stage and the five
/// built-ins.
pub(crate) const BUILT_IN_NAMES: [&str; 7] = [DETECTOR_KERNEL, DETECTOR_BUILT_IN, UNDECLARED_TOOL, INVALID_ARGS, UNSEEN_HOST, SECRET_IN_ARGS, INJECTION_MARKERS];

/// The longest kind that the pipeline accepts (D37).
const MAX_KIND: usize = 64;

/// The `side` label of the stage timer. The `stage` label is `built_in`, `webhook` or `judge`: no
/// detector name, host or content reaches a label (A2).
fn side_label(side: Side) -> &'static str {
    match side {
        Side::Request => "request",
        Side::Response => "response",
    }
}

/// Record the time of one stage that ran, from `started` until now.
fn record_stage(side: Side, stage: &'static str, started: std::time::Instant) {
    histogram!(names::GATEWAY_TOOL_POLICY_DETECTOR_SECONDS, "side" => side_label(side), "stage" => stage).record(started.elapsed().as_secs_f64());
}

/// The timer of an async stage. It records the time on drop, so a stage that the deadline stops
/// (its future is dropped) still gives a sample of about the time it ran. It holds no task.
struct StageTimer {
    side: Side,
    stage: &'static str,
    started: Instant,
}

impl StageTimer {
    fn start(side: Side, stage: &'static str) -> Self {
        Self { side, stage, started: Instant::now() }
    }
}

impl Drop for StageTimer {
    fn drop(&mut self) {
        histogram!(names::GATEWAY_TOOL_POLICY_DETECTOR_SECONDS, "side" => side_label(self.side), "stage" => self.stage).record(self.started.elapsed().as_secs_f64());
    }
}

/// The findings per subject that a detector returned.
type Found = Result<Vec<Vec<Finding>>, DetectorError>;

/// The built-in stage, the webhooks and the judge of one gateway.
pub struct DetectorPipeline {
    built_in: Arc<dyn BuiltInStage>,
    webhooks: Vec<Arc<dyn Detector>>,
    judge: Option<Arc<dyn Detector>>,
    /// `[tool_policy.detectors] timeout_ms`: an upper bound for the webhook and judge stages of one run.
    timeout: Duration,
}

impl fmt::Debug for DetectorPipeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DetectorPipeline {{ webhooks: {}, judge: {}, timeout: {:?} }}",
            self.webhooks.len(),
            self.judge.is_some(),
            self.timeout
        )
    }
}

/// Whether a kind matches `[a-z0-9_]{1,64}` (D37).
fn valid_kind(kind: &str) -> bool {
    (1..=MAX_KIND).contains(&kind.len()) && kind.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl DetectorPipeline {
    #[must_use]
    pub fn new(built_in: Arc<dyn BuiltInStage>, webhooks: Vec<Arc<dyn Detector>>, judge: Option<Arc<dyn Detector>>, timeout: Duration) -> Self {
        Self { built_in, webhooks, judge, timeout }
    }

    /// Whether `name` is a known detector name (D37): a built-in name, a configured webhook or the
    /// configured judge.
    #[must_use]
    pub fn known_detector(&self, name: &str) -> bool {
        BUILT_IN_NAMES.contains(&name) || self.webhooks.iter().any(|d| d.name() == name) || self.judge.as_ref().is_some_and(|d| d.name() == name)
    }

    /// One vector of findings and one host set per subject. It never fails and never unwinds: each
    /// stage runs under `catch_unwind` (D22).
    ///
    /// The webhook and judge stages end at `min(deadline, run start + timeout)`. The built-in stage
    /// is synchronous, so the deadline cannot stop it (§ 6.10 bounds its work).
    pub async fn run(&self, batch: DetectBatch, deadline: Instant) -> PipelineOutput {
        let run_start = Instant::now();
        let n = batch.subjects.len();
        let stage_start = std::time::Instant::now();
        let built = match AssertUnwindSafe(async { self.built_in.run(&batch) }).catch_unwind().await {
            Ok(out) if out.findings.len() == n && out.hosts.len() == n => Ok(out),
            Ok(_) => Err(DetectorError::WrongShape),
            // The payload is dropped unread: it can hold content (A2).
            Err(_payload) => {
                tracing::warn!(stage = "built_in", "a detector stage panicked");
                Err(DetectorError::Panicked)
            }
        };
        record_stage(batch.side, "built_in", stage_start);
        let mut out = match built {
            Ok(out) => PipelineOutput {
                findings: out.findings.into_iter().map(|f| self.check(DETECTOR_BUILT_IN, f)).collect(),
                hosts: out.hosts,
                stages: Vec::new(),
            },
            Err(e) => PipelineOutput {
                findings: failed_built_in(e, n),
                hosts: vec![None; n],
                stages: Vec::new(),
            },
        };
        // The stage travels with each finding, so the metric label of a webhook finding is
        // `webhook` whatever detector name it gives (D37 accepts `kernel`).
        out.stages = out.findings.iter().map(|f| vec![Stage::BuiltIn; f.len()]).collect();
        // The webhooks and the judge get the call subjects only: never a result text (D40).
        let calls: Vec<usize> = (0..n).filter(|&i| batch.subjects[i].is_call()).collect();
        if calls.is_empty() || (self.webhooks.is_empty() && self.judge.is_none()) {
            return out;
        }
        let late = self.late_stages(&batch, &calls, &out.findings, (run_start + self.timeout).min(deadline)).await;
        // `late` holds one entry per webhook, then one for the judge.
        for (k, found_by) in late.into_iter().enumerate() {
            let stage = if k < self.webhooks.len() { Stage::Webhook } else { Stage::Judge };
            for (j, found) in found_by.into_iter().enumerate() {
                out.stages[calls[j]].extend(std::iter::repeat_n(stage, found.len()));
                out.findings[calls[j]].extend(found);
            }
        }
        out
    }

    /// The webhooks at the same time, then the judge, inside one hard deadline (D30, D35). The result
    /// holds one entry per webhook, then one for the judge, each with one vector per call subject.
    async fn late_stages(&self, batch: &DetectBatch, calls: &[usize], built: &[Vec<Finding>], cut: Instant) -> Vec<Vec<Vec<Finding>>> {
        let m = calls.len();
        let side = batch.side;
        let call_batch = Arc::new(DetectBatch {
            earlier: Vec::new(),
            caller: batch.caller.clone(),
            deadline: cut,
            webhook_bodies: batch.webhook_bodies.clone(),
            ..DetectBatch::new(
                batch.side,
                batch.org,
                batch.request_id.clone(),
                calls.iter().map(|&i| batch.subjects[i].clone()).collect(),
                batch.seen_hosts.clone(),
            )
        });
        // A slot holds the checked findings of a detector that finished. An empty slot at the
        // deadline is a detector that did not finish.
        let hooks: Vec<Mutex<Option<Vec<Vec<Finding>>>>> = self.webhooks.iter().map(|_| Mutex::new(None)).collect();
        let judged: Mutex<Option<Vec<Vec<Finding>>>> = Mutex::new(None);
        let stages = async {
            join_all(self.webhooks.iter().zip(&hooks).map(|(d, slot)| {
                let b = Arc::clone(&call_batch);
                async move {
                    let _timer = StageTimer::start(side, "webhook");
                    let got = guarded("webhook", async move { d.detect(b).await }).await;
                    *lock(slot) = Some(self.accept(Stage::Webhook, d.as_ref(), got, m));
                }
            }))
            .await;
            if let Some(judge) = &self.judge {
                let earlier = (0..m)
                    .map(|j| {
                        let mut e = built[calls[j]].clone();
                        for slot in &hooks {
                            e.extend(lock(slot).as_ref().map(|f| f[j].clone()).unwrap_or_default());
                        }
                        e
                    })
                    .collect();
                let judge_batch = Arc::new(DetectBatch {
                    earlier,
                    caller: call_batch.caller.clone(),
                    deadline: call_batch.deadline,
                    webhook_bodies: call_batch.webhook_bodies.clone(),
                    ..DetectBatch::new(
                        call_batch.side,
                        call_batch.org,
                        call_batch.request_id.clone(),
                        call_batch.subjects.clone(),
                        call_batch.seen_hosts.clone(),
                    )
                });
                let _timer = StageTimer::start(side, "judge");
                let got = guarded("judge", async move { judge.detect(judge_batch).await }).await;
                *lock(&judged) = Some(self.accept(Stage::Judge, judge.as_ref(), got, m));
            }
        };
        // At the deadline the future of each unfinished detector is dropped.
        let _ = tokio::time::timeout_at(cut, stages).await;
        let mut late: Vec<Vec<Vec<Finding>>> = self
            .webhooks
            .iter()
            .zip(&hooks)
            .map(|(d, slot)| lock(slot).take().unwrap_or_else(|| failed(Stage::Webhook, d.as_ref(), DetectorError::Timeout, m)))
            .collect();
        if let Some(judge) = &self.judge {
            late.push(lock(&judged).take().unwrap_or_else(|| failed(Stage::Judge, judge.as_ref(), DetectorError::Timeout, m)));
        }
        late
    }

    /// The checked findings of one detector of `stage`, with `m` subjects.
    fn accept(&self, stage: Stage, d: &dyn Detector, got: Found, m: usize) -> Vec<Vec<Finding>> {
        match got {
            Ok(found) if found.len() == m => found.into_iter().map(|f| self.check_late(stage, d, f)).collect(),
            Ok(_) => failed(stage, d, DetectorError::WrongShape, m),
            Err(e) => failed(stage, d, e, m),
        }
    }

    /// The D37 check on the findings of one subject of the built-in stage: every finding with a bad
    /// kind or an unknown detector name is dropped, and together they give one `detector_error`.
    fn check(&self, name: &str, found: Vec<Finding>) -> Vec<Finding> {
        let before = found.len();
        let mut kept: Vec<Finding> = found.into_iter().filter(|f| valid_kind(&f.kind) && self.known_detector(&f.detector)).collect();
        if kept.len() < before {
            kept.push(detector_error(name));
        }
        kept
    }

    /// The strict check on the findings of one subject of a webhook or the judge (D14, D37, P7). A
    /// webhook finding must carry the name of its detector, a valid kind that the detector declares
    /// and a valid version. A webhook `detector_error` is kept only when it is the adapter's own
    /// failure finding. The judge keeps the D37 rule and the version rule. All dropped findings
    /// together give one `check` failure finding with the failure severity of the detector.
    fn check_late(&self, stage: Stage, d: &dyn Detector, found: Vec<Finding>) -> Vec<Finding> {
        let before = found.len();
        let mut kept: Vec<Finding> = found
            .into_iter()
            .filter(|f| match stage {
                Stage::Webhook => webhook_finding_ok(d, f),
                _ => valid_kind(&f.kind) && self.known_detector(&f.detector) && valid_version(&f.version),
            })
            .collect();
        if kept.len() < before {
            kept.push(failure_finding(d.name(), d.failure_severity(), WebhookFailure::Check));
            if stage == Stage::Webhook {
                count_webhook_failure(d.name(), WebhookFailure::Check);
            }
        }
        kept
    }
}

/// Whether one finding of a webhook passes the strict check (P7).
fn webhook_finding_ok(d: &dyn Detector, f: &Finding) -> bool {
    if f.detector != d.name() || !valid_kind(&f.kind) || !valid_version(&f.version) {
        return false;
    }
    if f.kind == DETECTOR_ERROR {
        // The failure finding of the adapter itself (P7): an adapter reason, the failure severity
        // of the detector, no evidence. Any other `detector_error` is a forgery.
        return ADAPTER.iter().any(|r| r.as_str() == f.version) && f.severity == d.failure_severity() && f.evidence.is_empty();
    }
    d.kinds().is_none_or(|k| k.contains(&f.kind))
}

/// Poll the future of one async detector of `stage` and catch a panic (D22). A panic gives
/// `Panicked` and one event that names the stage only. The payload is dropped unread (A2).
async fn guarded(stage: &'static str, detect: impl Future<Output = Found>) -> Found {
    match AssertUnwindSafe(detect).catch_unwind().await {
        Ok(got) => got,
        Err(_payload) => {
            tracing::warn!(stage, "a detector stage panicked");
            Err(DetectorError::Panicked)
        }
    }
}

/// The reason that a [`DetectorError`] gives.
fn reason_of(error: DetectorError) -> WebhookFailure {
    match error {
        DetectorError::Failed => WebhookFailure::Error,
        DetectorError::WrongShape => WebhookFailure::Shape,
        DetectorError::Timeout => WebhookFailure::Deadline,
        DetectorError::Panicked => WebhookFailure::Panic,
    }
}

/// One `detector_error` of the built-in stage per subject.
fn failed_built_in(error: DetectorError, n: usize) -> Vec<Vec<Finding>> {
    if error != DetectorError::Panicked {
        tracing::warn!(stage = "built_in", error = ?error, "a detector gave no findings");
    }
    vec![vec![detector_error(DETECTOR_BUILT_IN)]; n]
}

/// One failure finding of `d` per subject, with its failure severity (D16). A webhook counts the
/// failure once per subject. The event names the stage and the error variant, never the detector
/// name or any content.
fn failed(stage: Stage, d: &dyn Detector, error: DetectorError, n: usize) -> Vec<Vec<Finding>> {
    let reason = reason_of(error);
    if error != DetectorError::Panicked {
        tracing::warn!(stage = if stage == Stage::Webhook { "webhook" } else { "judge" }, error = ?error, "a detector gave no findings");
    }
    if stage == Stage::Webhook {
        for _ in 0..n {
            count_webhook_failure(d.name(), reason);
        }
    }
    vec![vec![failure_finding(d.name(), d.failure_severity(), reason)]; n]
}

/// The value of a slot. A poisoned lock still holds a usable value.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::detectors::BuiltInDetector;
    use crate::application::tool_policy::testing::{
        DeadlineDetector, DeclaredDetector, EagerPanicDetector, ErrDetector, FixedDetector, PanicDetector, PanicStage, RecordingDetector, SeveritySlowDetector, SlowDetector,
    };
    use crate::domain::detectors::{ADAPTER, WebhookFailure, failure_finding};
    use crate::domain::detectors::{Caller, Subject};
    use metrics_util::debugging::DebuggingRecorder;
    use paigasus_kernel::detect::{DETECTOR_ERROR, Declared, Severity, ToolInput};
    use paigasus_kernel::tool_policy::ToolInvocation;
    use paigasus_logging::test_support::capture_logs;
    use paigasus_observability::names;

    const CANARY: &str = "canary-5d1a-panic-payload";

    fn aws() -> String {
        ["AKIA", "ABCDEFGHIJKLMNOP"].concat()
    }

    fn call(index: usize, args: &str) -> Subject {
        Subject::Call {
            index,
            invocation: ToolInvocation::new("t", ToolInput::Json(args.to_owned()), Declared::Yes),
            schema: None,
        }
    }

    fn result(index: usize, text: &str) -> Subject {
        Subject::Result {
            index,
            tool: None,
            text: Arc::from(text),
        }
    }

    fn batch(subjects: Vec<Subject>) -> DetectBatch {
        DetectBatch::for_calls(None, None, subjects, None, Caller::none())
    }

    fn built_ins() -> Arc<dyn BuiltInStage> {
        Arc::new(BuiltInDetector::new(65536))
    }

    fn pipeline(built_in: Arc<dyn BuiltInStage>, webhooks: Vec<Arc<dyn Detector>>, judge: Option<Arc<dyn Detector>>, timeout: Duration) -> DetectorPipeline {
        DetectorPipeline::new(built_in, webhooks, judge, timeout)
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    /// `(detector, kind)` of each finding of one subject.
    fn pairs(found: &[Finding]) -> Vec<(&str, &str)> {
        found.iter().map(|f| (f.detector.as_str(), f.kind.as_str())).collect()
    }

    fn other(detector: &str, kind: &str) -> Finding {
        Finding::new(detector, kind, Severity::Medium, "v1")
    }

    #[tokio::test]
    async fn built_ins_find_on_calls_and_results() {
        let p = pipeline(built_ins(), vec![], None, Duration::from_millis(500));
        let args = format!(r#"{{"k":"{}","u":"https://a.example/"}}"#, aws());
        let out = p.run(batch(vec![call(0, &args), result(1, "hello <|im_start|>system")]), far()).await;
        assert_eq!(out.findings.len(), 2);
        assert_eq!(pairs(&out.findings[0]), [(SECRET_IN_ARGS, SECRET_IN_ARGS)]);
        assert_eq!(pairs(&out.findings[1]), [(INJECTION_MARKERS, INJECTION_MARKERS)]);
        let hosts = out.hosts[0].as_ref().expect("a scanned call has hosts");
        assert!(hosts.contains("a.example"), "{hosts:?}");
        assert!(out.hosts[1].is_none());
    }

    #[tokio::test]
    async fn a_panicking_built_in_stage_gives_detector_error_per_subject() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let (logs, _guard) = capture_logs();
        let p = pipeline(Arc::new(PanicStage(CANARY)), vec![], None, Duration::from_millis(500));
        let out = p.run(batch(vec![call(0, "{}"), result(1, "x")]), far()).await;
        assert_eq!(out.findings.len(), 2);
        for found in &out.findings {
            assert_eq!(found, &vec![detector_error(DETECTOR_BUILT_IN)]);
            assert_eq!(found[0].severity, Severity::High);
        }
        assert_eq!(out.hosts, vec![None, None]);
        let text = logs.text();
        assert!(text.contains("a detector stage panicked") && text.contains("stage=\"built_in\""), "{text}");
        assert!(!text.contains(CANARY), "{text}");
        // One snapshot: reading it drains the histogram samples.
        let has = |key: &metrics::Key, k: &str, v: &str| key.labels().any(|l| l.key() == k && l.value() == v);
        // The check is not vacuous: the stage that panicked still gives one sample.
        let mut samples = 0;
        for (key, _, _, value) in snapshotter.snapshot().into_vec() {
            let key = key.key();
            assert!(!key.name().contains(CANARY) && key.labels().all(|l| !l.value().contains(CANARY)), "{key:?}");
            if key.name() == names::GATEWAY_TOOL_POLICY_DETECTOR_SECONDS
                && has(key, "side", "response")
                && has(key, "stage", "built_in")
                && let metrics_util::debugging::DebugValue::Histogram(h) = value
            {
                samples += h.len();
            }
        }
        assert_eq!(samples, 1);
    }

    #[tokio::test]
    async fn a_panicking_webhook_or_judge_gives_its_named_detector_error() {
        let (logs, _guard) = capture_logs();
        let fixed: Arc<dyn Detector> = Arc::new(FixedDetector("hook_fixed", vec![other("hook_fixed", "fixed_kind")]));
        let panicking: Arc<dyn Detector> = Arc::new(PanicDetector("hook_panic", CANARY));
        let judge: Arc<dyn Detector> = Arc::new(PanicDetector("judge", CANARY));
        let p = pipeline(built_ins(), vec![panicking, fixed], Some(judge), Duration::from_millis(500));
        let out = p.run(batch(vec![call(0, "{}"), result(1, "x"), call(2, "{}")]), far()).await;
        let want = [("hook_panic", DETECTOR_ERROR), ("hook_fixed", "fixed_kind"), ("judge", DETECTOR_ERROR)];
        assert_eq!(pairs(&out.findings[0]), want);
        assert_eq!(pairs(&out.findings[1]), Vec::<(&str, &str)>::new());
        assert_eq!(pairs(&out.findings[2]), want);
        let text = logs.text();
        assert!(text.contains("stage=\"webhook\"") && text.contains("stage=\"judge\""), "{text}");
        assert!(!text.contains(CANARY), "{text}");
    }

    /// A built-in stage that gives no output, whatever the batch.
    struct EmptyStage;

    impl BuiltInStage for EmptyStage {
        fn run(&self, _batch: &DetectBatch) -> PipelineOutput {
            PipelineOutput::default()
        }
    }

    #[tokio::test]
    async fn a_built_in_stage_of_the_wrong_shape_gives_detector_error() {
        let p = pipeline(Arc::new(EmptyStage), vec![], None, Duration::from_millis(500));
        let out = p.run(batch(vec![call(0, "{}")]), far()).await;
        assert_eq!(out.findings, vec![vec![detector_error(DETECTOR_BUILT_IN)]]);
        assert_eq!(out.hosts, vec![None]);
    }

    #[tokio::test]
    async fn an_erroring_detector_gives_its_named_detector_error() {
        let p = pipeline(built_ins(), vec![Arc::new(ErrDetector("hook_a"))], None, Duration::from_millis(500));
        let out = p.run(batch(vec![call(0, "{}"), call(1, "{}")]), far()).await;
        assert_eq!(out.findings, vec![vec![failure_finding("hook_a", Severity::High, WebhookFailure::Error)]; 2]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_webhook_is_cut_at_the_deadline() {
        let fast: Arc<dyn Detector> = Arc::new(FixedDetector("hook_fast", vec![other("hook_fast", "fast_kind")]));
        let slow: Arc<dyn Detector> = Arc::new(SlowDetector("hook_slow", Duration::from_secs(10)));
        let p = pipeline(built_ins(), vec![fast, slow], None, Duration::from_millis(500));
        let start = Instant::now();
        let out = p.run(batch(vec![call(0, "{}")]), far()).await;
        assert_eq!(start.elapsed(), Duration::from_millis(500));
        assert_eq!(pairs(&out.findings[0]), [("hook_fast", "fast_kind"), ("hook_slow", DETECTOR_ERROR)]);
        assert_eq!(out.findings[0][1], failure_finding("hook_slow", Severity::High, WebhookFailure::Deadline));
    }

    /// The samples of the stage timer of `stage`, side `response`. One snapshot drains them.
    fn stage_samples(snapshotter: &metrics_util::debugging::Snapshotter, stage: &str) -> Vec<f64> {
        let mut found = Vec::new();
        for (key, _, _, value) in snapshotter.snapshot().into_vec() {
            let key = key.key();
            let has = |k: &str, v: &str| key.labels().any(|l| l.key() == k && l.value() == v);
            if key.name() == names::GATEWAY_TOOL_POLICY_DETECTOR_SECONDS
                && has("side", "response")
                && has("stage", stage)
                && let metrics_util::debugging::DebugValue::Histogram(h) = value
            {
                found.extend(h.iter().map(|v| v.into_inner()));
            }
        }
        found
    }

    fn near_deadline(samples: &[f64]) -> bool {
        samples.len() == 1 && (0.49..0.6).contains(&samples[0])
    }

    /// A webhook that the deadline stops still gives one sample of about the timeout.
    #[tokio::test(start_paused = true)]
    async fn a_webhook_cut_at_the_deadline_still_gives_a_sample() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let slow: Arc<dyn Detector> = Arc::new(SlowDetector("hook_slow", Duration::from_secs(10)));
        let p = pipeline(built_ins(), vec![slow], None, Duration::from_millis(500));
        p.run(batch(vec![call(0, "{}")]), far()).await;
        let samples = stage_samples(&snapshotter, "webhook");
        assert!(near_deadline(&samples), "{samples:?}");
    }

    /// A judge that runs at the deadline gives one sample. A judge that never started gives none.
    #[tokio::test(start_paused = true)]
    async fn a_judge_at_the_deadline_gives_a_sample_and_a_never_started_judge_gives_none() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let judge: Arc<dyn Detector> = Arc::new(SlowDetector("judge", Duration::from_secs(10)));
        let p = pipeline(built_ins(), vec![], Some(judge), Duration::from_millis(500));
        p.run(batch(vec![call(0, "{}")]), far()).await;
        let samples = stage_samples(&snapshotter, "judge");
        assert!(near_deadline(&samples), "{samples:?}");
        // The webhooks use the whole budget, so the judge never starts and has no timer.
        let slow: Arc<dyn Detector> = Arc::new(SlowDetector("hook_slow", Duration::from_secs(10)));
        let judge: Arc<dyn Detector> = Arc::new(SlowDetector("judge", Duration::from_secs(10)));
        let p = pipeline(built_ins(), vec![slow], Some(judge), Duration::from_millis(500));
        p.run(batch(vec![call(0, "{}")]), far()).await;
        // Each snapshot drains every histogram, so read the webhook samples first, from one snapshot.
        let hook = stage_samples(&snapshotter, "webhook");
        assert!(near_deadline(&hook), "{hook:?}");
        // The drained snapshot is empty, so check the judge with a second run on its own.
        let slow: Arc<dyn Detector> = Arc::new(SlowDetector("hook_slow", Duration::from_secs(10)));
        let judge: Arc<dyn Detector> = Arc::new(SlowDetector("judge", Duration::from_secs(10)));
        let p = pipeline(built_ins(), vec![slow], Some(judge), Duration::from_millis(500));
        p.run(batch(vec![call(0, "{}")]), far()).await;
        assert!(stage_samples(&snapshotter, "judge").is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn the_earlier_deadline_wins() {
        let slow: Arc<dyn Detector> = Arc::new(SlowDetector("hook_slow", Duration::from_secs(10)));
        let p = pipeline(built_ins(), vec![slow], None, Duration::from_millis(500));
        let start = Instant::now();
        let out = p.run(batch(vec![call(0, "{}")]), start + Duration::from_millis(100)).await;
        assert_eq!(start.elapsed(), Duration::from_millis(100));
        assert_eq!(out.findings, vec![vec![failure_finding("hook_slow", Severity::High, WebhookFailure::Deadline)]]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_judge_that_the_webhooks_leave_no_time_for_gives_detector_error() {
        let slow: Arc<dyn Detector> = Arc::new(SlowDetector("hook_slow", Duration::from_secs(10)));
        let judge = RecordingDetector::new("judge", vec![]);
        let p = pipeline(built_ins(), vec![slow], Some(judge.clone() as Arc<dyn Detector>), Duration::from_millis(500));
        let out = p.run(batch(vec![call(0, "{}")]), far()).await;
        assert_eq!(pairs(&out.findings[0]), [("hook_slow", DETECTOR_ERROR), ("judge", DETECTOR_ERROR)]);
        assert!(judge.batches().is_empty(), "the judge never started");
    }

    #[tokio::test]
    async fn webhooks_get_call_subjects_only() {
        let hook = RecordingDetector::new("hook_a", vec![other("hook_a", "hook_kind")]);
        let judge = RecordingDetector::new("judge", vec![other("judge", "judge_kind")]);
        let p = pipeline(
            built_ins(),
            vec![hook.clone() as Arc<dyn Detector>],
            Some(judge.clone() as Arc<dyn Detector>),
            Duration::from_millis(500),
        );
        let secret = format!(r#"{{"k":"{}"}}"#, aws());
        let out = p.run(batch(vec![call(0, &secret), result(1, "result text <|im_start|>"), call(2, "{}")]), far()).await;
        for got in [hook.batches(), judge.batches()] {
            assert_eq!(got.len(), 1);
            let subjects = &got[0].subjects;
            assert!(subjects.iter().all(Subject::is_call), "{subjects:?}");
            let indices: Vec<usize> = subjects
                .iter()
                .map(|s| match s {
                    Subject::Call { index, .. } | Subject::Result { index, .. } => *index,
                })
                .collect();
            assert_eq!(indices, [0, 2]);
        }
        assert!(hook.batches()[0].earlier.is_empty());
        let earlier = &judge.batches()[0].earlier;
        assert_eq!(
            earlier.iter().map(|e| pairs(e)).collect::<Vec<_>>(),
            [vec![(SECRET_IN_ARGS, SECRET_IN_ARGS), ("hook_a", "hook_kind")], vec![("hook_a", "hook_kind")]]
        );
        assert_eq!(pairs(&out.findings[0]), [(SECRET_IN_ARGS, SECRET_IN_ARGS), ("hook_a", "hook_kind"), ("judge", "judge_kind")]);
        assert_eq!(pairs(&out.findings[1]), [(INJECTION_MARKERS, INJECTION_MARKERS)], "a result gets no webhook or judge finding");
        assert_eq!(pairs(&out.findings[2]), [("hook_a", "hook_kind"), ("judge", "judge_kind")]);
        // The stage of each finding travels with it, in the shape of the findings.
        use crate::domain::detectors::Stage;
        assert_eq!(
            out.stages,
            [vec![Stage::BuiltIn, Stage::Webhook, Stage::Judge], vec![Stage::BuiltIn], vec![Stage::Webhook, Stage::Judge]]
        );
    }

    #[tokio::test]
    async fn a_batch_with_no_call_does_not_reach_the_webhooks() {
        let hook = RecordingDetector::new("hook_a", vec![]);
        let p = pipeline(built_ins(), vec![hook.clone() as Arc<dyn Detector>], None, Duration::from_millis(500));
        let out = p.run(batch(vec![result(0, "plain")]), far()).await;
        assert_eq!(out.findings, vec![Vec::<Finding>::new()]);
        assert!(hook.batches().is_empty());
    }

    #[tokio::test]
    async fn a_bad_kind_or_an_unknown_detector_name_is_replaced() {
        let long = "k".repeat(65);
        let bad = [
            other("hook_a", "Bad-Kind"),
            other("hook_a", &long),
            other("hook_a", ""),
            other("hook_a", "a,b"),
            other("hook_a", "a:b"),
            other("hook_a", "a\nb"),
            other("hook_a", &format!("sk_{}", aws())),
            other("hook_a", "k\u{e9}"),
            other("nobody", "fine_kind"),
            other("kernel", "fine_kind"),
        ];
        for one in &bad {
            let p = pipeline(built_ins(), vec![Arc::new(FixedDetector("hook_a", vec![one.clone()]))], None, Duration::from_millis(500));
            let out = p.run(batch(vec![call(0, "{}")]), far()).await;
            assert_eq!(out.findings, vec![vec![failure_finding("hook_a", Severity::High, WebhookFailure::Check)]], "{one:?}");
        }
        let mut all = bad.to_vec();
        all.push(other("hook_a", "fine_kind"));
        all.push(other("hook_a", &"k".repeat(64)));
        let p = pipeline(built_ins(), vec![Arc::new(FixedDetector("hook_a", all))], None, Duration::from_millis(500));
        let out = p.run(batch(vec![call(0, "{}")]), far()).await;
        let long64 = "k".repeat(64);
        assert_eq!(
            pairs(&out.findings[0]),
            [("hook_a", "fine_kind"), ("hook_a", long64.as_str()), ("hook_a", DETECTOR_ERROR)],
            "one detector_error for all bad findings"
        );
        assert_eq!(out.findings[0][2].version, "check");
    }

    #[test]
    fn the_known_detector_names() {
        let p = pipeline(built_ins(), vec![Arc::new(ErrDetector("hook_a"))], None, Duration::from_millis(500));
        for name in ["kernel", "built_in", "undeclared_tool", "invalid_args", "unseen_host", "secret_in_args", "injection_markers", "hook_a"] {
            assert!(p.known_detector(name), "{name}");
        }
        for name in ["nobody", "judge", "webhook", "", "Kernel"] {
            assert!(!p.known_detector(name), "{name}");
        }
        let with_judge = pipeline(built_ins(), vec![], Some(Arc::new(ErrDetector("judge"))), Duration::from_millis(500));
        assert!(with_judge.known_detector("judge"));
        assert_eq!(format!("{with_judge:?}"), "DetectorPipeline { webhooks: 0, judge: true, timeout: 500ms }");
    }

    #[test]
    fn the_run_future_is_send() {
        fn send<T: Send>(_: &T) {}
        let p = pipeline(built_ins(), vec![], None, Duration::from_millis(500));
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let _enter = rt.enter();
        let fut = p.run(batch(vec![]), Instant::now());
        send(&fut);
    }

    #[tokio::test]
    async fn a_webhook_finding_under_a_built_in_kind_is_replaced() {
        let hook: Arc<dyn Detector> = Arc::new(DeclaredDetector {
            name: "hook_a",
            kinds: ["pii_email".to_owned()].into(),
            severity: Severity::Low,
            found: vec![Finding::new("hook_a", SECRET_IN_ARGS, Severity::Low, "h1")],
        });
        let p = pipeline(built_ins(), vec![hook], None, Duration::from_millis(500));
        let args = format!(r#"{{"k":"{}"}}"#, aws());
        let out = p.run(batch(vec![call(0, &args)]), far()).await;
        assert_eq!(out.findings[0], vec![out.findings[0][0].clone(), failure_finding("hook_a", Severity::Low, WebhookFailure::Check)]);
        assert_eq!(pairs(&out.findings[0])[0], (SECRET_IN_ARGS, SECRET_IN_ARGS), "the built-in finding stays");
        assert_eq!(out.findings[0][0].severity, Severity::High);
    }

    fn two_sided(severity: Severity, found: Vec<Finding>) -> Vec<Finding> {
        let hook: Arc<dyn Detector> = Arc::new(DeclaredDetector {
            name: "hook_a",
            kinds: ["pii_email".to_owned()].into(),
            severity,
            found,
        });
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        rt.block_on(pipeline(built_ins(), vec![hook], None, Duration::from_millis(500)).run(batch(vec![call(0, "{}")]), far()))
            .findings
            .remove(0)
    }

    /// P7: the failure finding of the adapter itself survives the strict check. Any other
    /// `detector_error` of a webhook is a `check` failure.
    #[test]
    fn a_webhook_failure_finding_of_the_adapter_is_kept_and_a_forged_one_is_replaced() {
        for reason in ADAPTER {
            let own = failure_finding("hook_a", Severity::Low, reason);
            assert_eq!(two_sided(Severity::Low, vec![own.clone()]), vec![own], "{reason:?}");
        }
        let check = failure_finding("hook_a", Severity::Low, WebhookFailure::Check);
        let own = failure_finding("hook_a", Severity::Low, WebhookFailure::Transport);
        let forged = [
            // A pipeline reason, not an adapter reason.
            failure_finding("hook_a", Severity::Low, WebhookFailure::Panic),
            // The wrong severity.
            failure_finding("hook_a", Severity::High, WebhookFailure::Transport),
            // Evidence.
            own.clone().with_evidence(paigasus_kernel::detect::Evidence::host("r", None, "h.example".to_owned())),
            // An empty version, as the built-in `detector_error` has.
            detector_error("hook_a"),
            // Another detector name.
            failure_finding("hook_b", Severity::Low, WebhookFailure::Transport),
            // The pipeline's own `check` reason is not an adapter reason.
            failure_finding("hook_a", Severity::Low, WebhookFailure::Check),
        ];
        for f in forged {
            assert_eq!(two_sided(Severity::Low, vec![f.clone()]), vec![check.clone()], "{f:?}");
        }
    }

    #[tokio::test]
    async fn a_detector_that_panics_before_its_future_gives_its_failure_severity() {
        let eager: Arc<dyn Detector> = Arc::new(EagerPanicDetector("hook_eager", CANARY));
        let fixed: Arc<dyn Detector> = Arc::new(FixedDetector("hook_fixed", vec![other("hook_fixed", "fixed_kind")]));
        let (logs, _guard) = capture_logs();
        let out = pipeline(built_ins(), vec![eager, fixed], None, Duration::from_millis(500)).run(batch(vec![call(0, "{}")]), far()).await;
        assert_eq!(pairs(&out.findings[0]), [("hook_eager", DETECTOR_ERROR), ("hook_fixed", "fixed_kind")]);
        // The detector has `failure_severity = Low`, so `Low` proves that D16 picks it (a default
        // `High` would also pass for a guard that ignores it).
        assert_eq!((out.findings[0][0].severity, out.findings[0][0].version.as_str()), (Severity::Low, "panic"));
        assert!(!logs.text().contains(CANARY));
    }

    #[tokio::test(start_paused = true)]
    async fn the_backstop_cut_uses_the_failure_severity() {
        let slow: Arc<dyn Detector> = Arc::new(SeveritySlowDetector("hook_slow", Duration::from_secs(10), Severity::Low));
        let out = pipeline(built_ins(), vec![slow], None, Duration::from_millis(500)).run(batch(vec![call(0, "{}")]), far()).await;
        assert_eq!(out.findings, vec![vec![failure_finding("hook_slow", Severity::Low, WebhookFailure::Deadline)]]);
        assert_eq!((out.findings[0][0].severity, out.findings[0][0].version.as_str()), (Severity::Low, "deadline"));
    }

    #[tokio::test(start_paused = true)]
    async fn an_adapter_that_honours_the_deadline_returns_per_call_results() {
        let d: Arc<dyn Detector> = Arc::new(DeadlineDetector { name: "hook_dl", answered: 2 });
        let start = Instant::now();
        let out = pipeline(built_ins(), vec![d], None, Duration::from_millis(500))
            .run(batch(vec![call(0, "{}"), call(1, "{}"), call(2, "{}")]), far())
            .await;
        assert_eq!(out.findings[0].iter().map(|f| f.version.as_str()).collect::<Vec<_>>(), ["ok"]);
        assert_eq!(out.findings[1].iter().map(|f| f.version.as_str()).collect::<Vec<_>>(), ["ok"]);
        assert_eq!(pairs(&out.findings[2]), [("hook_dl", DETECTOR_ERROR)]);
        assert_eq!(out.findings[2][0].version, "deadline");
        // The detector returned 10 ms before the cut, so the pipeline backstop did not fire.
        assert_eq!(start.elapsed(), Duration::from_millis(490));
    }

    #[tokio::test(start_paused = true)]
    async fn each_rebuild_keeps_the_caller_the_deadline_and_the_bodies() {
        let hook = RecordingDetector::new("hook_a", vec![]);
        let judge = RecordingDetector::new("judge", vec![]);
        let bodies: Arc<[bytes::Bytes]> = Arc::from(vec![bytes::Bytes::from_static(b"{}")]);
        let mut source = batch(vec![call(0, "{}")]);
        source.caller = Caller {
            principal: Arc::from("prn:x"),
            session: Some(uuid::Uuid::nil()),
            opt_in: Arc::from(Vec::<String>::new()),
        };
        source.webhook_bodies = Some(Arc::clone(&bodies));
        let start = Instant::now();
        let p = pipeline(
            built_ins(),
            vec![hook.clone() as Arc<dyn Detector>],
            Some(judge.clone() as Arc<dyn Detector>),
            Duration::from_millis(500),
        );
        p.run(source, far()).await;
        for got in [hook.batches(), judge.batches()] {
            assert_eq!(got.len(), 1);
            assert_eq!((got[0].caller.principal.as_ref(), got[0].caller.session), ("prn:x", Some(uuid::Uuid::nil())));
            assert_eq!(got[0].deadline, start + Duration::from_millis(500));
            assert!(Arc::ptr_eq(got[0].webhook_bodies.as_ref().expect("bodies"), &bodies));
        }
    }

    #[tokio::test]
    async fn a_webhook_failure_counts_its_provider_and_reason() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let p = pipeline(built_ins(), vec![Arc::new(ErrDetector("hook_a"))], None, Duration::from_millis(500));
        p.run(batch(vec![call(0, "{}"), call(1, "{}")]), far()).await;
        let mut count = 0;
        let mut series = 0;
        for (key, _, _, value) in snapshotter.snapshot().into_vec() {
            let key = key.key();
            if key.name() != names::GATEWAY_TOOL_POLICY_WEBHOOK_FAILURES_TOTAL {
                continue;
            }
            series += 1;
            let has = |k: &str, v: &str| key.labels().any(|l| l.key() == k && l.value() == v);
            assert!(has("provider", "hook_a") && has("reason", "error") && key.labels().count() == 2, "{key:?}");
            if let metrics_util::debugging::DebugValue::Counter(c) = value {
                count += c;
            }
        }
        assert_eq!((series, count), (1, 2));
    }

    #[tokio::test]
    async fn a_strict_check_replaces_a_finding_under_another_name_a_built_in_kind_or_an_undeclared_kind() {
        let bad = [
            Finding::new("kernel", "pii_email", Severity::Medium, "v1"),
            Finding::new("hook_a", SECRET_IN_ARGS, Severity::Medium, "v1"),
            Finding::new("hook_a", "pii_iban", Severity::Medium, "v1"),
            Finding::new("hook_a", "pii_email", Severity::Medium, "V1!"),
            Finding::new("hook_a", "pii_email", Severity::Medium, "v".repeat(33)),
        ];
        for bad in bad {
            let d = DeclaredDetector {
                name: "hook_a",
                kinds: ["pii_email".to_owned()].into(),
                severity: Severity::Low,
                found: vec![bad.clone()],
            };
            let out = pipeline(built_ins(), vec![Arc::new(d)], None, Duration::from_millis(500)).run(batch(vec![call(0, "{}")]), far()).await;
            assert_eq!(out.findings[0], vec![failure_finding("hook_a", Severity::Low, WebhookFailure::Check)], "{bad:?}");
        }
    }

    #[tokio::test]
    async fn a_judge_finding_with_a_bad_version_is_replaced() {
        for version in ["V1!".to_owned(), "v".repeat(33)] {
            let judge: Arc<dyn Detector> = Arc::new(FixedDetector("judge", vec![Finding::new("judge", "fine_kind", Severity::Medium, version.as_str())]));
            let out = pipeline(built_ins(), vec![], Some(judge), Duration::from_millis(500)).run(batch(vec![call(0, "{}")]), far()).await;
            assert_eq!(out.findings[0], vec![failure_finding("judge", Severity::High, WebhookFailure::Check)], "{version}");
        }
        let judge: Arc<dyn Detector> = Arc::new(FixedDetector("judge", vec![Finding::new("judge", "fine_kind", Severity::Medium, "v1")]));
        let out = pipeline(built_ins(), vec![], Some(judge), Duration::from_millis(500)).run(batch(vec![call(0, "{}")]), far()).await;
        assert_eq!(pairs(&out.findings[0]), [("judge", "fine_kind")], "a good version stays");
    }

    /// The `check` reason counts once for a webhook bad finding, with its provider, and never for the judge.
    #[tokio::test]
    async fn a_check_failure_counts_once_for_a_webhook_and_not_for_the_judge() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let hook: Arc<dyn Detector> = Arc::new(FixedDetector("hook_a", vec![other("hook_a", "fine_kind"), other("hook_a", "Bad-Kind"), other("hook_a", "also bad")]));
        let judge: Arc<dyn Detector> = Arc::new(FixedDetector("judge", vec![other("judge", "Bad-Kind")]));
        pipeline(built_ins(), vec![hook], Some(judge), Duration::from_millis(500)).run(batch(vec![call(0, "{}")]), far()).await;
        let mut series = Vec::new();
        for (key, _, _, value) in snapshotter.snapshot().into_vec() {
            let key = key.key();
            if key.name() == names::GATEWAY_TOOL_POLICY_WEBHOOK_FAILURES_TOTAL
                && let metrics_util::debugging::DebugValue::Counter(c) = value
            {
                let label = |k: &str| key.labels().find(|l| l.key() == k).map(|l| l.value().to_owned()).unwrap_or_default();
                series.push((label("provider"), label("reason"), c));
            }
        }
        assert_eq!(series, vec![("hook_a".to_owned(), "check".to_owned(), 1)]);
    }
}
