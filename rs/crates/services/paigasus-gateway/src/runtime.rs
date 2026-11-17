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
use paigasus_observability::names;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_util::task::TaskTracker;

use crate::adapters::iam::client::build_channel;
use crate::adapters::nats::PolicyEvents;
use crate::adapters::policy_source::file::FilePolicySource;
use crate::adapters::policy_source::iam::IamPolicySource;
use crate::application::tool_policy::{CacheSettings, CedarDecider, Invalidate, PolicyCache, RequestToolPolicy};
use crate::config::{GatewayConfig, PolicySourceConfig};
use crate::domain::limits::ChargeDropReason;
use crate::domain::tool_policy::PolicySource;

/// D16: how long shutdown waits for in-flight limit charges.
pub const CHARGE_DRAIN_BUDGET: Duration = Duration::from_secs(5);

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
/// - Callers must pass either a non-empty `servers` or a `shutdown` that resolves; an
///   empty set with a non-resolving `shutdown` would disable both `select!` arms and
///   wait forever. The gateway always spawns its main HTTP task, so it never hits this.
pub async fn supervise(mut servers: JoinSet<anyhow::Result<()>>, shutdown: impl Future<Output = ()>, tx: watch::Sender<()>, charges: Option<TaskTracker>) -> anyhow::Result<()> {
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
    if let Some(charges) = charges {
        drain_charges(&charges, CHARGE_DRAIN_BUDGET).await;
    }
    result
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

/// SMA-752 D19: what the composition root wires for the tool policy.
pub struct ToolPolicyRuntime {
    /// The cache (for its refresh task) and the request side. `None` with `[tool_policy] enabled = false`.
    pub policy: Option<(Arc<PolicyCache>, Arc<RequestToolPolicy>)>,
    /// The decider for tool interception. The Cedar decider only with the tool policy on: without
    /// the request side there is no snapshot, and the Cedar decider would block every call (fail
    /// closed). Otherwise `AllowAll`, as before SMA-752.
    pub decider: Arc<dyn ToolCallDecider>,
}

/// Build the tool policy from a config that passed [`GatewayConfig::validate`]: `settings()` panics
/// on an org id that `validate` refuses. With `enabled`, `validate` has made sure that tool
/// interception is on, that `on_policy_unavailable` is set (D9) and that a source is set.
///
/// # Errors
///
/// `enabled = true` with no `[tool_policy.source]` (a config that skipped `validate`).
pub fn build_tool_policy(config: &GatewayConfig) -> anyhow::Result<ToolPolicyRuntime> {
    if !config.tool_policy.enabled {
        return Ok(ToolPolicyRuntime {
            policy: None,
            decider: Arc::new(AllowAll),
        });
    }
    let settings = config.tool_policy.settings();
    let source: Arc<dyn PolicySource> = match &config.tool_policy.source {
        Some(PolicySourceConfig::File { dir }) => Arc::new(FilePolicySource::new(dir)),
        // The channel is lazy: an IAM that is down at boot does not stop the gateway.
        Some(PolicySourceConfig::Iam {}) => {
            let credential_file = config
                .iam
                .service_credential_file
                .clone()
                .expect("GatewayConfig::validate requires iam.service_credential_file with tool_policy.source kind = \"iam\"");
            Arc::new(IamPolicySource::new(build_channel(&config.iam)?, credential_file))
        }
        None => anyhow::bail!("tool_policy.enabled = true requires tool_policy.source"),
    };
    let cache = PolicyCache::new(source, CacheSettings::from_settings(&settings));
    tracing::info!(
        mode = settings.default_mode.as_str(),
        "the tool policy is on (SMA-752): tool definitions, results and calls are checked against the Cedar policies"
    );
    Ok(ToolPolicyRuntime {
        policy: Some((Arc::clone(&cache), Arc::new(RequestToolPolicy::new(cache, settings)))),
        decider: Arc::new(CedarDecider),
    })
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
        out[0]
    }

    /// SMA-752 D19, Task 16 ruling (b): with the tool policy off, nothing is built and the decider
    /// is `AllowAll`, as before SMA-752.
    #[tokio::test]
    async fn the_tool_policy_off_builds_nothing_and_allows_every_call() {
        let cfg = config(&format!("{BASE}[tool_interception]\nenabled = true\n"), true);
        let rt = build_tool_policy(&cfg).expect("builds");
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
        let rt = build_tool_policy(&cfg).expect("builds");
        assert!(rt.policy.is_some());
        assert_eq!(decide_one(&rt.decider).await, Decision::Block { code: BlockCode::PolicyUnavailable });
    }

    /// A config that skipped `validate` and has no source is an error, not a panic.
    #[test]
    fn the_tool_policy_on_without_a_source_is_an_error() {
        let cfg = config(&format!("{BASE}[tool_policy]\nenabled = true\non_policy_unavailable = \"open\"\n"), false);
        let err = build_tool_policy(&cfg).err().expect("an error");
        assert!(err.to_string().contains("tool_policy.source"), "{err}");
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
        let cache = build_tool_policy(&tp_config(true, None)).expect("builds").policy.expect("on").0;

        assert!(build_policy_events(&tp_config(true, None), &cache).await.expect("no error").is_none(), "no nats table: refresh only");
        assert!(build_policy_events(&tp_config(false, Some(&url)), &cache).await.expect("no error").is_none(), "the tool policy is off");
        let events = tokio::time::timeout(Duration::from_secs(5), build_policy_events(&tp_config(true, Some(&url)), &cache))
            .await
            .expect("the broker is not waited for")
            .expect("a down broker is not an error");
        assert!(events.is_some(), "on with a nats table: the subscription is built");
    }

    /// A creds file that does not exist is a boot error that names the path, not the URL.
    #[tokio::test]
    async fn a_missing_creds_file_is_a_boot_error_without_the_url() {
        let mut cfg = tp_config(true, Some("nats://user:hunter2@127.0.0.1:1"));
        cfg.tool_policy.nats.as_mut().expect("nats").creds_file = Some("/nonexistent/gw.creds".to_string());
        let cache = build_tool_policy(&cfg).expect("builds").policy.expect("on").0;
        let err = build_policy_events(&cfg, &cache).await.err().expect("an error");
        let text = format!("{err:#}");
        assert!(text.contains("/nonexistent/gw.creds"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::{pending, ready};

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
        let result = supervise(servers, pending(), tx, None).await;

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
        let result = supervise(servers, ready(()), tx, None).await;

        assert!(result.is_ok(), "clean shutdown must drain all tasks to Ok, got {result:?}");
    }

    #[tokio::test]
    async fn early_clean_return_warns_not_errors() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        // A task that returns Ok before any shutdown — the warn branch, not an error.
        servers.spawn(async { Ok(()) });

        let result = supervise(servers, pending(), tx, None).await;

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
        let result = supervise(servers, ready(()), tx, None).await;

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
        let result = supervise(servers, ready(()), tx, Some(charges)).await;
        assert!(result.is_ok());
        assert!(landed.load(Ordering::SeqCst), "supervise waits for an in-flight charge");
    }
}
