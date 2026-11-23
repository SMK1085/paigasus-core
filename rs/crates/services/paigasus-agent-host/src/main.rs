// SPDX-License-Identifier: Apache-2.0

//! paigasus-agent-host composition root (spec §6.5). The boot order is:
//!
//! 1. load and validate the configuration, 2. start logging, 3. read the `[[agents]]` list and
//!    check each model against the model registry (spec §6.2), 4. install the metrics recorder,
//! 5. connect to the database (and, when the worker pool starts, the lease pool), 6. bind the HTTP
//!    listener (and the metrics listener), 7. serve the boot router, 8. start the Prometheus
//!    upkeep task, 9. migrate under the advisory lock, 10. when `[nats]` is set, check each
//!    channel inbox and the durable (spec §9.1), 11. install the application router in the boot
//!    slot, 12. start the NATS source, the scheduler loop (when `timers.enabled` is true) and,
//!    when the pool starts, the worker pool, 13. wait for a shutdown signal and drain.
//!
//! The binary connects to the database first and binds second, as `paigasus-iam` does. A replica
//! that migrates, or that waits for the lock, is then present and not ready, and not absent.

use std::sync::Arc;
use std::time::Duration;

use paigasus_agent_host::adapters::agents::ConfigAgentResolver;
use paigasus_agent_host::adapters::boot::{BootSlot, boot_http_router};
use paigasus_agent_host::adapters::helikon::memory_tool::run_nonce;
use paigasus_agent_host::adapters::helikon::model_registry::ModelRegistry;
use paigasus_agent_host::adapters::helikon::runner::HelikonAgentRunner;
use paigasus_agent_host::adapters::http::{AppState, Ingress, app_router};
use paigasus_agent_host::adapters::iam::IamClient;
use paigasus_agent_host::adapters::memory_commit::MemoryCommit;
use paigasus_agent_host::adapters::nats::consumer::{NatsSource, bind_channels, connect_and_check};
use paigasus_agent_host::adapters::persistence::inbox_repository::PgInboxRepository;
use paigasus_agent_host::adapters::persistence::lease_store::PgLeaseStore;
use paigasus_agent_host::adapters::persistence::migration_lock::migrate_under_lock;
use paigasus_agent_host::adapters::persistence::run_commit::PgRunCommitter;
use paigasus_agent_host::adapters::persistence::scheduler::PgScheduler;
use paigasus_agent_host::application::scheduler_loop::run_scheduler_loop;
use paigasus_agent_host::application::worker::pool::{PoolSettings, holder_prefix, run_worker_pool};
use paigasus_agent_host::application::worker::ports::RunLimits;
use paigasus_agent_host::application::worker::run::{RunWorker, WorkerPorts, WorkerSettings};
use paigasus_agent_host::config::Config;
use paigasus_agent_host_core::agent::AgentDefinition;
use paigasus_agent_host_core::fault::FaultPoint;
use paigasus_agent_host_core::ports::{InboxRepository, LeaseRepository, Scheduler};
use paigasus_agent_host_core::screen::AllowAll;
use paigasus_agent_host_core::timer::BundledTzResolver;
use paigasus_observability::names;
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
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

    // Spec §6.2: an agent that names an unregistered model is a config error. It stops the boot
    // before the database is touched.
    let agents = config.agent_definitions()?;
    let models = Arc::new(model_registry()?);
    models.check(&agents)?;
    let pool_starts = if !config.worker.enabled {
        tracing::info!("worker.enabled is false; the worker pool is off");
        false
    } else if agents.is_empty() {
        tracing::warn!("no [[agents]] is configured; the worker pool does not start");
        false
    } else {
        true
    };

    // The recorder exists only when metrics are on. Without it, `/metrics` is never mounted.
    let metrics_handle = config.metrics.enabled.then(|| paigasus_observability::init("paigasus-agent-host"));
    if metrics_handle.is_some() {
        describe_agent_host_metrics();
    }

    // Spec §6.1: the main pool has room for two connections per run plus the ingress, and the
    // lease store has its own pool, so a busy main pool cannot delay a heartbeat.
    let concurrency = config.worker.concurrency;
    let db = connect(config.database_url.as_str(), 2 * concurrency + 8).await?;
    let lease_db = if pool_starts { Some(connect(config.database_url.as_str(), concurrency + 1).await?) } else { None };

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
        r = boot(&db, lease_db, &config, &slot, agents, models) => r,
        () = &mut shutdown => {
            tracing::info!("shutdown signal received during boot");
            interrupted = true;
            Ok(BootTasks::default())
        }
    };
    let BootTasks { nats, scheduler, workers } = match outcome {
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
    if let Some(WorkerPoolTask {
        worker,
        leases,
        settings,
        holder_prefix,
    }) = workers
    {
        let rx = rx.clone();
        // The pool ends on the shutdown watch: it cancels its runs and releases their leases.
        // `drain` aborts it after `DRAIN_TIMEOUT`, and an aborted lease expires.
        servers.spawn(async move { run_worker_pool(worker, leases, settings, holder_prefix, rx).await });
        tracing::info!(concurrency = settings.concurrency, "the worker pool started");
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
    workers: Option<WorkerPoolTask>,
}

/// The worker pool (spec §6.1): the run worker, the lease repository of the take, and the pool
/// settings. `serve` spawns `run_worker_pool` with them.
struct WorkerPoolTask {
    worker: Arc<RunWorker>,
    leases: Arc<dyn LeaseRepository>,
    settings: PoolSettings,
    holder_prefix: String,
}

/// A database pool of at most `max_connections` connections.
async fn connect(url: &str, max_connections: u32) -> Result<DatabaseConnection, sea_orm::DbErr> {
    let mut options = ConnectOptions::new(url);
    options.max_connections(max_connections);
    Database::connect(options).await
}

/// The model registry. In production it is empty (spec §6.2): the real provider is SMA-851. The
/// `test-model` feature registers the file-scripted `scripted` model.
fn model_registry() -> anyhow::Result<ModelRegistry> {
    #[cfg_attr(not(feature = "test-model"), allow(unused_mut))]
    let mut models = ModelRegistry::default();
    #[cfg(feature = "test-model")]
    paigasus_agent_host::adapters::test_model::register(&mut models)?;
    Ok(models)
}

/// The fault point of the run commit and the run worker. Only the `test-model` feature has one:
/// the file barrier of the SIGKILL test.
fn fault_point() -> Arc<dyn FaultPoint> {
    #[cfg(feature = "test-model")]
    if let Some(barrier) = paigasus_agent_host::adapters::test_model::FileBarrier::from_env() {
        tracing::warn!("the test barrier is on");
        return Arc::new(barrier);
    }
    Arc::new(paigasus_agent_host_core::fault::NoFaults)
}

/// Build the worker pool on the real adapters. The take, the heartbeat and the release use the
/// lease pool. The claim, the run and the commit use the main pool.
fn worker_pool_task(
    db: &DatabaseConnection,
    lease_db: DatabaseConnection,
    config: &Config,
    agents: Vec<AgentDefinition>,
    models: Arc<ModelRegistry>,
    memory: MemoryCommit,
) -> anyhow::Result<WorkerPoolTask> {
    let faults = fault_point();
    let caps = config.memory.caps.to_caps();
    let heartbeat_timeout = config.lease.heartbeat_statement_timeout();
    let resolver = ConfigAgentResolver::new(db.clone(), agents);
    let leases = Arc::new(PgLeaseStore::new(lease_db, heartbeat_timeout).serving(resolver.served()));
    let ports = WorkerPorts {
        leases: leases.clone(),
        claims: Arc::new(PgLeaseStore::new(db.clone(), heartbeat_timeout)),
        committer: Arc::new(PgRunCommitter::new(db.clone(), memory.clone(), caps, Arc::clone(&faults))),
        resolver: Arc::new(resolver),
        screen: Arc::new(AllowAll),
        runner: Arc::new(HelikonAgentRunner::new(db.clone(), models)),
        memory: memory.store(),
        faults,
        nonce: run_nonce,
    };
    let settings = WorkerSettings {
        claim: config.claim.to_claim_config(),
        timing: config.lease.keeper_timing()?,
        limits: RunLimits {
            max_turns: config.run.max_turns,
            timeout: Duration::from_secs(config.run.timeout_secs),
        },
        retry: config.retry.to_policy(),
        max_runs_per_lease: config.worker.max_runs_per_lease,
        caps,
    };
    Ok(WorkerPoolTask {
        worker: Arc::new(RunWorker::new(ports, settings)),
        leases,
        settings: PoolSettings {
            concurrency: config.worker.concurrency,
            poll_interval: Duration::from_millis(config.worker.poll_interval_ms),
            lease_ttl: Duration::from_secs(config.lease.ttl_secs),
        },
        holder_prefix: holder_prefix(),
    })
}

/// Everything between the bind and ready: the migration, the NATS boot checks, the build of the
/// worker pool (when `lease_db` is set) and the install of the application router. It returns an
/// error and does not use `?` out of `serve`, so that `serve` always drains.
async fn boot(db: &DatabaseConnection, lease_db: Option<DatabaseConnection>, config: &Config, slot: &BootSlot, agents: Vec<AgentDefinition>, models: Arc<ModelRegistry>) -> anyhow::Result<BootTasks> {
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
    // One commit mode for the process: `memory.backend` selects it, and the worker pool and the
    // application state share it. For `s3` the build runs the boot probe (SMA-829).
    let memory = MemoryCommit::build(&config.memory, db.clone(), utf8_env()).await?;
    // `serve` sets `lease_db` only when the pool starts: `worker.enabled` and an agent.
    let workers = match lease_db {
        Some(lease_db) => Some(worker_pool_task(db, lease_db, config, agents, models, memory.clone())?),
        None => None,
    };
    let state = AppState {
        db: db.clone(),
        inboxes,
        ingress,
        memory,
    };
    slot.install(app_router(state, None))?;
    Ok(BootTasks { nats, scheduler, workers })
}

/// The process environment as UTF-8 pairs. `std::env::vars()` panics on a non-UTF-8 value, so a pair
/// with a non-UTF-8 part is dropped.
fn utf8_env() -> impl Iterator<Item = (String, String)> {
    std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
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
        "Inbox leases found lost, labeled by operation (heartbeat, claim, commit, screen_out, back_off, release_after_shutdown or deadline)."
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
    // The worker run loop (SMA-790 spec §6.14). The names are provisional; SMA-798 owns them.
    describe_counter!(
        names::AGENT_HOST_RUNS_TOTAL,
        "Runs that ended, labeled by class (completed, stopped, deterministic, transient, screened, discarded or shutdown)."
    );
    describe_histogram!(names::AGENT_HOST_RUN_DURATION_SECONDS, "Duration of one run, from the claim to the end of the commit, in seconds.");
    describe_counter!(names::AGENT_HOST_EVENTS_SCREENED_TOTAL, "Inbox events that the screen flagged.");
    describe_counter!(names::AGENT_HOST_COMMIT_MEMORY_CONFLICTS_TOTAL, "Run commits that the memory store refused with a version conflict.");
    describe_counter!(names::AGENT_HOST_COMMIT_OUTCOME_UNKNOWN_TOTAL, "Run commits whose COMMIT statement failed, so the outcome is unknown.");
    describe_counter!(names::AGENT_HOST_WORKER_RESTARTS_TOTAL, "Restarts of a worker task of the pool.");
    describe_gauge!(
        names::AGENT_HOST_INBOX_LAG_SECONDS,
        "Age in seconds of the oldest due pending event of the agents that this host serves. It is 0 when nothing is due."
    );
    describe_histogram!(names::AGENT_HOST_EVENT_ATTEMPTS, "The attempts of an inbox event when it goes to done or dead.");
    describe_counter!(
        names::AGENT_HOST_MEMORY_S3_SKIPPED_OBJECTS_TOTAL,
        "Objects under a memory scope that a read skipped or refused, labeled by reason. It counts events at each read, not distinct objects."
    );
    describe_counter!(
        names::AGENT_HOST_MEMORY_S3_PARTIAL_APPLIES_TOTAL,
        "S3 memory change sets that were partly applied. The store may hold part of the set."
    );
    // The name is provisional until SMA-798 moves it to `names::`.
    describe_counter!(
        "agent_host_memory_refusals_total",
        "Memory tool commands that the agent host refused, labeled by kind (conflict, cap, path, sealed, store)."
    );
}
