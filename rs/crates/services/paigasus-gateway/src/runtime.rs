// SPDX-License-Identifier: Apache-2.0

//! Server-task supervision for the `paigasus-gateway` composition root.
//!
//! [`supervise`] runs a set of long-lived server tasks on a shared graceful-shutdown
//! [`watch`] channel: it waits for the first of a shutdown signal or any task ending,
//! then broadcasts graceful shutdown to the rest and drains them, surfacing the first
//! error. This turns a metrics-listener (or upkeep) failure — previously a detached
//! task that only logged before dying — into an error that propagates out of `main`
//! (SMA-463). Mirrors `paigasus-iam`'s composition-root supervision so both services
//! share one model.

use std::future::Future;

use tokio::sync::watch;
use tokio::task::JoinSet;

/// Supervise a set of server tasks on a shared graceful-shutdown watch.
///
/// Waits for the first of: `shutdown` resolving (an OS signal), or any task in
/// `servers` ending (cleanly, with an error, or by panic). Then broadcasts graceful
/// shutdown via `tx` and drains the remaining tasks, returning the first error
/// observed. A clean early task return is logged (warn) but is not an error.
///
/// # Invariants
/// - Every task in `servers` must observe shutdown through a [`watch::Receiver`] cloned
///   from the same channel as `tx` **before** this function is called, so `tx.send`
///   reaches it. Receivers cloned before the first send do not wake spuriously, so the
///   first `changed().await` correctly waits.
/// - Callers must pass either a non-empty `servers` or a `shutdown` that resolves; an
///   empty set with a non-resolving `shutdown` would disable both `select!` arms and
///   wait forever. The gateway always spawns its main HTTP task, so it never hits this.
pub async fn supervise(mut servers: JoinSet<anyhow::Result<()>>, shutdown: impl Future<Output = ()>, tx: watch::Sender<()>) -> anyhow::Result<()> {
    // Stop on the first of: shutdown signal, or a server task ending.
    let early_error: Option<anyhow::Error> = tokio::select! {
        () = shutdown => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::{pending, ready};

    /// Spawn a task that returns `Ok(())` only once the shutdown broadcast fires.
    fn spawn_until_shutdown(servers: &mut JoinSet<anyhow::Result<()>>, mut rx: watch::Receiver<()>) {
        servers.spawn(async move {
            let _ = rx.changed().await;
            Ok(())
        });
    }

    #[tokio::test]
    async fn early_error_is_surfaced_and_triggers_shutdown() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        // A peer that only ends when told to shut down — proves the broadcast reaches it.
        spawn_until_shutdown(&mut servers, rx.clone());
        // A task that fails immediately.
        servers.spawn(async { Err(anyhow::anyhow!("boom")) });

        // `shutdown` never fires; the only way out is the failing task.
        let result = supervise(servers, pending(), tx).await;

        let err = result.expect_err("a failing task must surface as Err");
        assert_eq!(err.to_string(), "boom");
    }

    #[tokio::test]
    async fn clean_shutdown_drains_all_to_ok() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        spawn_until_shutdown(&mut servers, rx.clone());

        // `shutdown` is ready immediately → supervise broadcasts, both tasks drain Ok.
        let result = supervise(servers, ready(()), tx).await;

        assert!(result.is_ok(), "clean shutdown must drain all tasks to Ok, got {result:?}");
    }

    #[tokio::test]
    async fn early_clean_return_warns_not_errors() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        // A task that returns Ok before any shutdown — the warn branch, not an error.
        servers.spawn(async { Ok(()) });

        let result = supervise(servers, pending(), tx).await;

        assert!(result.is_ok(), "a clean early return is not an error, got {result:?}");
    }

    #[tokio::test]
    async fn error_surfaced_even_when_shutdown_wins_the_select() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        // A task that fails immediately, while shutdown is ALSO ready.
        servers.spawn(async { Err(anyhow::anyhow!("late boom")) });

        // Whichever `select!` arm wins, the drain must still surface the error.
        let result = supervise(servers, ready(()), tx).await;

        let err = result.expect_err("the error must survive even when shutdown wins the select");
        assert_eq!(err.to_string(), "late boom");
    }
}
