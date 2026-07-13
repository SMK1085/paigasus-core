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
