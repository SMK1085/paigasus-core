// SPDX-License-Identifier: Apache-2.0

//! paigasus-gateway composition root: load + validate config, init logging, construct the IAM +
//! OpenAI clients, assemble the shared [`AppState`], serve `/healthz`+`/readyz` and the protected
//! `/v1/chat/completions` proxy on one HTTP port, and shut down gracefully on SIGINT/SIGTERM. The
//! gateway is simpler than `paigasus-iam` — one HTTP port, no gRPC server, no DB, no background
//! relays.

use std::sync::Arc;
use std::time::Duration;

use paigasus_gateway::adapters::http::{AppState, ResponsesSettings, router};
use paigasus_gateway::adapters::iam::IamClient;
use paigasus_gateway::adapters::limits::build_limits;
use paigasus_gateway::adapters::openai::OpenAiClient;
use paigasus_gateway::config::GatewayConfig;
use paigasus_gateway::domain::decision_log::OsEntropy;
use paigasus_gateway::domain::limits::SystemClock;
use paigasus_gateway::runtime;
use paigasus_gateway::service_info::Capabilities;
use paigasus_observability::names;
use tokio::task::JoinSet;

/// Dispatch before any runtime is built. `healthcheck` is what the image's `HEALTHCHECK` runs:
/// the images are shell-less, so the binary probes itself (SMA-500 D4).
///
/// Exit codes: 0 healthy, 1 unhealthy, 2 usage error.
fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("decisions") {
        return decisions(&argv[1..]);
    }
    match paigasus_observability::health::dispatch(std::env::args().skip(1)) {
        Ok(paigasus_observability::health::Mode::Serve) => match serve() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Error: {error:?}");
                std::process::ExitCode::FAILURE
            }
        },
        Ok(paigasus_observability::health::Mode::Healthcheck { path }) => healthcheck(&path),
        Err(usage) => {
            eprintln!("{usage}");
            std::process::ExitCode::from(2)
        }
    }
}

/// `decisions export` (SMA-755 § 5.11): the operator export of the decision log. Exit codes: 0 done, 1 bad
/// arguments, 2 NATS or stream error, 3 an event of a wrong org, 4 more than `--max-events`.
fn decisions(args: &[String]) -> std::process::ExitCode {
    use paigasus_gateway::application::decision_export::{Exit, USAGE};
    if args.first().map(String::as_str) != Some("export") {
        eprintln!("{USAGE}");
        return std::process::ExitCode::from(Exit::BadArgs.code());
    }
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: no runtime: {e}");
            return std::process::ExitCode::from(Exit::Nats.code());
        }
    };
    let exit = runtime.block_on(paigasus_gateway::adapters::decision_log::reader::run(args[1..].to_vec()));
    std::process::ExitCode::from(exit.code())
}

/// `load()` but deliberately NOT `validate()`: the probe needs only `http_addr`, and
/// `GatewayConfig::validate` rejects an empty OpenAI API key — which would fail the healthcheck
/// for a reason that has nothing to do with health.
///
/// The error text is never printed (see the IAM counterpart: `State.Health.Log` retains it).
fn healthcheck(path: &str) -> std::process::ExitCode {
    let Ok(config) = GatewayConfig::load() else {
        eprintln!("healthcheck: config load failed");
        return std::process::ExitCode::FAILURE;
    };
    match paigasus_observability::health::probe(config.http_addr, path, std::time::Duration::from_secs(2)) {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) | Err(_) => std::process::ExitCode::FAILURE,
    }
}

