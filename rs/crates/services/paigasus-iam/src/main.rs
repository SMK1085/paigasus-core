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
    paigasus_logging::init("paigasus-iam");

    let config = IamConfig::load()?;
    let db = Database::connect(&config.database_url).await?;
    Migrator::up(&db, None).await?;

    let request_timeout = Duration::from_secs(30);
    let (tx, rx) = tokio::sync::watch::channel(());

    let mut servers: JoinSet<anyhow::Result<()>> = JoinSet::new();
    {
        let mut rx = rx.clone();
        let state = AppState { db: db.clone() };
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
        let addr = config.grpc_addr;
        servers.spawn(async move {
            grpc::serve(addr, request_timeout, async move {
                let _ = rx.changed().await;
            })
            .await
            .map_err(anyhow::Error::from)
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
