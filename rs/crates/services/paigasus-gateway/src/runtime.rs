// SPDX-License-Identifier: Apache-2.0

//! Server-task supervision for the `paigasus-gateway` composition root.
//!
//! [`supervise`] runs a set of long-lived server tasks on a shared graceful-shutdown
//! [`watch`] channel: it waits for the first of a shutdown signal or any task ending,
//! then broadcasts graceful shutdown to the rest and drains them, surfacing the first
//! error. This turns a metrics-listener (or upkeep) failure — previously a detached
//! task that only logged before dying — into an error that propagates out of `main`
//! (SMA-463). Mirrors `paigasus-iam`'s composition-root supervision so both services
//! share one model.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use metrics::counter;
use paigasus_gateway_core::{AllowAll, ToolCallDecider};
use paigasus_kernel::tool_policy::history::HistoryDigester;
use paigasus_observability::names;
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::task::TaskTracker;

use crate::adapters::decision_log::jetstream;
use crate::adapters::decision_log::{RedisDecisionLog, RedisSlot, RelaySettings, WriterSettings, connect_loop, prime_decision_log_metrics, run_relay, run_writer};
use crate::adapters::detectors::BuiltInDetector;
use crate::adapters::history::HmacHistoryDigester;
use crate::adapters::http::interception::ToolInterception;
use crate::adapters::iam::client::build_channel;
use crate::adapters::nats::PolicyEvents;
use crate::adapters::policy_source::file::FilePolicySource;
use crate::adapters::policy_source::iam::IamPolicySource;
use crate::application::decision_log::DecisionRecorder;
use crate::application::detectors::DetectorPipeline;
use crate::application::tool_history::ToolHistory;
use crate::application::tool_policy::{CacheSettings, CedarDecider, Invalidate, PolicyCache, RequestToolPolicy, prime_tool_policy_metrics};
use crate::config::{GatewayConfig, PolicySourceConfig, ToolHistoryConfig};
use crate::domain::decision_log::{Entropy, TracingDecisionLog};
use crate::domain::limits::{ChargeDropReason, Clock};
use crate::domain::tool_policy::PolicySource;

/// D16: how long shutdown waits for in-flight limit charges.
pub const CHARGE_DRAIN_BUDGET: Duration = Duration::from_secs(5);

/// SMA-755 § 5.6, P-18: how long shutdown waits for the writer and the relay of the decision log.
pub const DECISION_LOG_DRAIN_BUDGET: Duration = Duration::from_secs(2);

/// Supervise a set of server tasks on a shared graceful-shutdown watch.
///
/// Waits for the first of: `shutdown` resolving (an OS signal), or any task in
/// `servers` ending (cleanly, with an error, or by panic). Then broadcasts graceful
/// shutdown via `tx` and drains the remaining tasks, returning the first error
/// observed. A clean early task return is logged (warn) but is not an error.
///
/// # Invariants
/// - Every task in `servers` must observe shutdown through a [`watch::Receiver`] cloned
///   from the same channel as `tx` **before** this function is called, so `tx.send`
///   reaches it. Receivers cloned before the first send do not wake spuriously, so the
///   first `changed().await` correctly waits.
/// - `charges`: the Redis limit store's spawned charges (SMA-677 D16). After the servers drain,
///   they get at most [`CHARGE_DRAIN_BUDGET`]; the rest are counted as dropped.
/// - `history`: the settle and append tasks of the tool history (SMA-753). They drain together with the charges, for at
///   most [`CHARGE_DRAIN_BUDGET`] in all. A settle that still runs is lost and logged. It leaves its events `pending`,
///   and `pending` events still count.
/// - `decision_log`: the writer and the relay run outside `servers`, on a shutdown watch of their own (P-18).
///   They drain last, after the servers and after the charges, because a server that stops can still
///   record a decision. They get at most [`DECISION_LOG_DRAIN_BUDGET`]. They never end the gateway.
/// - Callers must pass either a non-empty `servers` or a `shutdown` that resolves; an
///   empty set with a non-resolving `shutdown` would disable both `select!` arms and
///   wait forever. The gateway always spawns its main HTTP task, so it never hits this.
pub async fn supervise(
    mut servers: JoinSet<anyhow::Result<()>>,
    shutdown: impl Future<Output = ()>,
    tx: watch::Sender<()>,
    charges: Option<TaskTracker>,
    history: Option<TaskTracker>,
    decision_log: Option<DecisionLogTasks>,
) -> anyhow::Result<()> {
    // Stop on the first of: shutdown signal, or a server task ending.
    let early_error: Option<anyhow::Error> = tokio::select! {
        () = shutdown => {
            tracing::info!("shutdown signal received");
            None
        }
        Some(joined) = servers.join_next() => {
            match joined {
                Ok(Ok(())) => {
                    tracing::warn!("a server task exited before shutdown was requested");
                    None
                }
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "a server task failed");
                    Some(e)
                }
                Err(join_err) => {
                    tracing::error!(error = %join_err, "a server task panicked");
                    Some(join_err.into())
                }
            }
        }
    };

    // Ask any still-running server to shut down gracefully.
    let _ = tx.send(());

    // Drain the remaining server task(s); surface the first error.
    let mut result = early_error.map_or(Ok(()), Err);
    while let Some(joined) = servers.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(e)) if result.is_ok() => result = Err(e),
            Ok(Err(_)) => {}
            Err(join_err) if result.is_ok() => result = Err(join_err.into()),
            Err(_) => {}
        }
    }
    // The charge drain and the history drain run together, so the worst case is one budget, not two.
    tokio::join!(
        async {
            if let Some(charges) = &charges {
                drain_charges(charges, CHARGE_DRAIN_BUDGET).await;
            }
        },
        async {
            if let Some(history) = &history {
                drain_history(history, CHARGE_DRAIN_BUDGET).await;
            }
        }
    );
    if let Some(tasks) = decision_log {
        tasks.drain(DECISION_LOG_DRAIN_BUDGET).await;
    }
    result
}