#[tokio::main]
async fn serve() -> anyhow::Result<()> {
    let config = GatewayConfig::load()?;
    config.validate().map_err(anyhow::Error::msg)?;
    paigasus_logging::init("paigasus-gateway", &config.log_level);

    // Metrics (SMA-446 Unit 2): install the global Prometheus recorder only when `[metrics]` is
    // enabled — `!enabled` means no recorder is installed AND `/metrics` is never mounted below
    // (a `None` handle short-circuits both wiring blocks). `config.validate()` already rejected
    // `metrics.addr == http_addr` above.
    let metrics_handle = config.metrics.enabled.then(|| paigasus_observability::init("paigasus-gateway"));
    // Register `# HELP`/`# TYPE` exposition text for every family this service emits (spec
    // §4.1) — only when a recorder was actually installed above; describing metrics nobody will
    // ever scrape is pointless, and `describe_*!` against no installed recorder is a silent
    // no-op anyway.
    if metrics_handle.is_some() {
        describe_gateway_metrics();
    }

    // SMA-677 D17 "Order": after `paigasus_observability::init`, so `prime_metrics` and the Redis
    // breaker's constructor reach the installed recorder. Eager: with `backend = "redis"`, a Redis
    // that is down fails the boot here (Q14).
    let limits = build_limits(config.limits.as_ref()).await?;
    // SMA-753: the tool history, also after `init` (the metrics) and eager for Redis (spec D10, D11).
    // ONE digester: the history keeps it, and `build_tool_policy` gives the history's digester to the
    // policy cache (spec P3), so the no-key warning is logged once.
    let history_config = paigasus_gateway::adapters::history::history_config_in_use(config.tool_policy.enabled, config.tool_history.as_ref());
    let history = paigasus_gateway::adapters::history::build_tool_history(history_config, Arc::new(SystemClock), Arc::new(OsEntropy), runtime::history_digester(history_config)?).await?;

    // Outbound clients. IAM connects lazily (a dead IAM does not block startup); the OpenAI client
    // is built with the three split timeout budgets. Neither construction logs the key.
    let iam = IamClient::connect(&config.iam).await?;
    let openai = OpenAiClient::new(
        &config.upstream.openai,
        Duration::from_secs(config.connect_timeout_secs),
        Duration::from_secs(config.first_byte_timeout_secs),
        Duration::from_secs(config.stream_idle_timeout_secs),
    )?;

    // SMA-752 D19: the tool policy. Only after `validate` above (see `build_tool_policy`).
    // SMA-755: one recorder for the decider, the request side and the report site (P-8). The decision log
    // never waits for Redis or NATS, so a broker that is down does not block the boot (spec § 5.6).
    let decision_log = runtime::build_decision_log(&config, Arc::new(SystemClock), Arc::new(OsEntropy)).await?;
    let tool_policy_runtime = runtime::build_tool_policy(&config, decision_log.recorder, history.history)?;
    let tool_interception = runtime::tool_interception(&config, &tool_policy_runtime);
    let tool_policy = tool_policy_runtime.policy;

    let state = AppState {
        iam: Arc::new(iam),
        openai: Arc::new(openai),
        max_request_bytes: config.max_request_bytes,
        capabilities: Capabilities::from_config(&config),
        limits: limits.limits,
        dpop_enabled: config.dpop.enabled,
        tool_interception,
        tool_policy: tool_policy.as_ref().map(|(_, tp)| Arc::clone(tp)),
        responses: config.responses.enabled.then(|| {
            tracing::info!("the responses route is on (SMA-769)");
            Arc::new(ResponsesSettings {
                max_request_bytes: config.responses.max_request_bytes,
                max_record_bytes: config.responses.max_record_bytes,
                max_held_bytes: config.responses.max_held_bytes,
            })
        }),
    };
    if config.dpop.enabled {
        tracing::info!("DPoP is on: the protected routes accept Authorization: DPoP with one DPoP proof header; IAM must have authn.dpop on");
    }

    let app = router(state);
    // Same-port `/metrics`: merged onto the main router only when enabled AND no separate
    // `metrics.addr` is configured. `metrics.enabled = false`, or a separate `addr`, leaves `app`
    // untouched — in the `addr` case `/metrics` is served on its own listener below instead.
    let app = match (&metrics_handle, config.metrics.addr) {
        (Some(handle), None) => app.merge(paigasus_observability::metrics_router(handle.clone())),
        _ => app,
    };

    // All long-lived tasks share one graceful-shutdown `watch` and one `JoinSet`, so a failure in
    // any of them (metrics listener, upkeep) propagates via `runtime::supervise` instead of being
    // silently swallowed by a detached task that only logs before dying (SMA-463). Mirrors
    // `paigasus-iam`'s composition root. Every task takes an `rx.clone()` made BEFORE `supervise`
    // sends, so its first `changed().await` waits rather than firing immediately.
    let (tx, rx) = tokio::sync::watch::channel(());
    let mut servers: JoinSet<anyhow::Result<()>> = JoinSet::new();

    // Main HTTP server. The listener is bound up-front so a bind failure aborts startup fast (a
    // gateway that can't bind its public port should fail immediately), then moved into the task.
    let http_listener = tokio::net::TcpListener::bind(config.http_addr).await?;
    {
        let mut rx = rx.clone();
        servers.spawn(async move {
            axum::serve(http_listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.changed().await;
                })
                .await
                .map_err(anyhow::Error::from)
        });
    }

    // Separate metrics listener, only when both enabled AND `metrics.addr` is configured — the
    // RECOMMENDED posture for a public gateway (keeps `/metrics` off the public HTTP port). Binds
    // INSIDE the task so a bind OR serve failure surfaces as a task error the `JoinSet` observes
    // (SMA-463), rather than a detached task that only logged before dying. On the same
    // shutdown-watch as every other task.
    if let (Some(handle), Some(metrics_addr)) = (metrics_handle.clone(), config.metrics.addr) {
        let mut rx = rx.clone();
        let metrics_app = paigasus_observability::metrics_router(handle);
        servers.spawn(async move {
            let listener = tokio::net::TcpListener::bind(metrics_addr).await?;
            tracing::info!(%metrics_addr, "paigasus-gateway metrics listener started");
            axum::serve(listener, metrics_app)
                .with_graceful_shutdown(async move {
                    let _ = rx.changed().await;
                })
                .await
                .map_err(anyhow::Error::from)
        });
    }

    // Periodic Prometheus upkeep (CodeRabbit round-1 fix): `PrometheusBuilder::install_recorder()`
    // (unlike `install()`) does NOT spawn the maintenance task `PrometheusHandle::run_upkeep()`
    // needs to periodically drain/decay histograms — without calling it ourselves, memory grows
    // unbounded over the life of the process. `init()` itself stays runtime-agnostic (it's also
    // called from plain `#[test]` code with no Tokio runtime), so the spawn lives here. Now on the
    // shared `JoinSet` + shutdown-watch (SMA-463) rather than a detached task, so a panic here is
    // observed rather than silently lost.
    if let Some(handle) = metrics_handle.clone() {
        let mut rx = rx.clone();
        servers.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                tokio::select! {
                    _ = interval.tick() => handle.run_upkeep(),
                    _ = rx.changed() => break,
                }
            }
            Ok(())
        });
    }

    // SMA-752 § 6.3.3: the tool-policy cache refresh, on the same shutdown watch. § 6.3.5: with a
    // `[tool_policy.nats]` table, also the NATS invalidation subscription. A broker that is down
    // does not stop the boot; the client connects in the background.
    if let Some((cache, _)) = tool_policy {
        if let Some(events) = runtime::build_policy_events(&config, &cache).await? {
            servers.spawn(events.run(rx.clone()));
        }
        let rx = rx.clone();
        servers.spawn(async move {
            cache.run_refresh(rx).await;
            Ok(())
        });
    }

    tracing::info!(%config.http_addr, "paigasus-gateway started");

    // Supervise: stop on the first of a shutdown signal or any task ending, broadcast graceful
    // shutdown to the rest, and surface the first error (SMA-463).
    runtime::supervise(servers, shutdown_signal(), tx, limits.charge_tasks, history.tasks, decision_log.tasks).await
}

