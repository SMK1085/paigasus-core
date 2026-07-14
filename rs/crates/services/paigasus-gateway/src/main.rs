// SPDX-License-Identifier: Apache-2.0

//! paigasus-gateway composition root: load + validate config, init logging, construct the IAM +
//! OpenAI clients, assemble the shared [`AppState`], serve `/healthz`+`/readyz` and the protected
//! `/v1/chat/completions` proxy on one HTTP port, and shut down gracefully on SIGINT/SIGTERM. The
//! gateway is simpler than `paigasus-iam` — one HTTP port, no gRPC server, no DB, no background
//! relays.

use std::sync::Arc;
use std::time::Duration;

use paigasus_gateway::adapters::http::{AppState, router};
use paigasus_gateway::adapters::iam::IamClient;
use paigasus_gateway::adapters::openai::OpenAiClient;
use paigasus_gateway::config::GatewayConfig;
use paigasus_observability::names;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    // Periodic Prometheus upkeep (CodeRabbit round-1 fix): `PrometheusBuilder::install_recorder()`
    // (unlike `install()`) does NOT spawn the maintenance task `PrometheusHandle::run_upkeep()`
    // needs to periodically drain/decay histograms — without calling it ourselves, memory grows
    // unbounded over the life of the process. `init()` itself stays runtime-agnostic (it's also
    // called from plain `#[test]` code with no Tokio runtime), so the spawn lives here instead,
    // only once an async runtime + a real handle both exist. Cloned off `metrics_handle` (the
    // original is still needed below for `metrics_router`); races the same shutdown signal the
    // main server uses so this task stops cleanly rather than lingering after the servers below
    // have shut down.
    if let Some(handle) = metrics_handle.clone() {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            let shutdown = shutdown_signal();
            tokio::pin!(shutdown);
            loop {
                tokio::select! {
                    _ = interval.tick() => handle.run_upkeep(),
                    () = &mut shutdown => break,
                }
            }
        });
    }

    // Outbound clients. IAM connects lazily (a dead IAM does not block startup); the OpenAI client
    // is built with the three split timeout budgets. Neither construction logs the key.
    let iam = IamClient::connect(&config.iam).await?;
    let openai = OpenAiClient::new(
        &config.upstream.openai,
        Duration::from_secs(config.connect_timeout_secs),
        Duration::from_secs(config.first_byte_timeout_secs),
        Duration::from_secs(config.stream_idle_timeout_secs),
    )?;

    let state = AppState {
        iam: Arc::new(iam),
        openai: Arc::new(openai),
        max_request_bytes: config.max_request_bytes,
    };

    let app = router(state);
    // Same-port `/metrics`: merged onto the main router only when enabled AND no separate
    // `metrics.addr` is configured. `metrics.enabled = false`, or a separate `addr`, leaves `app`
    // untouched — in the `addr` case `/metrics` is served on its own listener below instead.
    let app = match (&metrics_handle, config.metrics.addr) {
        (Some(handle), None) => app.merge(paigasus_observability::metrics_router(handle.clone())),
        _ => app,
    };

    // Separate metrics listener, only when both enabled AND `metrics.addr` is configured — the
    // RECOMMENDED posture for a public gateway (keeps `/metrics` off the public HTTP port).
    // `shutdown_signal()` has no captured state, so calling it a second time here is safe: tokio
    // supports multiple independent listeners for the same signal, each notified on delivery — no
    // broadcast channel is needed to share graceful shutdown between the two `axum::serve` tasks.
    if let (Some(handle), Some(metrics_addr)) = (metrics_handle.clone(), config.metrics.addr) {
        let metrics_app = paigasus_observability::metrics_router(handle);
        let metrics_listener = tokio::net::TcpListener::bind(metrics_addr).await?;
        tracing::info!(%metrics_addr, "paigasus-gateway metrics listener started");
        tokio::spawn(async move {
            if let Err(err) = axum::serve(metrics_listener, metrics_app).with_graceful_shutdown(shutdown_signal()).await {
                tracing::error!(%err, "paigasus-gateway metrics listener exited");
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(config.http_addr).await?;
    tracing::info!(%config.http_addr, "paigasus-gateway started");
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;

    Ok(())
}

/// Registers `# HELP`/`# TYPE` exposition text for the 7 metric families `paigasus-gateway`
/// emits (spec §4.1), via the `names::` consts so this can't drift from `names::ALL`. Mirrors
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
        "Calls the gateway's auth middleware makes to IAM (introspect/authorize), labeled by operation and result."
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
