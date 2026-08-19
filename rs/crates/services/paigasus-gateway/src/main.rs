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
use paigasus_gateway::runtime;
use paigasus_gateway::service_info::Capabilities;
use paigasus_observability::names;
use tokio::task::JoinSet;

/// Dispatch before any runtime is built. `healthcheck` is what the image's `HEALTHCHECK` runs:
/// the images are shell-less, so the binary probes itself (SMA-500 D4).
///
/// Exit codes: 0 healthy, 1 unhealthy, 2 usage error.
fn main() -> std::process::ExitCode {
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
        capabilities: Capabilities::from_config(&config),
    };

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

    tracing::info!(%config.http_addr, "paigasus-gateway started");

    // Supervise: stop on the first of a shutdown signal or any task ending, broadcast graceful
    // shutdown to the rest, and surface the first error (SMA-463).
    runtime::supervise(servers, shutdown_signal(), tx).await
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