/// Registers `# HELP`/`# TYPE` exposition text for the 38 metric families listed below (SMA-755 counted
/// them; the number is a plain count, not a gate). `paigasus-gateway` emits them (spec §4.1 and later), via the `names::` consts so this can't drift from `names::ALL`. Mirrors
/// the meanings documented in `docs/ops/RUNBOOK-observability.md` §2.3.
fn describe_gateway_metrics() {
    use metrics::{describe_counter, describe_gauge, describe_histogram};

    describe_counter!(
        names::GATEWAY_HTTP_REQUESTS_TOTAL,
        "HTTP requests handled by the gateway's HTTP router, labeled by route, method, and status_class."
    );
    describe_histogram!(
        names::GATEWAY_HTTP_REQUEST_DURATION_SECONDS,
        "Gateway HTTP request latency in seconds (time-to-first-byte for streaming chat completions, not full stream duration)."
    );
    describe_gauge!(names::GATEWAY_HTTP_INFLIGHT_REQUESTS, "Requests currently being handled on the gateway HTTP router.");
    describe_counter!(
        names::GATEWAY_IAM_CALLS_TOTAL,
        "Calls the gateway's auth middleware makes to IAM (introspect/introspect_token/authorize), labeled by operation and result."
    );
    describe_histogram!(names::GATEWAY_IAM_CALL_DURATION_SECONDS, "Latency of gateway-to-IAM calls in seconds.");
    describe_counter!(
        names::GATEWAY_UPSTREAM_REQUESTS_TOTAL,
        "OpenAI upstream calls made by the gateway, labeled by status_class (time-to-first-byte only for streaming responses)."
    );
    describe_histogram!(
        names::GATEWAY_UPSTREAM_REQUEST_DURATION_SECONDS,
        "OpenAI upstream call latency in seconds (time-to-first-byte only for streaming responses)."
    );
    describe_counter!(
        names::GATEWAY_LIMIT_REFUSALS_TOTAL,
        "Chat requests refused by a limit before egress, labeled by reason (principal_rate, org_rate, org_budget)."
    );
    describe_counter!(
        names::GATEWAY_TOKENS_CHARGED_TOTAL,
        "Tokens the charge guard sent to the limit store, labeled by source (reported, estimated)."
    );
    describe_counter!(names::GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL, "Chat requests whose scope names no organization. Expected 0.");
    describe_counter!(
        names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL,
        "Limit-store calls that failed or met an open breaker, labeled by op (check, charge) and kind (io, server, decode). A check here is a request admitted with no limit (fail-open)."
    );
    describe_counter!(
        names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL,
        "Limit charges that were not sent, labeled by reason (no_runtime, shutdown, period_expired)."
    );
    describe_gauge!(
        names::GATEWAY_REDIS_BREAKER_STATE,
        "The limits Redis circuit breaker: 0 closed, 1 half-open, 2 open. Aggregate max by (job, role), never sum."
    );
    describe_counter!(
        names::GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL,
        "Transitions of the limits Redis circuit breaker, labeled by role and to."
    );
    describe_counter!(
        names::GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL,
        "Tool-policy loads, labeled by result (ok, source_error, compile_error, timeout). A timeout counts once per load."
    );
    describe_histogram!(names::GATEWAY_TOOL_POLICY_LOAD_SECONDS, "Seconds one tool-policy source call takes.");
    describe_histogram!(names::GATEWAY_TOOL_POLICY_COMPILE_SECONDS, "Seconds one tool-policy compile takes.");
    describe_gauge!(
        names::GATEWAY_TOOL_POLICY_STALE_ENTRIES,
        "Tool-policy cache entries whose last refresh failed. They are served until max_stale_secs."
    );
    describe_counter!(names::GATEWAY_TOOL_POLICY_EVICTIONS_TOTAL, "Tool-policy tenant entries evicted for idleness.");
    describe_counter!(names::GATEWAY_TOOL_POLICY_REFRESH_ERRORS_TOTAL, "Tool-policy cache entries whose refresh failed.");
    describe_counter!(
        names::GATEWAY_TOOL_POLICY_DECISIONS_TOTAL,
        "Tool-policy decisions, labeled by side (request, response), mode, effect (allow, observe, block) and basis (policy, errored_forbid, unavailable)."
    );
    describe_counter!(
        names::GATEWAY_TOOL_POLICY_REQUEST_REFUSALS_TOTAL,
        "Requests the tool policy refused, and refusals that the mode did not enforce, labeled by mode, reason (unreadable_turn, unknown_tool_choice, unreplaceable_result) and refused (true, false)."
    );
    describe_histogram!(
        names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS,
        "Seconds the tool policy adds to one request: the request side plus the decider."
    );
    describe_counter!(
        names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL,
        "Tool-policy findings, labeled by side (request, response), kind, detector and severity. The labels come from a closed table of triples; the rest is other."
    );
    describe_histogram!(
        names::GATEWAY_TOOL_POLICY_DETECTOR_SECONDS,
        "Seconds one detector stage takes, labeled by side (request, response) and stage (built_in, webhook, judge)."
    );
    describe_counter!(
        names::GATEWAY_TOOL_HISTORY_STORE_ERRORS_TOTAL,
        "Tool-history store calls that failed or ran past the request budget, labeled by op (lookup, write, reserve, settle, append) and kind (io, server, decode, deadline)."
    );
    describe_counter!(
        names::GATEWAY_TOOL_HISTORY_IDS_CAPPED_TOTAL,
        "Ids that a request sent past the cap of 128 in one list, labeled by list (calls, results, refs)."
    );
    describe_counter!(
        names::GATEWAY_TOOL_HISTORY_EVENTS_TRIMMED_TOTAL,
        "Tool-history events that a write removed, labeled by reason (retention, cap)."
    );
    describe_counter!(
        names::GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL,
        "Tool-policy cache invalidations from NATS, labeled by cause (message, connect, resubscribe, slow_consumer)."
    );
    describe_gauge!(names::GATEWAY_TOOL_POLICY_NATS_CONNECTED, "1 while the tool-policy NATS client is connected, else 0.");
    describe_counter!(names::GATEWAY_DECISION_LOG_RECORDED_TOTAL, "Tool decision records that the decision log accepted.");
    describe_counter!(names::GATEWAY_DECISION_LOG_WRITTEN_TOTAL, "Tool decision events that the writer put into the Redis stream.");
    describe_counter!(
        names::GATEWAY_DECISION_LOG_DROPPED_TOTAL,
        "Tool decision events that the gateway dropped, labeled by reason (queue_full, closed, redis_unavailable, write_error, breaker_open, backlog_full, publish_rejected, redis_deleted, shutdown)."
    );
    describe_counter!(
        names::GATEWAY_DECISION_LOG_TASK_RESTARTS_TOTAL,
        "Panics that a decision-log task caught and survived, labeled by task (writer, relay)."
    );
    describe_counter!(
        names::GATEWAY_DECISION_LOG_PUBLISHED_TOTAL,
        "Tool decision events that the relay published to JetStream and that got a PubAck."
    );
    describe_counter!(
        names::GATEWAY_DECISION_LOG_REDIS_RESETS_TOTAL,
        "Redis losses of the decision stream that the relay detected (the consumer group was gone). One per incident."
    );
    describe_counter!(
        names::GATEWAY_DECISION_LOG_LOST_ESTIMATE_TOTAL,
        "An estimate of the decision events in the Redis losses: the last 15 s XLEN sample of the relay. It can undercount or overcount."
    );
    describe_histogram!(names::GATEWAY_DECISION_LOG_PUBLISH_SECONDS, "Seconds from the publish of a decision event to its JetStream PubAck.");
    describe_gauge!(names::GATEWAY_DECISION_LOG_BACKLOG, "Entries in the decision stream in Redis (XLEN), sampled every 15 s by the relay.");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("install SIGTERM handler").recv().await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
