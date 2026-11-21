// SPDX-License-Identifier: Apache-2.0

//! paigasus-agent-host composition root (spec §6.5). The boot order is:
//!
//! 1. load and validate the configuration, 2. start logging, 3. install the metrics recorder,
//! 4. connect to the database, 5. bind the HTTP listener (and the metrics listener), 6. serve the
//!    boot router, 7. start the Prometheus upkeep task, 8. migrate under the advisory lock,
//! 9. install the application router in the boot slot, 10. wait for a shutdown signal and drain.
//!
//! The binary connects to the database first and binds second, as `paigasus-iam` does. A replica
//! that migrates, or that waits for the lock, is then present and not ready, and not absent.

use std::sync::Arc;
use std::time::Duration;

use paigasus_agent_host::adapters::boot::{BootSlot, boot_http_router};
use paigasus_agent_host::adapters::http::{AppState, Ingress, app_router};
use paigasus_agent_host::adapters::iam::IamClient;
use paigasus_agent_host::adapters::persistence::inbox_repository::PgInboxRepository;
use paigasus_agent_host::adapters::persistence::migration_lock::migrate_under_lock;
use paigasus_agent_host::config::Config;
use paigasus_observability::names;
use sea_orm::Database;
use tokio::task::JoinSet;

/// How long the drain waits for the server tasks after the shutdown broadcast.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Dispatch before any runtime is built. `healthcheck` is what the image `HEALTHCHECK` runs: the
/// image has no shell, so the binary probes itself.
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

/// Loads the configuration but does not validate it: the probe needs only `http_addr`. The error
/// text is never printed, because Docker keeps the last health outputs and a configuration error
/// can name values from the environment.
fn healthcheck(path: &str) -> std::process::ExitCode {
    let Ok(config) = Config::load() else {
        eprintln!("healthcheck: config load failed");
        return std::process::ExitCode::FAILURE;
    };
    match paigasus_observability::health::probe(config.http_addr, path, Duration::from_secs(2)) {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) | Err(_) => std::process::ExitCode::FAILURE,
    }
}