/// The three tasks of the Redis decision log. They run outside the server `JoinSet` (P-18): a failure of
/// one of them never stops the gateway (spec § 5.6).
pub struct DecisionLogTasks {
    shutdown: watch::Sender<()>,
    writer: JoinHandle<()>,
    relay: JoinHandle<()>,
    connector: JoinHandle<()>,
}

/// The time that [`DecisionLogTasks::drain`] adds to the budget before it aborts a task.
pub const DECISION_LOG_DRAIN_GRACE: Duration = Duration::from_millis(500);

impl DecisionLogTasks {
    /// The tasks and the sender of their shutdown watch. Each task must hold a receiver of `shutdown`.
    #[must_use]
    pub fn new(shutdown: watch::Sender<()>, writer: JoinHandle<()>, relay: JoinHandle<()>, connector: JoinHandle<()>) -> Self {
        Self { shutdown, writer, relay, connector }
    }

    /// Send the shutdown signal, then wait for the three tasks. The writer and the relay drain for at most
    /// `budget` themselves. A task that is still running `budget` plus [`DECISION_LOG_DRAIN_GRACE`] after the
    /// signal is aborted.
    pub async fn drain(self, budget: Duration) {
        let Self { shutdown, writer, relay, connector } = self;
        let _ = shutdown.send(());
        let grace = budget + DECISION_LOG_DRAIN_GRACE;
        tokio::join!(wait_task("writer", writer, grace), wait_task("relay", relay, grace), wait_task("connector", connector, grace));
    }
}

async fn wait_task(name: &'static str, mut handle: JoinHandle<()>, grace: Duration) {
    match tokio::time::timeout(grace, &mut handle).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::error!(task = name, error = %e, "a decision-log task ended with a panic"),
        Err(_) => {
            tracing::warn!(task = name, "a decision-log task did not stop in time and was aborted");
            handle.abort();
        }
    }
}

/// What the composition root wires for the decision log.
pub struct DecisionLogRuntime {
    /// The one recorder (P-8). The same `Arc` goes to the decider, the request side and the report site.
    pub recorder: Arc<DecisionRecorder>,
    /// The writer, the relay and the connect loop. `None` with `[decision_log] enabled = false`.
    pub tasks: Option<DecisionLogTasks>,
}

/// Build the decision log from a config that passed [`GatewayConfig::validate`]. With the log off, the
/// recorder writes the tracing line only. With the log on, it builds the Redis log, spawns the connect loop,
/// the writer and the relay, and primes the metrics. It never waits for Redis or NATS: the connect loop and
/// the NATS client connect in the background, so a broker that is down does not stop the boot (spec § 5.6).
///
/// # Errors
///
/// A `[decision_log]` config that skipped `validate` (no Redis URL or no `[decision_log.nats]`), or a NATS
/// creds file that cannot be read or parsed.
pub async fn build_decision_log(config: &GatewayConfig, clock: Arc<dyn Clock>, entropy: Arc<dyn Entropy>) -> anyhow::Result<DecisionLogRuntime> {
    let dl = &config.decision_log;
    if !dl.enabled {
        return Ok(DecisionLogRuntime {
            recorder: Arc::new(DecisionRecorder::new(Arc::new(TracingDecisionLog), clock, entropy, dl.digest_key())),
            tasks: None,
        });
    }
    let redis_url = dl.redis_url.clone().ok_or_else(|| anyhow::anyhow!("decision_log.enabled = true requires decision_log.redis_url"))?;
    let nats = dl.nats.as_ref().ok_or_else(|| anyhow::anyhow!("decision_log.enabled = true requires [decision_log.nats]"))?;
    let violations = jetstream::Violations::default();
    let js = jetstream::connect(nats, violations.clone()).await?;
    let (log, queue) = RedisDecisionLog::new(dl.queue_capacity);
    let recorder = Arc::new(DecisionRecorder::new(Arc::new(log), clock, entropy, dl.digest_key()));
    prime_decision_log_metrics();

    let key = format!("{}:decisions", dl.key_prefix);
    let slot = RedisSlot::new();
    let (shutdown, rx) = watch::channel(());
    let connector = tokio::spawn(connect_loop(redis_url, slot.clone(), rx.clone()));
    let writer = tokio::spawn(run_writer(
        queue,
        slot.clone(),
        WriterSettings {
            key: key.clone(),
            max_len: dl.stream_max_len,
        },
        rx.clone(),
        DECISION_LOG_DRAIN_BUDGET,
    ));
    let relay = tokio::spawn(run_relay(
        slot,
        js,
        RelaySettings::new(key, recorder.instance_id().to_string()),
        violations,
        rx,
        DECISION_LOG_DRAIN_BUDGET,
    ));
    tracing::info!("the decision log is on (SMA-755): tool decisions go to a Redis stream and from there to JetStream");
    Ok(DecisionLogRuntime {
        recorder,
        tasks: Some(DecisionLogTasks::new(shutdown, writer, relay, connector)),
    })
}

/// D16: close the tracker and wait at most `budget`. Returns the number of charges still running
/// then. They are lost, and counted as `gateway_limit_charges_dropped_total{reason="shutdown"}`.
pub async fn drain_charges(charges: &TaskTracker, budget: Duration) -> usize {
    charges.close();
    if tokio::time::timeout(budget, charges.wait()).await.is_ok() {
        return 0;
    }
    let lost = charges.len();
    counter!(names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, "reason" => ChargeDropReason::Shutdown.as_label()).increment(u64::try_from(lost).unwrap_or(u64::MAX));
    tracing::warn!(lost, "shutdown: limit charges still running after the drain budget were dropped");
    lost
}

/// Close the history tracker and wait at most `budget`. Returns the number of tasks that were lost.
async fn drain_history(history: &TaskTracker, budget: Duration) -> usize {
    history.close();
    if tokio::time::timeout(budget, history.wait()).await.is_ok() {
        return 0;
    }
    let lost = history.len();
    tracing::warn!(
        lost,
        "shutdown: tool history tasks still running after the drain budget were lost (a lost settle leaves its events pending)"
    );
    lost
}

