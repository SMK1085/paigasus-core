// SPDX-License-Identifier: Apache-2.0

//! paigasus-agent-host composition root (spec §6.5). The boot order is:
//!
//! 1. load and validate the configuration, 2. start logging, 3. install the metrics recorder,
//! 4. connect to the database, 5. bind the HTTP listener (and the metrics listener), 6. serve the
//!    boot router, 7. start the Prometheus upkeep task, 8. migrate under the advisory lock,
//! 9. when `[nats]` is set, check each channel inbox and the durable (spec §9.1), 10. install the
//!    application router in the boot slot, 11. start the NATS source and, when `timers.enabled` is
//!    true, the scheduler loop, 12. wait for a shutdown signal and drain.
//!
//! The binary connects to the database first and binds second, as `paigasus-iam` does. A replica
//! that migrates, or that waits for the lock, is then present and not ready, and not absent.

use std::sync::Arc;
use std::time::Duration;

use paigasus_agent_host::adapters::boot::{BootSlot, boot_http_router};
use paigasus_agent_host::adapters::http::{AppState, Ingress, app_router};
use paigasus_agent_host::adapters::iam::IamClient;
use paigasus_agent_host::adapters::nats::consumer::{NatsSource, bind_channels, connect_and_check};
use paigasus_agent_host::adapters::persistence::inbox_repository::PgInboxRepository;
use paigasus_agent_host::adapters::persistence::migration_lock::migrate_under_lock;
use paigasus_agent_host::adapters::persistence::scheduler::PgScheduler;
use paigasus_agent_host::application::scheduler_loop::run_scheduler_loop;
use paigasus_agent_host::config::Config;
use paigasus_agent_host_core::ports::{InboxRepository, Scheduler};
use paigasus_agent_host_core::timer::BundledTzResolver;
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
    let outcome: anyhow::Result<BootTasks> = tokio::select! {
        r = boot(&db, &config, &slot) => r,
        () = &mut shutdown => {
            tracing::info!("shutdown signal received during boot");
            interrupted = true;
            Ok(BootTasks::default())
        }
    };
    let BootTasks { nats, scheduler } = match outcome {
        Ok(tasks) if !interrupted => tasks,
        Ok(_) => {
            let _ = tx.send(());
            drain(&mut servers).await;
            return Ok(());
        }
        Err(e) => {
            tracing::error!(error = %e, "boot failed after the listener was bound; draining");
            let _ = tx.send(());
            drain(&mut servers).await;
            return Err(e);
        }
    };
    if let Some(NatsTask { source, inboxes }) = nats {
        let rx = rx.clone();
        servers.spawn(async move { source.run(inboxes, rx).await });
        tracing::info!("the NATS source started");
    }
    if let Some(scheduler) = scheduler {
        let rx = rx.clone();
        let settings = config.timers.loop_settings();
        // The loop ends only on the shutdown watch. It reads the watch between rows and never
        // cuts a fire short.
        servers.spawn(async move { run_scheduler_loop(scheduler, settings, rx).await });
        tracing::info!("the scheduler loop started");
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

/// The checked NATS source, and the repository it inserts into. `serve` spawns it.
struct NatsTask {
    source: NatsSource,
    inboxes: Arc<dyn InboxRepository>,
}

/// The tasks that `boot` builds and `serve` spawns after the application router is in place.
#[derive(Default)]
struct BootTasks {
    nats: Option<NatsTask>,
    scheduler: Option<Arc<dyn Scheduler>>,
}

/// Everything between the bind and ready: the migration, the NATS boot checks and the install of
/// the application router. It returns an error and does not use `?` out of `serve`, so that
/// `serve` always drains.
async fn boot(db: &sea_orm::DatabaseConnection, config: &Config, slot: &BootSlot) -> anyhow::Result<BootTasks> {
    let migration = migrate_under_lock(db, config.migration.lock_wait()).await?;
    tracing::info!(
        waited = ?migration.waited,
        polls = migration.polls,
        migrations_applied = migration.migrations_applied,
        "database migrations complete"
    );
    let inboxes: Arc<dyn InboxRepository> = Arc::new(PgInboxRepository::new(db.clone()));
    // Spec §9.1: after the migrations, each channel inbox must exist and the durable filters must
    // be the channel subjects. Each failure is a config error and stops the boot.
    let nats = match &config.nats {
        Some(nats) => {
            let channels = bind_channels(nats, &*inboxes).await?;
            let source = connect_and_check(nats, &channels).await?;
            Some(NatsTask {
                source,
                inboxes: Arc::clone(&inboxes),
            })
        }
        None => {
            tracing::info!("no [nats] section is set; the NATS source is off");
            None
        }
    };
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
    // Only the resolver and the scheduler: the timer store has no caller in this binary yet.
    let scheduler: Option<Arc<dyn Scheduler>> = if config.timers.enabled {
        let tz = Arc::new(BundledTzResolver::new());
        Some(Arc::new(PgScheduler::new(db.clone(), tz, config.timers.scheduler_settings())))
    } else {
        tracing::info!("timers.enabled is false; the scheduler loop is off");
        None
    };
    tracing::info!(worker_enabled = config.worker.enabled, "worker.enabled is read, and SMA-787 starts no worker");
    let state = AppState { db: db.clone(), inboxes, ingress };
    slot.install(app_router(state, None))?;
    Ok(BootTasks { nats, scheduler })
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
    describe_counter!(
        names::AGENT_HOST_NATS_MESSAGES_TERMINATED_TOTAL,
        "NATS messages ended with term because the body is not a valid CloudEvent."
    );
    describe_counter!(
        names::AGENT_HOST_NATS_MESSAGES_TENANCY_SKIPPED_TOTAL,
        "NATS messages acked and skipped because the event does not name the inbox org."
    );
    describe_counter!(
        names::AGENT_HOST_NATS_MESSAGES_UNMAPPED_TOTAL,
        "NATS messages on a subject that no channel maps. Each one is nacked with the maximum delay."
    );
    describe_counter!(
        names::AGENT_HOST_TIMER_FIRED_TOTAL,
        "Timer firings, labeled by kind (follow_up or schedule), origin (agent or system) and outcome (inserted or duplicate)."
    );
    describe_counter!(
        names::AGENT_HOST_TIMER_FIRE_ERRORS_TOTAL,
        "Timer rows that a scheduler pass could not fire, labeled by class (permanent or transient)."
    );
    describe_counter!(names::AGENT_HOST_TIMER_FAILED_TOTAL, "Timer rows set to failed, labeled by kind and origin.");
    describe_counter!(names::AGENT_HOST_TIMER_CANCEL_NOT_ACTIVE_TOTAL, "Buffered timer cancels that found the timer not active.");
    describe_histogram!(names::AGENT_HOST_SCHEDULER_PASS_DURATION_SECONDS, "Duration of one scheduler pass in seconds.");
    describe_gauge!(
        names::AGENT_HOST_SCHEDULER_LAG_SECONDS,
        "Age in seconds of the oldest due active timer after a scheduler pass. It is 0 when nothing is due."
    );
    describe_gauge!(
        names::AGENT_HOST_SCHEDULER_LAST_PASS_TIMESTAMP_SECONDS,
        "Unix time in seconds of the last scheduler pass that finished."
    );
    describe_counter!(names::AGENT_HOST_TIMER_TOOL_CALLS_TOTAL, "Timer tool calls, labeled by tool and outcome (ok or refused).");
    describe_counter!(
        names::AGENT_HOST_MEMORY_S3_SKIPPED_OBJECTS_TOTAL,
        "Objects under a memory scope that a read skipped or refused, labeled by reason. It counts events at each read, not distinct objects."
    );
    // The name is provisional until SMA-798 moves it to `names::`.
    describe_counter!(
        "agent_host_memory_refusals_total",
        "Memory tool commands that the agent host refused, labeled by kind (conflict, cap, path, sealed, store)."
    );
}
