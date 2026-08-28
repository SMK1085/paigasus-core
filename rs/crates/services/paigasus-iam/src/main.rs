// SPDX-License-Identifier: Apache-2.0

//! paigasus-iam composition root: load config, init logging, connect to the DB, BIND all three
//! listeners, then migrate and build `AppState` behind them (SMA-571), and shut down gracefully
//! on SIGINT/SIGTERM.

use std::sync::Arc;
use std::time::Duration;

use paigasus_iam::adapters::boot;
use paigasus_iam::adapters::events::{NatsEventPublisher, OutboxRelay, TracingEventPublisher};
use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::{AppState, serve_http};
use paigasus_iam::adapters::persistence::{OutboxRetentionPolicy, PgOutboxListener, PgOutboxMaintainer, PgPartitionMaintainer, RetentionPolicy, migrate_under_lock};
use paigasus_iam::config::{IamConfig, PublisherBackend};
use paigasus_iam_core::EventPublisher;
use paigasus_observability::names;
use sea_orm::Database;
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
/// `IamConfig::validate` rejects a config with no configured issuers — which would fail the
/// healthcheck for a reason that has nothing to do with health.
///
/// The error text is never printed. Docker retains the last five health-check outputs in
/// `State.Health.Log`, and a `figment::Error` names config keys and can carry values from the
/// `IAM_*` env layer.
fn healthcheck(path: &str) -> std::process::ExitCode {
    let Ok(config) = IamConfig::load() else {
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
    let config = IamConfig::load()?;
    config.validate().map_err(|e| anyhow::anyhow!(e))?;
    paigasus_logging::init("paigasus-iam", &config.log_level);

    // A fresh/empty `authz.bootstrap_admins` is valid config (`IamConfig::validate` allows
    // it — spec §11), but it means a fresh deployment has no way to grant itself the first
    // `platform_admin` role: warn loudly rather than fail boot, since a later `moon`/`psql`
    // operator path can still seed one directly. Task 21b wires the actual JIT seed.
    if config.authz.bootstrap_admins.is_empty() {
        tracing::warn!("no authz.bootstrap_admins configured — a fresh deployment has no platform administrator and cannot create organizations or grant roles");
    }

    // SMA-489 §3.4: inert config, not invalid — `validate()` above deliberately returns `Ok` for
    // it. The diagnostic lives HERE rather than inside `validate()` because `validate()` runs
    // before `paigasus_logging::init` above, so a `warn!` from there would be emitted before the
    // service logger exists and silently lost (CodeRabbit round 1).
    if !config.outbox.relay_enabled && config.outbox.wake_on_commit {
        tracing::warn!("outbox.wake_on_commit = true with outbox.relay_enabled = false — no relay is spawned, so no listener is spawned either and the setting has no effect");
    }

    // Metrics (SMA-446 Unit 3, mirrors `paigasus-gateway::main`'s identical Unit 2 wiring):
    // install the global Prometheus recorder only when `[metrics]` is enabled — `!enabled`
    // means no recorder is installed AND `/metrics` is never mounted below (a `None` handle
    // short-circuits both wiring blocks further down). `config.validate()` above already
    // rejected `metrics.addr == http_addr`.
    let metrics_handle = config.metrics.enabled.then(|| paigasus_observability::init("paigasus-iam"));
    // Register `# HELP`/`# TYPE` exposition text for every family this service emits (spec
    // §4.1) — only when a recorder was actually installed above; describing metrics nobody will
    // ever scrape is pointless, and `describe_*!` against no installed recorder is a silent
    // no-op anyway.
    if metrics_handle.is_some() {
        describe_iam_metrics();
        // SMA-495 / SMA-489 D12 priming. A metrics-rs series first appears already at its first
        // increment's VALUE, and `increase()` baselines on that first sample — so without this an
        // `increase(...) > 0` control could never fire on a replica's first notifying enqueue,
        // blinding IamOutboxNotificationsAbsent for exactly the first window after a deploy.
        //
        // Gated on the config, NOT sited in `PgOutbox::new`: that is a `Copy` value type built at
        // five composition-root sites, and priming there would put a process-global side effect in
        // a value constructor AND make the prime depend on DI ordering rather than configuration
        // (`tests/metrics.rs` builds `AppState` before installing a recorder). Gated here, the
        // series exists iff this replica is configured to nudge.
        if config.outbox.wake_on_commit {
            metrics::counter!(names::IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL).increment(0);
        }
    }

    let db = Database::connect(config.database_url.as_str()).await?;

    // SMA-571: bind BEFORE migrating, so a replica that is migrating — or, since SMA-559, waiting
    // up to `migration.lock_wait_secs` for the lock — is visibly UNREADY to its orchestrator
    // rather than absent. Bound HERE, synchronously, and not inside the spawned tasks below:
    // `servers.spawn` gives no ordering guarantee that a socket is listening before the `await`
    // that follows it, and it would defer an `EADDRINUSE` past the whole migration window,
    // surfacing the migration's error rather than the bind's.
    let (health_reporter, health_server) = grpc::health_service().await;
    // `health_service` hands the reporter back already flipped to SERVING (its M0 static
    // posture). Lower it before anything is bound: for the whole deferred window a
    // `grpc_health_probe` readiness check is the gRPC-side twin of `/readyz` 503 `migrating`,
    // and `BootSlot::install` is the ONLY thing that raises it again — see `boot.rs`.
    health_reporter.set_service_status("", tonic_health::ServingStatus::NotServing).await;
    let slot = boot::BootSlot::new(health_reporter);

    let http_listener = tokio::net::TcpListener::bind(config.http_addr).await?;
    // `TcpIncoming::bind` is synchronous and public, so the gRPC socket is listening at THIS
    // line rather than somewhere inside `serve_with_shutdown`'s own future. NOTE the `Server`
    // overload used below documents that "the `tcp_nodelay` and `tcp_keepalive` settings are
    // ignored when using this method" (tonic transport/server/mod.rs:701, covering both
    // `serve_with_incoming` and `serve_with_incoming_shutdown` — they share `serve_internal`),
    // while `Server::default()` sets `tcp_nodelay: true` (mod.rs:132). So the nodelay must be
    // re-applied HERE, on the incoming, or Nagle is silently re-enabled on every gRPC connection
    // and no test in this repo would catch it.
    let grpc_incoming = tonic::transport::server::TcpIncoming::bind(config.grpc_addr)?.with_nodelay(Some(true));
    // Separate metrics listener (SMA-446 Unit 3), only when metrics are enabled AND
    // `metrics.addr` is configured — bound here with the other two for the same reason.
    let metrics_listener = match config.metrics.addr {
        Some(addr) if metrics_handle.is_some() => Some((tokio::net::TcpListener::bind(addr).await?, addr)),
        _ => None,
    };

    let request_timeout = Duration::from_secs(30);
    let (tx, rx) = tokio::sync::watch::channel(());
    let mut servers: JoinSet<anyhow::Result<()>> = JoinSet::new();

    // Same-port `/metrics` (SMA-446 Unit 3): merged into the boot router below, only when
    // enabled AND no separate `metrics.addr` is configured — `metrics.enabled = false`, or a
    // separate `addr`, leaves this `None` (in the `addr` case `/metrics` is served on its own
    // listener, spawned separately below instead).
    let http_metrics_router = match (&metrics_handle, config.metrics.addr) {
        (Some(handle), None) => Some(paigasus_observability::metrics_router(handle.clone())),
        _ => None,
    };
    {
        let mut rx = rx.clone();
        // The boot router is the SINGLE, PERMANENT service on this listener: it owns
        // `/healthz`, `/readyz` and `/metrics` for the life of the process and falls through to
        // the slot for everything else. It is never rebuilt or replaced — `BootSlot::install`
        // fills the slot the router already points at.
        let app = boot::boot_http_router(slot.clone(), http_metrics_router);
        servers.spawn(async move {
            serve_http(http_listener, app, async move {
                let _ = rx.changed().await;
            })
            .await
            .map_err(anyhow::Error::from)
        });
    }
    if let Some((listener, metrics_addr)) = metrics_listener {
        // Keeps `/metrics` off the port that also serves the tenancy/authn/authz HTTP API. On
        // the SAME shutdown-watch every other task in this `JoinSet` uses, so it stops
        // gracefully alongside the HTTP/gRPC servers rather than lingering past them.
        let mut rx = rx.clone();
        let metrics_app = paigasus_observability::metrics_router(metrics_handle.clone().expect("guarded by the bind above"));
        servers.spawn(async move {
            tracing::info!(%metrics_addr, "paigasus-iam metrics listener started");
            axum::serve(listener, metrics_app)
                .with_graceful_shutdown(async move {
                    let _ = rx.changed().await;
                })
                .await
                .map_err(anyhow::Error::from)
        });
    }
    {
        // Periodic Prometheus upkeep (CodeRabbit round-1 fix, mirrors
        // `paigasus-gateway::main`'s identical fix): `PrometheusBuilder::install_recorder()`
        // (unlike `install()`) does NOT spawn the maintenance task
        // `PrometheusHandle::run_upkeep()` needs to periodically drain/decay histograms —
        // without calling it ourselves, memory grows unbounded over the life of the process.
        // `paigasus_observability::init()` itself stays runtime-agnostic (it's also called from
        // plain `#[test]` code with no Tokio runtime), so the spawn lives here instead, into the
        // same `JoinSet` on the same shutdown-watch as every other server task, only when
        // metrics are enabled.
        //
        // Spawned HERE, alongside the binds, rather than inside `boot_deferred` (SMA-571 fix
        // round 1): `/metrics` is live from the moment the HTTP listener binds, so leaving decay
        // to start only after the migration would mean no decay at all for a window that can be
        // `migration.lock_wait_secs` long. It also keeps `PrometheusHandle` — a
        // `metrics-exporter-prometheus` type this crate does not depend on directly — out of
        // `boot_deferred`'s signature, which is the only thing that made it unnameable.
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
    }
    {
        let mut rx = rx.clone();
        // As with HTTP: the object `boot_grpc_routes` returns is the single, permanent service
        // bound to this listener — health as a matched route plus an `UNAVAILABLE` catch-all
        // that delegates to the real, `AuthLayer`-wrapped routes once the slot is filled.
        // `AuthLayer` is deliberately NOT on this `Server`'s layer stack (unlike `grpc::router`):
        // it needs `AppState`, which does not exist yet, so it lives inside `boot::Serving`.
        let routes = boot::boot_grpc_routes(slot.clone(), health_server);
        servers.spawn(async move {
            tonic::transport::Server::builder()
                .timeout(request_timeout)
                .layer(paigasus_observability::CorrelationLayer)
                .serve_with_incoming_shutdown(routes.prepare(), grpc_incoming, async move {
                    let _ = rx.changed().await;
                })
                .await
                .map_err(anyhow::Error::from)
        });
    }
    tracing::info!(%config.http_addr, %config.grpc_addr, "paigasus-iam listeners bound; migrating");

    // SMA-571 §4.6: the whole post-bind boot is ONE fallible function so `?` can be used freely
    // inside it and the drain is structural rather than per-`?`. Adding a fallible step there can
    // no longer skip the graceful shutdown.
    //
    // ONE signal registration, kept alive across BOTH phases (SMA-571 fix round 1). Building a
    // fresh `shutdown_signal()` for the steady-state `select!` below would LOSE a SIGTERM that
    // landed in the gap between the two: tokio 1.53.1's signal driver `broadcast()` does
    // `pending.swap(false)` unconditionally and ignores the send error when there is no `Signal`
    // receiver, so a signal delivered while zero receivers are alive is consumed and never
    // redelivered to one created afterwards. The pod would then ignore its only SIGTERM and hang
    // until `terminationGracePeriodSeconds` expired into SIGKILL — precisely the stranded-lock
    // shape the arm below warns about.
    let mut shutdown = std::pin::pin!(shutdown_signal());
    let mut shutting_down = false;
    let outcome = tokio::select! {
        r = boot_deferred(&db, &config, &slot, &mut servers, &rx, request_timeout) => r,
        () = &mut shutdown => {
            // This window was unhandled before SMA-571 — but the pod is now PRESENT-and-unready
            // rather than absent, so a rolling update is far more likely to land here. Ignoring
            // SIGTERM for `lock_wait_secs` and then taking SIGKILL is the stranded-lock scenario
            // in RUNBOOK-containers.md. Cancelling `migrate_under_lock` between polls is safe,
            // and cancelling inside `Migrator::up` rolls the transaction back and releases the
            // transaction-scoped lock by construction.
            tracing::info!("shutdown signal received during boot");
            shutting_down = true;
            Ok(())
        }
    };
    if outcome.is_err() || shutting_down {
        if let Err(e) = &outcome {
            tracing::error!(error = %e, "boot failed after the listeners were bound; draining");
        }
        let _ = tx.send(());
        let outstanding = drain_bounded(&mut servers, DRAIN_TIMEOUT).await;
        if outstanding > 0 {
            tracing::warn!(unreaped = outstanding, "drain timed out with tasks not yet joined");
        }
        return outcome;
    }

    tracing::info!(%config.http_addr, %config.grpc_addr, "paigasus-iam started");

    // Stop on the first of: shutdown signal, or a server task ending.
    let early_error: Option<anyhow::Error> = tokio::select! {
        () = &mut shutdown => {
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
    result
}

/// How long the boot-failure drain waits before giving up and returning anyway. Bounded because
/// an unbounded drain turns a boot failure into a process that serves 503 forever (SMA-571 §4.6).
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Everything between the bind and "ready": the migration, `AppState::new`, the publisher dial,
/// and every background task. Returns `Err` rather than `?`-ing out of `serve()` so its single
/// caller can drain the already-bound listeners gracefully (SMA-571 AC 3).
///
/// Lives in `main.rs` deliberately: `migration_lock.rs`'s `the_composition_root_still_migrates_
/// under_the_lock` guard reads THIS file for `migrate_under_lock(` and `config.migration.lock_wait()`.
///
/// Panics are NOT drained — a panic unwinds through `#[tokio::main]` and aborts in-flight
/// requests. `catch_unwind` across this body would need `AssertUnwindSafe` and buys little: the
/// route-registration panic class is already covered by
/// `protected_router_merge_has_no_path_conflicts_in_any_capability_combination`.
async fn boot_deferred(
    db: &sea_orm::DatabaseConnection,
    config: &IamConfig,
    slot: &boot::BootSlot,
    servers: &mut JoinSet<anyhow::Result<()>>,
    rx: &tokio::sync::watch::Receiver<()>,
    request_timeout: Duration,
) -> anyhow::Result<()> {
    // SMA-559: serialised against a concurrently starting replica by a transaction-scoped
    // advisory lock. A waiter blocks here — but since SMA-571 it does so with all three
    // listeners already bound, answering `/readyz` 503 `migrating` and gRPC `UNAVAILABLE`
    // throughout, so the wait is visible to the orchestrator rather than an absent process.
    let migration = migrate_under_lock(db, config.migration.lock_wait()).await?;
    tracing::info!(
        waited = ?migration.waited,
        polls = migration.polls,
        migrations_applied = migration.migrations_applied,
        "database migrations complete"
    );
    // Built once and cloned into each server task below (a cheap handle-clone: every
    // per-aggregate service just wraps the same underlying connection pool, and the wired
    // authenticator's JWKS cache/single-flight state is `Arc`-shared) — HTTP and gRPC
    // share one `AppState` (Task 16, SMA-442; authn wiring SMA-443). Fails fast when the
    // Redis JWKS cache is configured but unreachable; JWKS themselves are fetched lazily
    // on first use, so startup stays independent of IdP availability (spec §6.4).
    //
    // `db` itself (the borrowed handle, not this clone) is also cloned below to build the
    // outbox relay (SMA-446 Slice B, Task B9) directly off the same connection pool — the
    // relay is wired straight in `main.rs` rather than through `AppState`, so `AppState::new`'s
    // signature stays unchanged.
    let state = AppState::new(db.clone(), config).await?;

    // Kept for the partition-maintenance task (SMA-467), spawned below.
    let db_for_maintenance = db.clone();

    // Kept for the outbox retention sweep (SMA-469), spawned below — for the same reason
    // `db_for_maintenance` is: `db` is borrowed here, so every consumer takes its own handle.
    let db_for_outbox_retention = db.clone();

    // SMA-471: the outbox relay's delivery sink, selected and — for the `nats` backend —
    // actually DIALLED here, inside the outbox block where it naturally belongs.
    //
    // It used to be hoisted above the first `servers.spawn` instead, because back then that
    // spawn was what bound a port: an early `?` from this dial would have returned past three
    // live listeners, skipping the graceful-shutdown `tx.send(())` and aborting their in-flight
    // requests rather than never having accepted one. That reason is gone. SMA-571 binds before
    // any of this runs, so a live listener at this point is the DESIGN, not an accident — and
    // `boot_deferred`'s caller now drains them bounded on any `Err` from this function, which is
    // strictly better than the hoist ever was: it covers every fallible step here, not just this
    // one. The hoist is therefore no longer needed, and keeping it would put the NATS dial in
    // front of a `relay_enabled = false` deployment that never uses it.
    //
    // `config.validate()` (called in `serve()`, before the bind) already rejects `relay_enabled =
    // false` together with `backend = "nats"`, so the `Tracing` arm below is reachable both when
    // the relay is enabled AND when it's disabled — never a live NATS backend with no relay to
    // drain into it.
    let publisher: Arc<dyn EventPublisher> = match config.outbox.publisher.backend {
        PublisherBackend::Nats => {
            let nats = Arc::new(NatsEventPublisher::connect(&config.outbox.publisher).await?);
            // The `iam_nats_connected` gauge sampler (SMA-471 review fix): folded into the same
            // shutdown-watched `servers` `JoinSet` as every other background task here, rather
            // than left as a detached `tokio::spawn` — see `NatsEventPublisher::
            // spawn_connection_gauge_sampler`'s doc for why that used to be a bug. Cloning the
            // `Arc` (rather than moving `nats` itself) is what lets the same publisher also be
            // handed to the relay block below.
            let sampler = nats.clone();
            let mut gauge_rx = rx.clone();
            let handle = sampler.spawn_connection_gauge_sampler(async move {
                let _ = gauge_rx.changed().await;
            });
            servers.spawn(async move { handle.await.map_err(anyhow::Error::from) });
            nats
        }
        PublisherBackend::Tracing => Arc::new(TracingEventPublisher),
    };

    {
        // The policy-snapshot background reload (SMA-444 Task 15, spec §7/D11 AC3): bounds
        // staleness even when `policy_gen` never visibly advances on this replica.
        // `CedarAuthorizer::is_authorized` (`AppState::authz`) additionally reloads
        // synchronously before every decision (AC1), so this loop only backstops
        // cross-replica/background staleness. `spawn_reload` already `tokio::spawn`s its own
        // task and hands back a `JoinHandle<()>`; wrapping that await in a `servers.spawn`
        // makes it exit on the same shutdown-watch signal as the HTTP/gRPC server tasks,
        // and surfaces a reload-task panic the same way a server-task panic would.
        //
        // `ttl`/`poll` are config-driven (`authz.policy_cache_ttl_secs`/
        // `authz.refresh_interval_secs`, SMA-444 Task 21) rather than the old hardcoded
        // `AUTHZ_POLICY_SNAPSHOT_TTL`/`AUTHZ_POLICY_RELOAD_POLL_INTERVAL` constants —
        // `IamConfig::validate` has already guaranteed both are non-zero.
        let mut rx = rx.clone();
        let ttl = Duration::from_secs(config.authz.policy_cache_ttl_secs);
        let poll = Duration::from_secs(config.authz.refresh_interval_secs);
        let handle = state.snapshot().spawn_reload(ttl, poll, async move {
            let _ = rx.changed().await;
        });
        servers.spawn(async move { handle.await.map_err(anyhow::Error::from) });
    }
    {
        // The persistent denial-audit drain (SMA-446 Slice A): `AppState::new` built the
        // bounded buffer + drain pair and wired the buffer's `AuditSink` into `CedarAuthorizer`;
        // here we spawn the drain that persists buffered denials to Postgres out of band, on the
        // same shutdown-watch as the HTTP/gRPC server tasks (mirroring the reload block above).
        // `take_denial_drain` hands the drain out exactly once — this is the sole caller — so a
        // double-spawn is impossible; a `None` would mean it was already taken (a wiring
        // defect), logged rather than silently ignored. `drain.run` returns `()`, so the spawned
        // task maps to `Ok(())` to match the `JoinSet<anyhow::Result<()>>` the servers share.
        let mut rx = rx.clone();
        match state.take_denial_drain() {
            Some(drain) => {
                let sink = state.audit_sink();
                servers.spawn(async move {
                    drain
                        .run(sink, async move {
                            let _ = rx.changed().await;
                        })
                        .await;
                    Ok(())
                });
            }
            None => tracing::error!("denial-audit drain was already taken before startup could spawn it — buffered denials will NOT be persisted"),
        }
    }
    {
        // The outbox relay (SMA-446 Slice B, Task B9): drains `event_outbox` rows — written by
        // `PgOutbox::enqueue` inside each triggering mutation's own transaction (Task B2) — into
        // calls on the `publisher` selected and constructed above, on the same shutdown-watch as
        // every other task here. Built directly off the `db` handle borrowed by this function
        // (not through `AppState`) so `AppState::new`'s signature stays unchanged. `publisher` is
        // either the real NATS JetStream sink (SMA-471, ADR-0016) or `TracingEventPublisher`
        // (Task B8, a logging-only sink for deployments with no broker configured) — never
        // constructed here; this block only consumes whichever one boot already selected.
        //
        // `max_attempts` is `u32` in config (a natural "count" type for an operator to read/
        // write) but `i32` in `OutboxRelay::new` (mirroring the `event_outbox.attempts` Postgres
        // column) — `try_from` + clamp to `i32::MAX` rather than a wrapping `as` cast, since a
        // wrapped negative `max_attempts` would park every row on its very first failed publish
        // (`attempts >= max_attempts` true immediately), the opposite of the configured intent.
        if config.outbox.relay_enabled {
            // SMA-489: the relay and the listener share one `Arc<Notify>` — the listener pokes
            // it on every `iam_outbox_event` notification, `run` races it against the poll
            // sleep. Created here (not in `AppState`) because both consumers live in this block.
            let wake = std::sync::Arc::new(tokio::sync::Notify::new());
            // Named (not shadowing `rx`) because the `wake_on_commit` block below needs its own
            // `rx.clone()` off the un-moved outer binding, mirroring the `gauge_rx` naming the
            // NATS connection-gauge sampler above uses for the same reason.
            let mut relay_rx = rx.clone();
            let relay = OutboxRelay::new(
                db.clone(),
                Duration::from_secs(config.outbox.poll_interval_secs),
                config.outbox.batch_size,
                i32::try_from(config.outbox.max_attempts).unwrap_or(i32::MAX),
            )
            .with_wake_debounce(Duration::from_millis(config.outbox.wake_debounce_ms));
            let relay_wake = wake.clone();
            servers.spawn(async move {
                relay
                    .run(publisher, relay_wake, async move {
                        let _ = relay_rx.changed().await;
                    })
                    .await;
                Ok(())
            });

            if config.outbox.wake_on_commit {
                // The listener gets its own connection string: `LISTEN` needs a direct or
                // session-mode connection, so a deployment behind a transaction-mode pooler can
                // point it elsewhere without moving the main pool (SMA-489 §1.5).
                let listen_url = config.outbox.listen_database_url.as_ref().unwrap_or(&config.database_url).as_str().to_string();
                // Watchdog is observability-only (D15): warn on silence, never reconnect on it.
                // `saturating_mul`: `validate` puts no upper bound on `poll_interval_secs`, so a
                // plain `* 3` is an unchecked `u64` multiply that panics in a debug build on an
                // absurd (but accepted) config. Saturating just pins the watchdog at "never".
                let watchdog = std::cmp::max(Duration::from_secs(60), Duration::from_secs(config.outbox.poll_interval_secs.saturating_mul(3)));
                let listener = PgOutboxListener::new(listen_url, wake, watchdog);
                let mut rx = rx.clone();
                servers.spawn(async move {
                    listener
                        .run(async move {
                            let _ = rx.changed().await;
                        })
                        .await;
                    Ok(())
                });
            } else {
                tracing::info!("outbox.wake_on_commit = false — no commit notification is emitted and no listener runs; delivery is gated by outbox.poll_interval_secs");
            }
        } else {
            tracing::warn!("outbox relay disabled — event_outbox rows will accrue undrained");
        }
    }
    {
        // Audit-log partition maintenance (SMA-467): create month partitions ahead and drop
        // aged-out denied (and, if configured, committed) leaves — mirrors the outbox relay's
        // spawn + shutdown-watch. Gated by `[audit.retention].enabled`; a startup run creates the
        // current + ahead months before the loop (non-fatal on error — the migration + the
        // DEFAULT partitions already backstop writes).
        if config.audit.retention.enabled {
            let policy = RetentionPolicy {
                ahead_months: config.audit.retention.ahead_months,
                denied_months: config.audit.retention.denied_months,
                committed_months: config.audit.retention.committed_months,
            };
            if config.audit.retention.committed_months > 0 {
                tracing::warn!(
                    committed_months = config.audit.retention.committed_months,
                    "audit.retention.committed_months > 0 — committed (compliance) audit partitions will be auto-dropped at this age"
                );
            }
            let maintainer = PgPartitionMaintainer::new(db_for_maintenance);
            let startup = maintainer.clone();
            let startup_policy = policy;
            // Awaited startup run (non-fatal).
            let report = startup.tick(chrono::Utc::now(), startup_policy).await;
            if report.errored {
                tracing::warn!("initial audit partition maintenance tick reported an error — continuing (DEFAULT partitions backstop writes)");
            }
            let interval = std::time::Duration::from_secs(config.audit.retention.interval_secs);
            let mut rx = rx.clone();
            servers.spawn(async move {
                maintainer
                    .run(policy, interval, async move {
                        let _ = rx.changed().await;
                    })
                    .await;
                Ok(())
            });
        } else {
            tracing::warn!(
                "audit.retention.enabled = false — no partition create-ahead or pruning will run; the DEFAULT partitions will fill over time and can block create-ahead until manually reattached (see RUNBOOK)"
            );
        }
        if !config.audit.query_enabled {
            tracing::warn!(
                "audit.query_enabled = false: entries are still written but GET /v1/audit and the \
                 AuditService gRPC are not served, so nothing can read them in-product"
            );
        }
    }
    {
        // Outbox retention (SMA-469): bounded, batched deletes of aged published rows and —
        // only when explicitly opted in — aged parked ones, plus the dead-letter backlog gauge.
        // Mirrors the audit partition-maintenance block above, with one deliberate difference:
        // this task is spawned UNCONDITIONALLY. `[outbox.retention].enabled = false` disables
        // the DELETES (it rides along in the policy) but the tick still runs, because the tick
        // is what refreshes `iam_outbox_parked_rows`. Gating the spawn on `enabled` would mean
        // an operator who pauses deletion during an incident — a plausible reaction — silently
        // loses the dead-letter backlog signal while the relay keeps parking rows.
        let policy = OutboxRetentionPolicy {
            enabled: config.outbox.retention.enabled,
            published_days: config.outbox.retention.published_days,
            parked_days: config.outbox.retention.parked_days,
            batch_size: config.outbox.retention.batch_size,
            max_batches_per_tick: config.outbox.retention.max_batches_per_tick,
        };
        if !config.outbox.retention.enabled {
            tracing::warn!("outbox.retention.enabled = false — event_outbox rows will never be deleted and the table will grow without bound; the dead-letter backlog gauge still updates");
        }
        if config.outbox.retention.parked_days > 0 {
            tracing::warn!(
                parked_days = config.outbox.retention.parked_days,
                "outbox.retention.parked_days > 0 — parked (dead-letter) rows will be auto-deleted at this age, whether or not an operator has inspected them, and unlike a discard through the dead-letter HTTP API this deletion writes no audit entry at all (only a counter increment) — the event's payload, actor, and correlation id are gone"
            );
        }
        let maintainer = PgOutboxMaintainer::new(db_for_outbox_retention);
        // An awaited startup sweep (non-fatal), mirroring the partition maintainer's: without
        // it nothing happens for the first `interval_secs`, which on a deployment being rescued
        // from an unbounded table is the wrong first impression.
        let report = maintainer.clone().tick(chrono::Utc::now(), policy).await;
        if report.errored {
            tracing::warn!("initial outbox retention tick reported an error — continuing");
        }
        let interval = Duration::from_secs(config.outbox.retention.interval_secs);
        let mut rx = rx.clone();
        servers.spawn(async move {
            maintainer
                .run(policy, interval, async move {
                    let _ = rx.changed().await;
                })
                .await;
            Ok(())
        });
    }

    slot.install(boot::Serving::new(state, request_timeout).await).await?;
    tracing::info!("boot slot installed; serving");
    Ok(())
}

/// Drain `servers`, bounded. Returns how many tasks were still UNREAPED at the timeout so the
/// caller can log it — a silent give-up would hide exactly the wedged task worth naming.
///
/// Unreaped, not "still running": `JoinSet::len` counts tasks this function has not yet pulled
/// out with `join_next`, so a task that finished microseconds before the budget expired is
/// counted too. The number is an upper bound on what is genuinely wedged, which is the right
/// direction for a diagnostic — it can over-report, never under-report.
async fn drain_bounded(servers: &mut JoinSet<anyhow::Result<()>>, budget: Duration) -> usize {
    let _ = tokio::time::timeout(budget, async {
        while let Some(joined) = servers.join_next().await {
            match joined {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!(error = %e, "a server task failed during drain"),
                Err(join_err) => tracing::warn!(error = %join_err, "a server task panicked during drain"),
            }
        }
    })
    .await;
    servers.len()
}

/// Registers `# HELP`/`# TYPE` exposition text for the 38 metric families `paigasus-iam` emits
/// directly (spec §4.1; includes the SMA-467 audit partition-maintenance families, the
/// SMA-469 outbox retention/dead-letter families, the SMA-476 Redis circuit-breaker families,
/// the SMA-481 system-row-retirement family, the SMA-471 NATS publisher families, and the
/// SMA-489 commit-nudge/listener families and the SMA-495 notifying-enqueue family), via the
/// `names::` consts so the string used here can't drift from the one used at the increment/set
/// call site, plus the 2 gRPC families via `paigasus_observability::describe_grpc()`. Mirrors
/// the meanings documented in `docs/ops/RUNBOOK-observability.md` §2.1/§2.2.
fn describe_iam_metrics() {
    use metrics::{describe_counter, describe_gauge, describe_histogram};

    describe_counter!(
        names::IAM_HTTP_REQUESTS_TOTAL,
        "HTTP requests handled by IAM's HTTP router, labeled by route, method, and status_class."
    );
    describe_histogram!(names::IAM_HTTP_REQUEST_DURATION_SECONDS, "IAM HTTP request latency in seconds (full request-response cycle).");
    describe_gauge!(names::IAM_HTTP_INFLIGHT_REQUESTS, "Requests currently being handled on IAM's HTTP router.");

    paigasus_observability::describe_grpc();

    describe_counter!(
        names::IAM_AUTHZ_DECISIONS_TOTAL,
        "Every CedarAuthorizer::is_authorized outcome, labeled by decision (allow/deny) and cache (hit/miss/bypass)."
    );
    describe_gauge!(
        names::IAM_REDIS_BREAKER_STATE,
        "Redis circuit-breaker state per connection: 0=closed, 1=half_open, 2=open. Label role=authz|api_keys|jwks. Set independently by every replica — aggregate max by (job, role), never sum. role=\"api_keys\" requires api_keys.introspect_cache.backend=\"redis\" AND that cache holding its own connection: either authz.cache.backend=\"memory\", or both Redis-backed with redis_urls that differ after trimming (SMA-485). Otherwise those commands are attributed to role=\"authz\"."
    );
    describe_counter!(
        names::IAM_REDIS_BREAKER_TRANSITIONS_TOTAL,
        "Redis circuit-breaker state transitions, labeled by role and to=closed|half_open|open. Catches flapping the gauge cannot see: the open window is 2s while scrapes are 15-30s apart."
    );
    describe_counter!(
        names::IAM_AUTHZ_POLICY_SNAPSHOT_RELOADS_TOTAL,
        "Every PolicySnapshot reload attempt, labeled by outcome (installed/rejected/failed). 'installed' must stay non-zero — the TTL backstop installs one every authz.policy_cache_ttl_secs regardless of whether the generation counter moved, so silence means revocations are not taking effect (SMA-470)."
    );
    describe_counter!(
        names::IAM_AUTHZ_GENERATION_REWINDS_TOTAL,
        "Rewinds of a Redis authz generation counter, labeled by counter (policy_gen/entity_gen), outcome (repaired/repair_failed/ceiling) and reason (missing/lower)."
    );
    describe_counter!(
        names::IAM_AUDIT_RECORDS_TOTAL,
        "Every audit-log insert attempt (mutation or denial), labeled by outcome (committed/denied) and result."
    );
    describe_counter!(
        names::IAM_DENIAL_AUDITS_DROPPED_TOTAL,
        "Denial-audit rows dropped because the bounded in-memory buffer was full — non-zero means gaps in the denial audit trail."
    );
    describe_counter!(
        names::IAM_DENIAL_AUDITS_ENQUEUED_TOTAL,
        "Denial-audit rows enqueued onto the bounded in-memory buffer, whether or not the enqueue also dropped an older row."
    );
    describe_counter!(
        names::IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL,
        "Bootstrap-admin seed attempts that failed and were swallowed, by stage (list = the pre-seed existence check, txn = the grant+audit+event transaction). A lost policy_gen bump is not counted."
    );
    describe_counter!(
        names::IAM_STARTER_POLICY_RECONCILES_TOTAL,
        "Boot-time starter-policy reconciliation, labeled by outcome (unchanged/seeded/reconciled/adopted/stale_binary/externally_modified/orphaned/failed). System-role reconciliation shares this counter for the 'failed' label only."
    );
    describe_counter!(
        names::IAM_OUTBOX_RELAY_TICKS_TOTAL,
        "Outbox relay poll-loop iterations, labeled by result (ok/error) — the relay's liveness signal."
    );
    describe_counter!(
        names::IAM_OUTBOX_RELAY_DRAINED_TOTAL,
        "Outbox rows locked and processed (published + failed, including newly-parked) in a relay tick."
    );
    describe_counter!(names::IAM_OUTBOX_RELAY_PUBLISHED_TOTAL, "Outbox rows successfully published in a relay tick.");
    describe_counter!(names::IAM_OUTBOX_RELAY_PUBLISH_FAILURES_TOTAL, "Outbox rows whose EventPublisher::publish call failed in a relay tick.");
    describe_counter!(
        names::IAM_OUTBOX_RELAY_PARKED_TOTAL,
        "Outbox rows newly parked (poison — exceeded [outbox].max_attempts) in a relay tick."
    );
    describe_gauge!(
        names::IAM_OUTBOX_OLDEST_UNPUBLISHED_AGE_SECONDS,
        "Age in seconds of the oldest unpublished-and-unparked outbox row seen in the most recent poll tick's batch."
    );

    describe_counter!(
        names::IAM_OUTBOX_RETENTION_TICKS_TOTAL,
        "Outbox retention sweep ticks, labeled by result (ok/error) — the sweep's liveness signal. Ticks even when [outbox.retention].enabled = false, because the tick also refreshes the dead-letter backlog gauge."
    );
    describe_counter!(names::IAM_OUTBOX_ROWS_DELETED_TOTAL, "event_outbox rows deleted by retention; label reason=published|parked.");
    describe_gauge!(
        names::IAM_OUTBOX_PARKED_ROWS,
        "Parked (dead-letter) event_outbox rows awaiting an operator. Set independently by every replica — aggregate max by (job), never sum."
    );
    describe_counter!(
        names::IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL,
        "Dead-letter ROWS returned to the live queue; label scope=one|bulk. Counts rows, not calls, so rate() is meaningful across both scopes."
    );
    describe_counter!(
        names::IAM_OUTBOX_DEAD_LETTERS_DISCARDED_TOTAL,
        "Dead letters permanently discarded by an operator — each one is an event that committed in IAM and will never reach any consumer."
    );

    describe_counter!(
        names::IAM_OUTBOX_RELAY_WAKEUPS_TOTAL,
        "Relay ticks by what woke them: notify (a Postgres LISTEN notification), poll (the poll_interval_secs timer) or backlog (the continuation after a full batch that made progress). One increment per TICK, so sum without (source) equals sum without (result) of iam_outbox_relay_ticks_total."
    );
    describe_histogram!(
        names::IAM_OUTBOX_PUBLISH_LAG_SECONDS,
        "End-to-end outbox latency: now - occurred_at when a row is successfully published. The only signal that proves the commit-nudge is working; iam_outbox_oldest_unpublished_age_seconds cannot, as it is refreshed only by poll ticks and reflects that tick's batch."
    );
    describe_counter!(
        names::IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL,
        "Notifications the outbox listener received. Flat at zero while rows are being drained means LISTEN is not reaching this replica — most likely a transaction-mode connection pooler, which silently does not support it."
    );
    describe_gauge!(
        names::IAM_OUTBOX_LISTENER_CONNECTED,
        "1 when the outbox listener holds a live LISTEN connection, 0 otherwise. Per-replica and the replicas do NOT agree — aggregate with min by (job), never sum or max."
    );
    describe_counter!(
        names::IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL,
        "Outbox-listener reconnects. Climbing means Postgres is churning the listener connection; notifications during each gap are dropped and picked up by the poll."
    );
    describe_counter!(
        names::IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL,
        "Enqueues that emitted a pg_notify — the write-side twin of iam_outbox_listener_notifications_total and the control IamOutboxNotificationsAbsent gates on. NOT 1:1 with it: Postgres collapses identical channel+payload notifications within a transaction, so N enqueues in one transaction give N increments but ONE notification. Counted pre-commit, so a rolled-back mutation increments it without delivering anything. A dead-letter replay increments it not at all."
    );

    describe_counter!(
        names::IAM_AUDIT_PARTITION_MAINTENANCE_TICKS_TOTAL,
        "Audit partition-maintenance ticks (create-ahead + prune); label result=ok|error."
    );
    describe_counter!(names::IAM_AUDIT_PARTITIONS_CREATED_TOTAL, "Audit monthly leaf partitions created by create-ahead.");
    describe_counter!(
        names::IAM_AUDIT_PARTITIONS_DROPPED_TOTAL,
        "Audit monthly leaf partitions dropped by retention; label outcome=denied|committed."
    );
    describe_gauge!(
        names::IAM_AUDIT_DEFAULT_PARTITION_ROWS,
        "Rows currently in the audit DEFAULT partitions — should be 0; nonzero means create-ahead fell behind."
    );

    describe_counter!(
        names::IAM_SYSTEM_ROWS_RETIRED_TOTAL,
        "Retirements of orphaned system-owned policy/role rows, by outcome (retired/blocked/refused)."
    );

    describe_counter!(
        names::IAM_NATS_PUBLISH_DUPLICATES_TOTAL,
        "JetStream acks returned as duplicates — a relay redelivery collapsed by Nats-Msg-Id dedup."
    );
    describe_histogram!(
        names::IAM_NATS_PUBLISH_DURATION_SECONDS,
        "JetStream publish ack round-trip latency, inside the relay's lock-holding transaction."
    );
    describe_gauge!(
        names::IAM_NATS_CONNECTED,
        "1 when the NATS client reports a live connection, 0 otherwise. Per-replica: aggregate max by (job)."
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

#[cfg(test)]
mod tests {
    use super::*;

    /// SMA-571 §4.6: the boot-failure drain MUST be bounded. `main.rs`'s shutdown-path drain has
    /// no timeout; reused unchanged for a boot failure, a task that never observes the watch would
    /// hang the process with three listening sockets serving 503 FOREVER — CrashLoopBackOff never
    /// happens and the replica is indistinguishable from a slow migration, which is exactly the
    /// state D4 rejects.
    #[tokio::test]
    async fn drain_bounded_returns_at_the_timeout_when_a_task_ignores_the_watch() {
        let (tx, rx) = tokio::sync::watch::channel(());
        let mut servers: JoinSet<anyhow::Result<()>> = JoinSet::new();
        let mut good = rx.clone();
        servers.spawn(async move {
            let _ = good.changed().await;
            Ok(())
        });
        servers.spawn(async move {
            std::future::pending::<()>().await;
            Ok(())
        });
        let _ = tx.send(());
        let started = tokio::time::Instant::now();
        let outstanding = drain_bounded(&mut servers, Duration::from_millis(200)).await;
        assert!(started.elapsed() < Duration::from_secs(2), "must return at the timeout, not hang");
        assert_eq!(outstanding, 1, "the task that ignored the watch is reported, not silently dropped");
    }

    /// The cooperative case: every task observes the watch, so the drain joins them all well
    /// inside its budget and reports nothing outstanding.
    #[tokio::test]
    async fn drain_bounded_joins_cooperative_tasks_and_reports_none_outstanding() {
        let (tx, rx) = tokio::sync::watch::channel(());
        let mut servers: JoinSet<anyhow::Result<()>> = JoinSet::new();
        for _ in 0..3 {
            let mut r = rx.clone();
            servers.spawn(async move {
                let _ = r.changed().await;
                Ok(())
            });
        }
        let _ = tx.send(());
        assert_eq!(drain_bounded(&mut servers, Duration::from_secs(5)).await, 0);
    }
}