/// SMA-752 D19: what the composition root wires for the tool policy.
pub struct ToolPolicyRuntime {
    /// The cache (for its refresh task) and the request side. `None` with `[tool_policy] enabled = false`.
    pub policy: Option<(Arc<PolicyCache>, Arc<RequestToolPolicy>)>,
    /// The decider for tool interception. The Cedar decider only with the tool policy on: without
    /// the request side there is no snapshot, and the Cedar decider would block every call (fail
    /// closed). Otherwise `AllowAll`, as before SMA-752.
    pub decider: Arc<dyn ToolCallDecider>,
    /// The Cedar decider behind `decider` (the same allocation), `None` with the tool policy off. It lets a
    /// test check that the decider holds the one recorder (P-8).
    pub cedar: Option<Arc<CedarDecider>>,
    /// The one decision recorder (P-8). `None` with the tool policy off: nothing is recorded.
    pub recorder: Option<Arc<DecisionRecorder>>,
    /// The one detector pipeline (SMA-855 § 6.6.4). `None` with the tool policy off or with
    /// `[tool_policy.detectors] enabled = false`.
    pub detectors: Option<Arc<DetectorPipeline>>,
    /// The tool history (SMA-753). `None` with the tool policy off or without `[tool_history]`.
    pub history: Option<Arc<ToolHistory>>,
}

/// The tool interception of the composition root, only with `[tool_interception] enabled = true`.
/// It holds the decider and the recorder of `rt`, so the report site records through the recorder
/// of the decider and of the request side (P-8).
#[must_use]
pub fn tool_interception(config: &GatewayConfig, rt: &ToolPolicyRuntime) -> Option<Arc<ToolInterception>> {
    config.tool_interception.enabled.then(|| {
        tracing::info!("tool-call interception is on (SMA-749): tool calls in chat completions are held, checked and may be rewritten");
        // SMA-752: the Cedar decider only with the tool policy on (`build_tool_policy`).
        Arc::new(ToolInterception {
            limits: config.tool_interception.limits(),
            decider: Arc::clone(&rt.decider),
            recorder: rt.recorder.clone(),
            history: rt.history.clone(),
        })
    })
}

/// Build the tool policy from a config that passed [`GatewayConfig::validate`]: `settings()` panics
/// on an org id that `validate` refuses. With `enabled`, `validate` has made sure that tool
/// interception is on, that `on_policy_unavailable` is set (D9) and that a source is set.
///
/// # Errors
///
/// `enabled = true` with no `[tool_policy.source]` (a config that skipped `validate`).
pub fn build_tool_policy(config: &GatewayConfig, recorder: Arc<DecisionRecorder>, history: Option<Arc<ToolHistory>>) -> anyhow::Result<ToolPolicyRuntime> {
    if !config.tool_policy.enabled {
        if history.is_some() {
            tracing::info!("[tool_history] is set, but the tool policy is off: the history is not used");
        }
        return Ok(ToolPolicyRuntime {
            policy: None,
            decider: Arc::new(AllowAll),
            cedar: None,
            recorder: None,
            detectors: None,
            history: None,
        });
    }
    let settings = config.tool_policy.settings();
    let (source, kind): (Arc<dyn PolicySource>, &str) = match &config.tool_policy.source {
        Some(PolicySourceConfig::File { dir }) => (Arc::new(FilePolicySource::new(dir)), "file"),
        // The channel is lazy: an IAM that is down at boot does not stop the gateway.
        Some(PolicySourceConfig::Iam {}) => {
            let credential_file = config
                .iam
                .service_credential_file
                .clone()
                .expect("GatewayConfig::validate requires iam.service_credential_file with tool_policy.source kind = \"iam\"");
            (Arc::new(IamPolicySource::new(build_channel(&config.iam)?, credential_file)), "iam")
        }
        None => anyhow::bail!("tool_policy.enabled = true requires tool_policy.source"),
    };
    // The history's digester when there is a history: one digester for the literals and the stored
    // values (spec P3), and one no-key warning at boot.
    let digester = match &history {
        Some(h) => h.digester(),
        None => history_digester(config.tool_history.as_ref())?,
    };
    let cache = PolicyCache::new(source, CacheSettings::from_settings(&settings)).with_digester(digester);
    if settings.settings_from_source {
        // The source holds the mode of each org, so `default_mode` applies to no org (final review M-2).
        tracing::info!(
            source = kind,
            "the tool policy is on (SMA-752): tool definitions, results and calls are checked against the Cedar policies; the mode of each org comes from the source"
        );
    } else {
        tracing::info!(
            source = kind,
            mode = settings.default_mode.as_str(),
            "the tool policy is on (SMA-752): tool definitions, results and calls are checked against the Cedar policies"
        );
    }
    prime_tool_policy_metrics();
    let detectors = build_detectors(config);
    let mut cedar = CedarDecider::new(Arc::clone(&recorder));
    if let Some(pipeline) = &detectors {
        cedar = cedar.with_detectors(Arc::clone(pipeline), config.tool_interception.limits().decide_timeout, config.tool_policy.detector_settings());
    }
    let mut request_side = RequestToolPolicy::new(Arc::clone(&cache), settings, Arc::clone(&recorder)).with_detectors(detectors.clone(), config.tool_policy.detector_settings());
    if let Some(h) = &history {
        cedar = cedar.with_history(Arc::clone(h));
        request_side = request_side.with_history(Arc::clone(h));
    }
    let cedar = Arc::new(cedar);
    Ok(ToolPolicyRuntime {
        policy: Some((cache, Arc::new(request_side))),
        decider: Arc::clone(&cedar) as Arc<dyn ToolCallDecider>,
        cedar: Some(cedar),
        recorder: Some(recorder),
        detectors,
        history,
    })
}

