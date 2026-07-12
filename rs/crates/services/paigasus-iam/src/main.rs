// SPDX-License-Identifier: Apache-2.0

//! paigasus-iam composition root: load config, init logging, connect + migrate the DB,
//! serve HTTP + gRPC health on two ports, and shut down gracefully on SIGINT/SIGTERM.

use std::time::Duration;

use paigasus_iam::adapters::http::{AppState, serve_http};
use paigasus_iam::adapters::{grpc, persistence::Migrator};
use paigasus_iam::config::IamConfig;
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

    let db = Database::connect(&config.database_url).await?;
    Migrator::up(&db, None).await?;
    // Built once and cloned into each server task below (a cheap handle-clone: every
    // per-aggregate service just wraps the same underlying connection pool, and the wired
    // authenticator's JWKS cache/single-flight state is `Arc`-shared) — HTTP and gRPC
    // share one `AppState` (Task 16, SMA-442; authn wiring SMA-443). Fails fast when the
    // Redis JWKS cache is configured but unreachable; JWKS themselves are fetched lazily
    // on first use, so startup stays independent of IdP availability (spec §6.4).
    let state = AppState::new(db, &config).await?;

    let request_timeout = Duration::from_secs(30);
    let (tx, rx) = tokio::sync::watch::channel(());

    let mut servers: JoinSet<anyhow::Result<()>> = JoinSet::new();
    {
        let mut rx = rx.clone();
        let state = state.clone();
        let addr = config.http_addr;
        servers.spawn(async move {
            serve_http(addr, state, request_timeout, async move {
                let _ = rx.changed().await;
            })
            .await
            .map_err(anyhow::Error::from)
        });
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
        // Overflow observability (SMA-446 Slice A): the denial buffer drops its OLDEST queued
        // entry when full (favoring recency) and bumps a monotonic counter. Emit that counter
        // periodically as a `tracing` gauge so a sustained denial burst outpacing the drain is
        // visible (a persistent-metrics backend is a later slice). Exits on the same
        // shutdown-watch as every other task.
        let mut rx = rx.clone();
        let buffer = state.denial_buffer();
        servers.spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(60));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let dropped = buffer.dropped();
                        if dropped > 0 {
                            tracing::warn!(dropped_denial_audits = dropped, "denial-audit buffer has dropped entries on overflow (drain is not keeping up)");
                        }
                    }
                    _ = rx.changed() => break,
                }
            }
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
