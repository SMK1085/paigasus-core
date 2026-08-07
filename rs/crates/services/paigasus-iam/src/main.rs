// SPDX-License-Identifier: Apache-2.0

//! paigasus-iam composition root: load config, init logging, connect + migrate the DB,
//! serve HTTP + gRPC health on two ports, and shut down gracefully on SIGINT/SIGTERM.

use std::sync::Arc;
use std::time::Duration;

use paigasus_iam::adapters::events::{OutboxRelay, TracingEventPublisher};
use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::{AppState, serve_http};
use paigasus_iam::adapters::persistence::{Migrator, OutboxRetentionPolicy, PgOutboxMaintainer, PgPartitionMaintainer, RetentionPolicy};
use paigasus_iam::config::IamConfig;
use paigasus_observability::names;
use sea_orm::Database;
use sea_orm_migration::MigratorTrait;
use tokio::task::JoinSet;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
    }

    let db = Database::connect(&config.database_url).await?;
    Migrator::up(&db, None).await?;
    // Built once and cloned into each server task below (a cheap handle-clone: every
    // per-aggregate service just wraps the same underlying connection pool, and the wired
    // authenticator's JWKS cache/single-flight state is `Arc`-shared) — HTTP and gRPC
    // share one `AppState` (Task 16, SMA-442; authn wiring SMA-443). Fails fast when the
    // Redis JWKS cache is configured but unreachable; JWKS themselves are fetched lazily
    // on first use, so startup stays independent of IdP availability (spec §6.4).
    //
    // `db` itself (the original handle, not this clone) is kept alive below to build the
    // outbox relay (SMA-446 Slice B, Task B9) directly off the same connection pool — the
    // relay is wired straight in `main.rs` rather than through `AppState`, so `AppState::new`'s
    // signature stays unchanged.
    let state = AppState::new(db.clone(), &config).await?;

    // Kept for the partition-maintenance task (SMA-467), spawned below; cloned before the outbox
    // relay block consumes the original `db` handle.
    let db_for_maintenance = db.clone();

    // Kept for the outbox retention sweep (SMA-469), spawned below — cloned here for the same
    // reason `db_for_maintenance` is: the outbox-relay block consumes the original `db` handle.
    let db_for_outbox_retention = db.clone();

    let request_timeout = Duration::from_secs(30);
    let (tx, rx) = tokio::sync::watch::channel(());

    // Same-port `/metrics` (SMA-446 Unit 3): built here, threaded into `serve_http` below, only
    // when enabled AND no separate `metrics.addr` is configured — `metrics.enabled = false`, or a
    // separate `addr`, leaves this `None` (in the `addr` case `/metrics` is served on its own
    // listener, spawned separately below instead).
    let http_metrics_router = match (&metrics_handle, config.metrics.addr) {
        (Some(handle), None) => Some(paigasus_observability::metrics_router(handle.clone())),
        _ => None,
    };

    let mut servers: JoinSet<anyhow::Result<()>> = JoinSet::new();
    {
        let mut rx = rx.clone();
        let state = state.clone();
        let addr = config.http_addr;
        servers.spawn(async move {
            serve_http(addr, state, request_timeout, http_metrics_router, async move {
                let _ = rx.changed().await;
            })
            .await
            .map_err(anyhow::Error::from)
        });
    }
    {
        // Separate metrics listener (SMA-446 Unit 3), only when both enabled AND `metrics.addr`
        // is configured — keeps `/metrics` off the port that also serves the tenancy/authn/authz
        // HTTP API. On the SAME shutdown-watch every other task in this `JoinSet` uses, so it
        // stops gracefully alongside the HTTP/gRPC servers rather than lingering past them.
        if let (Some(handle), Some(metrics_addr)) = (metrics_handle.clone(), config.metrics.addr) {
            let mut rx = rx.clone();
            let metrics_app = paigasus_observability::metrics_router(handle);
            servers.spawn(async move {
                let listener = tokio::net::TcpListener::bind(metrics_addr).await?;
                tracing::info!(%metrics_addr, "paigasus-iam metrics listener started");
                axum::serve(listener, metrics_app)
                    .with_graceful_shutdown(async move {
                        let _ = rx.changed().await;
                    })
                    .await
                    .map_err(anyhow::Error::from)
            });
        }
    }
    {
        let mut rx = rx.clone();
        let state = state.clone();
        let addr = config.grpc_addr;
        servers.spawn(async move {
            grpc::serve(addr, state, request_timeout, async move {
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
    }
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
        // calls on an injected `EventPublisher`, on the same shutdown-watch as every other task
        // above. Built directly off the `db` handle kept alive above (not through `AppState`) so
        // `AppState::new`'s signature stays unchanged; `TracingEventPublisher` (Task B8) is a
        // placeholder sink ahead of a real message-bus publisher (a later slice).
        //
        // `max_attempts` is `u32` in config (a natural "count" type for an operator to read/
        // write) but `i32` in `OutboxRelay::new` (mirroring the `event_outbox.attempts` Postgres
        // column) — `try_from` + clamp to `i32::MAX` rather than a wrapping `as` cast, since a
        // wrapped negative `max_attempts` would park every row on its very first failed publish
        // (`attempts >= max_attempts` true immediately), the opposite of the configured intent.
        if config.outbox.relay_enabled {
            let mut rx = rx.clone();
            let relay = OutboxRelay::new(
                db,
                Duration::from_secs(config.outbox.poll_interval_secs),
                config.outbox.batch_size,
                i32::try_from(config.outbox.max_attempts).unwrap_or(i32::MAX),
            );
            servers.spawn(async move {
                relay
                    .run(Arc::new(TracingEventPublisher), async move {
                        let _ = rx.changed().await;
                    })
                    .await;
                Ok(())
            });
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

    tracing::info!(%config.http_addr, %config.grpc_addr, "paigasus-iam started");

    // Stop on the first of: shutdown signal, or a server task ending.
    let early_error: Option<anyhow::Error> = tokio::select! {
        () = shutdown_signal() => {
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

/// Registers `# HELP`/`# TYPE` exposition text for the 27 metric families `paigasus-iam` emits
/// directly (spec §4.1; includes the SMA-467 audit partition-maintenance families, the
/// SMA-469 outbox retention/dead-letter families, the SMA-476 Redis circuit-breaker families,
/// and the SMA-481 system-row-retirement family), via the `names::` consts so the string used
/// here can't drift from the one used at the increment/set call site, plus the 2 gRPC families
/// via `paigasus_observability::describe_grpc()`. Mirrors the meanings documented in
/// `docs/ops/RUNBOOK-observability.md` §2.1/§2.2.
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
        "Age in seconds of the oldest unpublished-and-unparked outbox row seen in the most recent non-empty relay tick's batch."
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