/// The history digester of the gateway (SMA-753 spec P3), built once from `[tool_history]`. With no
/// table there is no history, so no key is needed and nothing is logged. A table with no
/// `digest_key` logs one boot warning: a condition on a string, bool or decimal value then has a
/// missing slot (reason `no_key`).
///
/// # Errors
///
/// A key that `GatewayConfig::validate` refuses (a config that skipped `validate`).
pub fn history_digester(config: Option<&ToolHistoryConfig>) -> anyhow::Result<Arc<dyn HistoryDigester>> {
    let Some(config) = config else {
        return Ok(Arc::new(HmacHistoryDigester::keyless()));
    };
    if config.digest_key.is_none() {
        tracing::warn!("[tool_history] has no digest_key: conditions on string, bool or decimal values have no value (missing slot, reason no_key)");
    }
    Ok(Arc::new(HmacHistoryDigester::from_config(config).map_err(anyhow::Error::msg)?))
}

/// The one detector pipeline of the gateway (SMA-855 § 6.6.4): the built-in stage only, no webhook
/// and no judge yet. With `[tool_policy.detectors] enabled = false` there is none, and one info
/// line says so at boot.
fn build_detectors(config: &GatewayConfig) -> Option<Arc<DetectorPipeline>> {
    let settings = config.tool_policy.detector_settings();
    if !settings.enabled {
        tracing::info!("tool-policy detectors are off: findings come only from the kernel");
        return None;
    }
    Some(Arc::new(DetectorPipeline::new(
        Arc::new(BuiltInDetector::new(settings.max_scan_bytes)),
        Vec::new(),
        None,
        settings.timeout,
    )))
}

/// SMA-752 § 6.3.5: the NATS invalidation subscription for `cache`, only with `[tool_policy]
/// enabled = true` AND a `[tool_policy.nats]` table. Otherwise `None`: the refresh alone keeps the
/// cache current. The caller spawns [`PolicyEvents::run`] on the server `JoinSet`.
///
/// A broker that is down is not an error: the client connects in the background.
///
/// # Errors
///
/// [`crate::adapters::nats::NatsError`] (a creds file that cannot be read or parsed, or options
/// that the client refuses).
pub async fn build_policy_events(config: &GatewayConfig, cache: &Arc<PolicyCache>) -> anyhow::Result<Option<PolicyEvents>> {
    let Some(nats) = config.tool_policy.nats.as_ref().filter(|_| config.tool_policy.enabled) else {
        return Ok(None);
    };
    let target: Arc<dyn Invalidate> = Arc::clone(cache) as Arc<dyn Invalidate>;
    Ok(Some(crate::adapters::nats::connect(nats, target).await?))
}

#[cfg(test)]
// `figment::Jail::expect_with` fixes its closure's `Err` type to `figment::Error` (the
// `config.rs` test module allows the size lint for the same reason).
#[allow(clippy::result_large_err)]
mod tool_policy_tests {
    use super::*;
    use paigasus_gateway_core::{BlockCode, Decision, DecisionContext, Format, parse_body, parse_request};

    const PRINCIPAL: &str = "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004";
    const ORG_A_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";
    const BASE: &str = "[upstream.openai]\napi_key = \"sk-test-key\"\n";

