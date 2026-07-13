// SPDX-License-Identifier: Apache-2.0

//! paigasus-gateway composition root: load + validate config, init logging, serve
//! `/healthz`+`/readyz` on one HTTP port, and shut down gracefully on SIGINT/SIGTERM. G4-G8
//! add the IAM/OpenAI clients, auth middleware, egress, and chat handler this composes; the
//! gateway is simpler than `paigasus-iam` — one HTTP port, no gRPC server, no DB, no
//! background relays.

use paigasus_gateway::adapters::http::router;
use paigasus_gateway::config::GatewayConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = GatewayConfig::load()?;
    config.validate().map_err(anyhow::Error::msg)?;
    paigasus_logging::init("paigasus-gateway", &config.log_level);

    let app = router();
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