#[tokio::main]
async fn serve() -> anyhow::Result<()> {
    let config = Config::load()?;
    config.validate()?;
    paigasus_logging::init("paigasus-agent-host", &config.log_level);

    // The recorder exists only when metrics are on. Without it, `/metrics` is never mounted.
    let metrics_handle = config.metrics.enabled.then(|| paigasus_observability::init("paigasus-agent-host"));
    if metrics_handle.is_some() {
        describe_agent_host_metrics();
    }

    let db = Database::connect(config.database_url.as_str()).await?;

    // Bind here, before the migration, so a bind failure shows at once and the socket listens for
    // the whole migration window.
    let http_listener = tokio::net::TcpListener::bind(config.http_addr).await?;
    let metrics_listener = match config.metrics.addr {
        Some(addr) if metrics_handle.is_some() => Some((tokio::net::TcpListener::bind(addr).await?, addr)),
        _ => None,
    };

    // Register the signal listeners before any task serves a request. A SIGTERM that arrives
    // once `/healthz` answers must reach the drain path, not the default handler.
    let mut shutdown = std::pin::pin!(shutdown_signal()?);
    let mut interrupted = false;

    let (tx, rx) = tokio::sync::watch::channel(());
    let mut servers: JoinSet<anyhow::Result<()>> = JoinSet::new();

    // Same-port `/metrics`: only when metrics are on and no separate address is set.
    let http_metrics_router = match (&metrics_handle, config.metrics.addr) {
        (Some(handle), None) => Some(paigasus_observability::metrics_router(handle.clone())),
        _ => None,
    };
    let slot = BootSlot::new();
    {
        let mut rx = rx.clone();
        let app = boot_http_router(slot.clone(), http_metrics_router);
        servers.spawn(async move {
            axum::serve(http_listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.changed().await;
                })
                .await
                .map_err(anyhow::Error::from)
        });
    }
    if let (Some((listener, metrics_addr)), Some(handle)) = (metrics_listener, metrics_handle.clone()) {
        let mut rx = rx.clone();
        let metrics_app = paigasus_observability::metrics_router(handle);
        servers.spawn(async move {
            tracing::info!(%metrics_addr, "paigasus-agent-host metrics listener started");
            axum::serve(listener, metrics_app)
                .with_graceful_shutdown(async move {
                    let _ = rx.changed().await;
                })
                .await
                .map_err(anyhow::Error::from)
        });
    }
    // `install_recorder` does not start the upkeep that drains histograms. Without this task the
    // memory grows for the life of the process. It starts before the migration because `/metrics`
    // is live from the bind.
    if let Some(handle) = metrics_handle {
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
    tracing::info!(%config.http_addr, "paigasus-agent-host listener bound; migrating");

    // One signal registration for the whole life of the process. A second registration can lose a
    // signal that arrives in the gap between the two.
    let outcome: anyhow::Result<()> = tokio::select! {
        r = boot(&db, &config, &slot) => r,
        () = &mut shutdown => {
            tracing::info!("shutdown signal received during boot");
            interrupted = true;
            Ok(())
        }
    };
    if outcome.is_err() || interrupted {
        if let Err(e) = &outcome {
            tracing::error!(error = %e, "boot failed after the listener was bound; draining");
        }
        let _ = tx.send(());
        drain(&mut servers).await;
        return outcome;
    }

    tracing::info!(%config.http_addr, "paigasus-agent-host started");

    // Stop on the first of a shutdown signal or a server task that ends.
    let early_error: Option<anyhow::Error> = tokio::select! {
        () = &mut shutdown => {
            tracing::info!("shutdown signal received");
            None
        }
        Some(joined) = servers.join_next() => match joined {
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
    };
    let _ = tx.send(());
    drain(&mut servers).await;
    early_error.map_or(Ok(()), Err)
}

/// Everything between the bind and ready: the migration and the install of the application
/// router. It returns an error and does not use `?` out of `serve`, so that `serve` always drains.
async fn boot(db: &sea_orm::DatabaseConnection, config: &Config, slot: &BootSlot) -> anyhow::Result<()> {
    let migration = migrate_under_lock(db, config.migration.lock_wait()).await?;
    tracing::info!(
        waited = ?migration.waited,
        polls = migration.polls,
        migrations_applied = migration.migrations_applied,
        "database migrations complete"
    );
    let ingress = match &config.iam {
        Some(iam) => Some(Arc::new(Ingress {
            iam: Arc::new(IamClient::connect(iam)?),
            max_body_bytes: config.ingress.max_body_bytes,
        })),
        None => {
            tracing::info!("no [iam] section is set; the event ingress and the service-info route are off");
            None
        }
    };
    tracing::info!(worker_enabled = config.worker.enabled, "worker.enabled is read, and SMA-787 starts no worker");
    let state = AppState {
        db: db.clone(),
        inboxes: Arc::new(PgInboxRepository::new(db.clone())),
        ingress,
    };
    slot.install(app_router(state, None))?;
    Ok(())
}

/// Wait for the server tasks, at most [`DRAIN_TIMEOUT`]. A task that ignores the shutdown watch
/// is aborted. An unbounded drain would let a failed boot serve 503 for ever.
async fn drain(servers: &mut JoinSet<anyhow::Result<()>>) {
    let drained = tokio::time::timeout(DRAIN_TIMEOUT, async { while servers.join_next().await.is_some() {} }).await;
    if drained.is_err() {
        tracing::warn!(unreaped = servers.len(), "drain timed out with tasks not yet joined");
        servers.abort_all();
    }
}

/// Registers the SIGINT and SIGTERM listeners now and returns a future that resolves on the first
/// of them. The registration is eager: a signal that arrives before the first poll is kept.
fn shutdown_signal() -> anyhow::Result<impl std::future::Future<Output = ()>> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        Ok(async move {
            tokio::select! {
                _ = interrupt.recv() => {},
                _ = terminate.recv() => {},
            }
        })
    }
    #[cfg(not(unix))]
    {
        Ok(async {
            let _ = tokio::signal::ctrl_c().await;
        })
    }
}

/// Registers the `# HELP` and `# TYPE` text of the metric families of this service. The names
/// come from `names::`, so they cannot drift from `names::ALL`.
fn describe_agent_host_metrics() {
    use metrics::{describe_counter, describe_gauge, describe_histogram};

    describe_counter!(
        names::AGENT_HOST_HTTP_REQUESTS_TOTAL,
        "HTTP requests handled by the agent host HTTP router, labeled by route, method, and status_class."
    );
    describe_histogram!(names::AGENT_HOST_HTTP_REQUEST_DURATION_SECONDS, "Agent host HTTP request latency in seconds.");
    describe_gauge!(names::AGENT_HOST_HTTP_INFLIGHT_REQUESTS, "Requests currently being handled on the agent host HTTP router.");
    describe_counter!(names::AGENT_HOST_INBOX_EVENTS_INSERTED_TOTAL, "Events stored as new inbox rows, labeled by channel.");
    describe_counter!(
        names::AGENT_HOST_INBOX_EVENTS_DUPLICATE_TOTAL,
        "Inbox inserts that found a stored row with the same key, labeled by channel."
    );
    describe_counter!(
        names::AGENT_HOST_INBOX_EVENTS_PAYLOAD_MISMATCH_TOTAL,
        "Duplicate inbox inserts whose type or data differ from the stored row, labeled by channel."
    );
    describe_counter!(
        names::AGENT_HOST_LEASE_LOST_TOTAL,
        "Inbox leases found lost, labeled by operation (heartbeat, claim, complete or deadline)."
    );
    describe_counter!(names::AGENT_HOST_CLAIMS_TOTAL, "Committed claims of inbox events. Each claim opens one run.");
    describe_counter!(names::AGENT_HOST_INBOX_EVENTS_DEAD_TOTAL, "Inbox events moved to dead after the maximum number of attempts.");
}