    /// A config read from `toml` alone, as `GatewayConfig::load` reads it. `validate` must pass
    /// when `validated` is true, as in `main`.
    fn config(toml: &str, validated: bool) -> GatewayConfig {
        let mut out = None;
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", toml)?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            if validated {
                cfg.validate().expect("the config validates");
            }
            out = Some(cfg);
            Ok(())
        });
        out.expect("a config")
    }

    /// The decision of `decider` for one `read_file` call, with no policy snapshot in the context.
    async fn decide_one(decider: &Arc<dyn ToolCallDecider>) -> Decision {
        let body = serde_json::json!({"object": "chat.completion", "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [
            {"id": "c0", "type": "function", "function": {"name": "read_file", "arguments": "{}"}}
        ]}, "finish_reason": "tool_calls"}]})
        .to_string();
        let calls = parse_body(Format::OpenAiChat, body.as_bytes(), 1 << 20).expect("body").calls;
        let turn = parse_request(Format::OpenAiChat, br#"{"model":"m","messages":[]}"#);
        let ctx = DecisionContext::new(PRINCIPAL, ORG_A_PRN, None);
        let out = decider.decide(Arc::new(ctx), Arc::new(turn), calls).await.expect("decide");
        assert_eq!(out.len(), 1);
        out[0].decision
    }

    /// SMA-752 D19, Task 16 ruling (b): with the tool policy off, nothing is built and the decider
    /// is `AllowAll`, as before SMA-752.
    #[tokio::test]
    async fn the_tool_policy_off_builds_nothing_and_allows_every_call() {
        let cfg = config(&format!("{BASE}[tool_interception]\nenabled = true\n"), true);
        let rt = build_tool_policy(&cfg, rec(), None).expect("builds");
        assert!(rt.policy.is_none());
        assert_eq!(decide_one(&rt.decider).await, Decision::Allow);
    }

    /// With the tool policy on and a file source, the cache and the request side exist, and the
    /// decider is the Cedar decider. Without the request's snapshot the Cedar decider fails closed,
    /// which `AllowAll` never does: a regression that leaves `AllowAll` on reds here.
    #[tokio::test]
    async fn the_tool_policy_on_builds_the_request_side_and_the_cedar_decider() {
        let cfg = config(
            &format!(
                "{BASE}[tool_interception]\nenabled = true\n[tool_policy]\nenabled = true\non_policy_unavailable = \"closed\"\ndefault_mode = \"enforce\"\nsource = {{ kind = \"file\", dir = \"/nonexistent\" }}\n"
            ),
            true,
        );
        let rt = build_tool_policy(&cfg, rec(), None).expect("builds");
        assert!(rt.policy.is_some());
        assert_eq!(decide_one(&rt.decider).await, Decision::Block { code: BlockCode::PolicyUnavailable });
    }

    /// A config that skipped `validate` and has no source is an error, not a panic.
    #[test]
    fn the_tool_policy_on_without_a_source_is_an_error() {
        let cfg = config(&format!("{BASE}[tool_policy]\nenabled = true\non_policy_unavailable = \"open\"\n"), false);
        let err = build_tool_policy(&cfg, rec(), None).err().expect("an error");
        assert!(err.to_string().contains("tool_policy.source"), "{err}");
    }

    fn rec() -> Arc<DecisionRecorder> {
        Arc::new(DecisionRecorder::tracing())
    }

    fn enabled_config() -> GatewayConfig {
        config(
            &format!(
                "{BASE}[tool_interception]\nenabled = true\n[tool_policy]\nenabled = true\non_policy_unavailable = \"closed\"\ndefault_mode = \"enforce\"\nsource = {{ kind = \"file\", dir = \"/nonexistent\" }}\n"
            ),
            true,
        )
    }

    /// P-8: one recorder serves the decider, the request side and the report site.
    #[tokio::test]
    async fn the_decider_the_request_side_and_the_report_site_share_one_recorder() {
        use crate::application::decision_log::DigestKey;
        use crate::domain::decision_log::{MemoryDecisionLog, OsEntropy};
        use crate::domain::limits::SystemClock;

        let key = DigestKey::new("k1".into(), "k".repeat(32).into()).expect("key");
        let recorder = Arc::new(DecisionRecorder::new(Arc::new(MemoryDecisionLog::new(true)), Arc::new(SystemClock), Arc::new(OsEntropy), Some(key)));
        let cfg = enabled_config();
        let rt = build_tool_policy(&cfg, Arc::clone(&recorder), None).expect("builds");
        let (_, request_side) = rt.policy.as_ref().expect("on");
        assert!(Arc::ptr_eq(request_side.recorder(), &recorder), "the request side");
        assert!(Arc::ptr_eq(rt.recorder.as_ref().expect("a recorder"), &recorder), "the runtime");
        let ti = tool_interception(&cfg, &rt).expect("interception is on");
        assert!(Arc::ptr_eq(ti.recorder.as_ref().expect("a recorder"), &recorder), "the report site");
        // The decider: a true identity check through the accessor of the Cedar decider.
        let cedar = rt.cedar.as_ref().expect("the Cedar decider");
        assert!(Arc::ptr_eq(cedar.recorder(), &recorder), "the decider");
        let cedar_as_dyn: Arc<dyn ToolCallDecider> = Arc::clone(cedar) as Arc<dyn ToolCallDecider>;
        assert!(std::ptr::addr_eq(Arc::as_ptr(&cedar_as_dyn), Arc::as_ptr(&rt.decider)), "the runtime decider is the Cedar decider");
    }

    /// SMA-855 § 6.6.4: with the detectors on (the default), one pipeline, and the Cedar decider
    /// holds that same pipeline.
    #[test]
    fn detectors_on_build_one_pipeline_for_the_decider() {
        let rt = build_tool_policy(&enabled_config(), rec(), None).expect("builds");
        let pipeline = rt.detectors.as_ref().expect("the detectors are on by default");
        let held = rt.cedar.as_ref().and_then(|c| c.pipeline()).expect("the decider holds the pipeline");
        assert!(Arc::ptr_eq(pipeline, held));
    }

    /// `[tool_policy.detectors] enabled = false`: no pipeline, and one info line at boot.
    #[test]
    fn detectors_off_builds_no_pipeline_and_logs_once() {
        let cfg = config(
            &format!(
                "{BASE}[tool_interception]\nenabled = true\n[tool_policy]\nenabled = true\non_policy_unavailable = \"closed\"\ndefault_mode = \"enforce\"\nsource = {{ kind = \"file\", dir = \"/nonexistent\" }}\n[tool_policy.detectors]\nenabled = false\n"
            ),
            true,
        );
        let (logs, _guard) = paigasus_logging::test_support::capture_logs();
        let rt = build_tool_policy(&cfg, rec(), None).expect("builds");
        assert!(rt.detectors.is_none());
        assert!(rt.cedar.as_ref().expect("the Cedar decider").pipeline().is_none());
        let text = logs.text();
        assert_eq!(text.matches("tool-policy detectors are off: findings come only from the kernel").count(), 1, "{text}");
        // With the detectors on, the line does not appear.
        let (logs, _guard) = paigasus_logging::test_support::capture_logs();
        build_tool_policy(&enabled_config(), rec(), None).expect("builds");
        assert!(!logs.text().contains("tool-policy detectors are off"));
    }

    /// With the tool policy off nothing is recorded: no recorder is kept.
    #[test]
    fn the_tool_policy_off_keeps_no_recorder() {
        let cfg = config(&format!("{BASE}[tool_interception]\nenabled = true\n"), true);
        let rt = build_tool_policy(&cfg, rec(), None).expect("builds");
        assert!(rt.recorder.is_none());
        assert!(tool_interception(&cfg, &rt).expect("on").recorder.is_none());
    }

    #[test]
    fn prime_tool_policy_metrics_primes_every_triple_at_zero() {
        use crate::application::tool_policy::FINDING_TRIPLES;
        let debugging = metrics_util::debugging::DebuggingRecorder::new();
        let snapshotter = debugging.snapshotter();
        let _local = metrics::set_default_local_recorder(&debugging);
        build_tool_policy(&enabled_config(), rec(), None).expect("builds");
        let others = ["info", "low", "medium", "high"].map(|s| ("other", "other", s));
        let mut n = 0;
        for side in ["request", "response"] {
            for (kind, detector, severity) in FINDING_TRIPLES.into_iter().chain(others) {
                let labels = [("side", side), ("kind", kind), ("detector", detector), ("severity", severity)];
                let v = crate::test_support::counter(&snapshotter, names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL, &labels);
                assert_eq!(v, Some(0), "{side}/{kind}/{detector}/{severity} is primed at 0");
                n += 1;
            }
        }
        assert_eq!(n, 40);
        let series = snapshotter
            .snapshot()
            .into_vec()
            .into_iter()
            .filter(|(k, ..)| k.key().name() == names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL)
            .count();
        assert_eq!(series, 40);
    }

    /// A tool policy config, on or off, with an optional `[tool_policy.nats]` table.
    fn tp_config(enabled: bool, nats: Option<&str>) -> GatewayConfig {
        let nats = nats.map(|url| format!("[tool_policy.nats]\nurl = \"{url}\"\n")).unwrap_or_default();
        config(
            &format!(
                "{BASE}[tool_interception]\nenabled = true\n[tool_policy]\nenabled = {enabled}\non_policy_unavailable = \"closed\"\nsource = {{ kind = \"file\", dir = \"/nonexistent\" }}\n{nats}"
            ),
            true,
        )
    }

    /// A port that nothing listens on.
    fn closed_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port()
    }

    /// SMA-752 § 6.3.5, Task 18: the subscription exists only with the tool policy on AND a
    /// `[tool_policy.nats]` table. A broker that is down is not a boot error.
    #[tokio::test]
    async fn the_nats_subscription_is_built_only_with_the_policy_on_and_a_nats_table() {
        let url = format!("nats://127.0.0.1:{}", closed_port());
        let cache = build_tool_policy(&tp_config(true, None), rec(), None).expect("builds").policy.expect("on").0;

        assert!(build_policy_events(&tp_config(true, None), &cache).await.expect("no error").is_none(), "no nats table: refresh only");
        assert!(build_policy_events(&tp_config(false, Some(&url)), &cache).await.expect("no error").is_none(), "the tool policy is off");
        let events = tokio::time::timeout(Duration::from_secs(5), build_policy_events(&tp_config(true, Some(&url)), &cache))
            .await
            .expect("the broker is not waited for")
            .expect("a down broker is not an error");
        assert!(events.is_some(), "on with a nats table: the subscription is built");
    }

    fn decision_log_config(enabled: bool) -> GatewayConfig {
        let dl = if enabled {
            format!(
                "[decision_log]\nenabled = true\nredis_url = \"redis://127.0.0.1:{}\"\ndigest_key = \"{}\"\ndigest_key_id = \"k1\"\n[decision_log.nats]\nurl = \"nats://127.0.0.1:{}\"\n",
                closed_port(),
                "k".repeat(32),
                closed_port()
            )
        } else {
            String::new()
        };
        config(
            &format!(
                "{BASE}[tool_interception]\nenabled = true\n[tool_policy]\nenabled = true\non_policy_unavailable = \"closed\"\ndefault_mode = \"enforce\"\nsource = {{ kind = \"file\", dir = \"/nonexistent\" }}\n{dl}"
            ),
            true,
        )
    }

    fn clock_and_entropy() -> (Arc<dyn crate::domain::limits::Clock>, Arc<dyn crate::domain::decision_log::Entropy>) {
        (Arc::new(crate::domain::limits::SystemClock), Arc::new(crate::domain::decision_log::OsEntropy))
    }

    #[tokio::test]
    async fn the_decision_log_off_uses_the_tracing_log() {
        let (clock, entropy) = clock_and_entropy();
        let rt = build_decision_log(&decision_log_config(false), clock, entropy).await.expect("builds");
        assert!(!rt.recorder.log().wants_digest(), "the tracing log wants no digest");
        assert!(rt.tasks.is_none());
    }

    /// With the log on, the recorder holds the Redis log, and the decider, the request side and the
    /// `ToolInterception` hold that one recorder. Redis and NATS are unreachable: the build does not wait.
    #[tokio::test]
    async fn the_decision_log_on_shares_one_redis_log() {
        let (clock, entropy) = clock_and_entropy();
        let cfg = decision_log_config(true);
        let dl = tokio::time::timeout(Duration::from_secs(5), build_decision_log(&cfg, clock, entropy))
            .await
            .expect("neither Redis nor NATS is waited for")
            .expect("builds");
        assert!(dl.recorder.log().wants_digest(), "the Redis log wants the digest");
        let tasks = dl.tasks.expect("the three tasks");
        let recorder = dl.recorder;
        let rt = build_tool_policy(&cfg, Arc::clone(&recorder), None).expect("builds");
        let (_, request_side) = rt.policy.as_ref().expect("on");
        assert!(Arc::ptr_eq(request_side.recorder(), &recorder), "the request side");
        assert!(Arc::ptr_eq(rt.cedar.as_ref().expect("cedar").recorder(), &recorder), "the decider");
        let ti = tool_interception(&cfg, &rt).expect("interception is on");
        assert!(Arc::ptr_eq(ti.recorder.as_ref().expect("a recorder"), &recorder), "the report site");
        tasks.drain(Duration::from_millis(100)).await;
    }

    /// A creds file that does not exist is a boot error that names the path, not the URL.
    #[tokio::test]
    async fn a_missing_creds_file_is_a_boot_error_without_the_url() {
        let mut cfg = tp_config(true, Some("nats://user:hunter2@127.0.0.1:1"));
        cfg.tool_policy.nats.as_mut().expect("nats").creds_file = Some("/nonexistent/gw.creds".to_string());
        let cache = build_tool_policy(&cfg, rec(), None).expect("builds").policy.expect("on").0;
        let err = build_policy_events(&cfg, &cache).await.err().expect("an error");
        let text = format!("{err:#}");
        assert!(text.contains("/nonexistent/gw.creds"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
    }

    /// SMA-753: one `ToolHistory` reaches the request side, the decider and the report site.
    #[tokio::test]
    async fn the_history_reaches_the_request_side_the_decider_and_the_report_site() {
        use crate::adapters::history::MemorySessionStore;
        use crate::domain::decision_log::OsEntropy;
        use crate::domain::limits::SystemClock;

        let history = Arc::new(ToolHistory::new(
            Arc::new(MemorySessionStore::new()),
            Arc::new(SystemClock),
            Arc::new(OsEntropy),
            Duration::from_secs(1),
            Arc::new(crate::adapters::history::HmacHistoryDigester::keyless()),
        ));
        let cfg = enabled_config();
        let rt = build_tool_policy(&cfg, rec(), Some(Arc::clone(&history))).expect("builds");
        let (_, request_side) = rt.policy.as_ref().expect("on");
        assert!(Arc::ptr_eq(request_side.history().expect("request side history"), &history), "the request side");
        assert!(Arc::ptr_eq(rt.cedar.as_ref().expect("cedar").history().expect("decider history"), &history), "the decider");
        let ti = tool_interception(&cfg, &rt).expect("interception is on");
        assert!(Arc::ptr_eq(ti.history.as_ref().expect("report site history"), &history), "the report site");
    }

    const NO_KEY_WARNING: &str = "[tool_history] has no digest_key: conditions on string, bool or decimal values have no value (missing slot, reason no_key)";

    /// SMA-753 spec P3: a `[tool_history]` table with no `digest_key` logs one boot warning. A
    /// table with a key, or no table, logs none.
    #[test]
    fn a_history_table_with_no_digest_key_warns_once_at_boot() {
        let with_table = |extra: &str| {
            config(
                &format!(
                    "{BASE}[tool_interception]\nenabled = true\n[tool_policy]\nenabled = true\non_policy_unavailable = \"closed\"\ndefault_mode = \"enforce\"\nsource = {{ kind = \"file\", dir = \"/nonexistent\" }}\n[tool_history]\n{extra}"
                ),
                true,
            )
        };
        let warnings = |cfg: &GatewayConfig| {
            let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
            build_tool_policy(cfg, rec(), None).expect("builds");
            logs.text().matches(NO_KEY_WARNING).count()
        };
        assert_eq!(warnings(&with_table("")), 1, "no key: one warning");
        assert_eq!(warnings(&with_table("digest_key = \"0123456789abcdef0123456789abcdef-one\"\n")), 0, "a key: no warning");
        assert_eq!(warnings(&enabled_config()), 0, "no table: no history and no warning");
    }

    /// SMA-753 spec P3: the boot builds ONE digester (`history_digester`, one warning). The history
    /// keeps it, and `build_tool_policy` uses the history's digester: it builds no second one, so it
    /// logs no second warning.
    #[test]
    fn the_history_digester_is_built_once_and_reaches_the_policy() {
        use crate::adapters::history::MemorySessionStore;
        use crate::domain::decision_log::OsEntropy;
        use crate::domain::limits::SystemClock;

        let cfg = config(
            &format!(
                "{BASE}[tool_interception]\nenabled = true\n[tool_policy]\nenabled = true\non_policy_unavailable = \"closed\"\ndefault_mode = \"enforce\"\nsource = {{ kind = \"file\", dir = \"/nonexistent\" }}\n[tool_history]\n"
            ),
            true,
        );
        let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
        let digester = history_digester(cfg.tool_history.as_ref()).expect("a digester");
        let history = Arc::new(ToolHistory::new(
            Arc::new(MemorySessionStore::new()),
            Arc::new(SystemClock),
            Arc::new(OsEntropy),
            Duration::from_secs(1),
            Arc::clone(&digester),
        ));
        assert!(Arc::ptr_eq(&history.digester(), &digester), "the history keeps the one digester");
        build_tool_policy(&cfg, rec(), Some(history)).expect("builds");
        assert_eq!(logs.text().matches(NO_KEY_WARNING).count(), 1, "{}", logs.text());
    }

    /// With the tool policy off the history is not used.
    #[tokio::test]
    async fn the_tool_policy_off_does_not_use_the_history() {
        use crate::adapters::history::MemorySessionStore;
        use crate::domain::decision_log::OsEntropy;
        use crate::domain::limits::SystemClock;

        let history = Arc::new(ToolHistory::new(
            Arc::new(MemorySessionStore::new()),
            Arc::new(SystemClock),
            Arc::new(OsEntropy),
            Duration::from_secs(1),
            Arc::new(crate::adapters::history::HmacHistoryDigester::keyless()),
        ));
        let cfg = config(&format!("{BASE}[tool_interception]\nenabled = true\n"), true);
        let rt = build_tool_policy(&cfg, rec(), Some(history)).expect("builds");
        assert!(rt.history.is_none());
        assert!(tool_interception(&cfg, &rt).expect("on").history.is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::{pending, ready};

    /// I1 (SMA-821): with the production values, the relay's wait for an in-flight KV step after the shutdown
    /// signal fits inside the time that `DecisionLogTasks::drain` gives the relay task. A wait of `kv_timeout`
    /// (5 s) would not fit, and drain would abort the relay while the step runs on.
    #[test]
    fn the_relay_step_wait_fits_inside_the_drain_time() {
        let s = RelaySettings::new("k".to_owned(), "c".to_owned());
        let wait = crate::adapters::decision_log::relay::step_shutdown_wait(&s, DECISION_LOG_DRAIN_BUDGET);
        assert!(
            wait < DECISION_LOG_DRAIN_BUDGET + DECISION_LOG_DRAIN_GRACE,
            "the relay waits {wait:?} for a KV step, and drain aborts it after {:?}",
            DECISION_LOG_DRAIN_BUDGET + DECISION_LOG_DRAIN_GRACE
        );
        assert!(wait <= DECISION_LOG_DRAIN_BUDGET, "the step wait is above the relay's own budget");
    }

    /// Spawn a task that returns `Ok(())` only once the shutdown broadcast fires.
    fn spawn_until_shutdown(servers: &mut JoinSet<anyhow::Result<()>>, mut rx: watch::Receiver<()>) {
        servers.spawn(async move {
            let _ = rx.changed().await;
            Ok(())
        });
    }

    #[tokio::test]
    async fn early_error_is_surfaced_and_triggers_shutdown() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        // A peer that only ends when told to shut down — proves the broadcast reaches it.
        spawn_until_shutdown(&mut servers, rx.clone());
        // A task that fails immediately.
        servers.spawn(async { Err(anyhow::anyhow!("boom")) });

        // `shutdown` never fires; the only way out is the failing task.
        let result = supervise(servers, pending(), tx, None, None, None).await;

        let err = result.expect_err("a failing task must surface as Err");
        assert_eq!(err.to_string(), "boom");
    }

    #[tokio::test]
    async fn clean_shutdown_drains_all_to_ok() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        spawn_until_shutdown(&mut servers, rx.clone());

        // `shutdown` is ready immediately → supervise broadcasts, both tasks drain Ok.
        let result = supervise(servers, ready(()), tx, None, None, None).await;

        assert!(result.is_ok(), "clean shutdown must drain all tasks to Ok, got {result:?}");
    }

    #[tokio::test]
    async fn early_clean_return_warns_not_errors() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        // A task that returns Ok before any shutdown — the warn branch, not an error.
        servers.spawn(async { Ok(()) });

        let result = supervise(servers, pending(), tx, None, None, None).await;

        assert!(result.is_ok(), "a clean early return is not an error, got {result:?}");
    }

    #[tokio::test]
    async fn error_surfaced_even_when_shutdown_wins_the_select() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        // A task that fails immediately, while shutdown is ALSO ready.
        servers.spawn(async { Err(anyhow::anyhow!("late boom")) });

        // Whichever `select!` arm wins, the drain must still surface the error.
        let result = supervise(servers, ready(()), tx, None, None, None).await;

        let err = result.expect_err("the error must survive even when shutdown wins the select");
        assert_eq!(err.to_string(), "late boom");
    }

    use crate::test_support::counter;
    use metrics_util::debugging::DebuggingRecorder;
    use paigasus_observability::names;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio_util::task::TaskTracker;

    #[test]
    fn the_drain_budget_is_five_seconds() {
        assert_eq!(CHARGE_DRAIN_BUDGET, Duration::from_secs(5));
    }

    /// D16: a charge still running after the budget is lost and counted as `reason="shutdown"`.
    #[tokio::test]
    async fn a_charge_still_running_after_the_budget_is_counted_as_dropped() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let charges = TaskTracker::new();
        charges.spawn(std::future::pending::<()>());
        assert_eq!(drain_charges(&charges, Duration::from_millis(50)).await, 1);
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, &[("reason", "shutdown")]), Some(1));
    }

    /// P-18: the decision log drains after the servers. A server that records a decision during its own
    /// graceful stop is seen by the writer. A stand-in writer reads the queue after the signal.
    #[tokio::test]
    async fn supervise_drains_the_decision_log_after_the_servers() {
        use crate::adapters::decision_log::RedisDecisionLog;
        use crate::domain::decision_log::DecisionLog;
        use paigasus_gateway_core::decision_log::{RecordHeader, ReportedCall, build_response};
        use paigasus_gateway_core::{Basis, BlockCode, DecisionContext, ReportDecision, ReportEntry};
        use std::sync::atomic::AtomicUsize;

        let (log, mut queue) = RedisDecisionLog::new(8);
        let log = Arc::new(log);
        let seen = Arc::new(AtomicUsize::new(0));
        let (dl_tx, mut dl_rx) = watch::channel(());
        let writer_seen = Arc::clone(&seen);
        let writer = tokio::spawn(async move {
            let _ = dl_rx.changed().await;
            while let Ok(_event) = queue.try_recv() {
                writer_seen.fetch_add(1, Ordering::SeqCst);
            }
        });
        let idle = tokio::spawn(std::future::pending::<()>());
        let relay = tokio::spawn(async {});
        let tasks = DecisionLogTasks {
            shutdown: dl_tx,
            writer,
            relay,
            connector: idle,
        };

        let (tx, mut rx) = watch::channel(());
        let mut servers = JoinSet::new();
        let server_log = Arc::clone(&log);
        servers.spawn(async move {
            let _ = rx.changed().await;
            // A graceful stop that takes a moment and records one decision on the way.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let entry = ReportEntry {
                choice: 0,
                position: 0,
                name: "read_file".into(),
                decision: ReportDecision::Block,
                basis: Some(Basis::Decider),
                code: Some(BlockCode::Policy),
                enforced: Some(true),
                reason: None,
            };
            let ctx = DecisionContext::new("prn:pgs:iam:::user/u", "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1", None);
            let header = RecordHeader::new(uuid::Uuid::nil(), std::time::SystemTime::UNIX_EPOCH, uuid::Uuid::nil(), 1, None);
            server_log.record(build_response(header, &ctx, &ReportedCall::new(entry, None, None)));
            Ok(())
        });
        let result = supervise(servers, ready(()), tx, None, None, Some(tasks)).await;
        assert!(result.is_ok());
        assert_eq!(seen.load(Ordering::SeqCst), 1, "the writer saw the decision that a server recorded while it stopped");
    }

    #[tokio::test]
    async fn supervise_drains_the_charges_after_the_servers() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        let charges = TaskTracker::new();
        let landed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&landed);
        charges.spawn(async move {
            tokio::task::yield_now().await;
            flag.store(true, Ordering::SeqCst);
        });
        let result = supervise(servers, ready(()), tx, Some(charges), None, None).await;
        assert!(result.is_ok());
        assert!(landed.load(Ordering::SeqCst), "supervise waits for an in-flight charge");
    }

    #[tokio::test]
    async fn supervise_drains_the_history_tasks() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        let history = TaskTracker::new();
        let landed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&landed);
        history.spawn(async move {
            tokio::task::yield_now().await;
            flag.store(true, Ordering::SeqCst);
        });
        let result = supervise(servers, ready(()), tx, None, Some(history), None).await;
        assert!(result.is_ok());
        assert!(landed.load(Ordering::SeqCst), "supervise waits for an in-flight history task");
    }

    #[tokio::test]
    async fn a_history_task_past_the_budget_is_lost_and_logged() {
        let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
        let history = TaskTracker::new();
        history.spawn(std::future::pending::<()>());
        assert_eq!(drain_history(&history, Duration::from_millis(50)).await, 1);
        assert!(logs.text().contains("tool history tasks still running"), "{}", logs.text());
    }
}
