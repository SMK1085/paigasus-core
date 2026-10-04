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
use std::time::Duration;

use metrics::counter;
use paigasus_observability::names;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_util::task::TaskTracker;

use crate::domain::limits::ChargeDropReason;

/// D16: how long shutdown waits for in-flight limit charges.
pub const CHARGE_DRAIN_BUDGET: Duration = Duration::from_secs(5);

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
/// - `charges`: the Redis limit store's spawned charges (SMA-677 D16). After the servers drain,
///   they get at most [`CHARGE_DRAIN_BUDGET`]; the rest are counted as dropped.
/// - Callers must pass either a non-empty `servers` or a `shutdown` that resolves; an
///   empty set with a non-resolving `shutdown` would disable both `select!` arms and
///   wait forever. The gateway always spawns its main HTTP task, so it never hits this.
pub async fn supervise(mut servers: JoinSet<anyhow::Result<()>>, shutdown: impl Future<Output = ()>, tx: watch::Sender<()>, charges: Option<TaskTracker>) -> anyhow::Result<()> {
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
    if let Some(charges) = charges {
        drain_charges(&charges, CHARGE_DRAIN_BUDGET).await;
    }
    result
}

/// D16: close the tracker and wait at most `budget`. Returns the number of charges still running
/// then. They are lost, and counted as `gateway_limit_charges_dropped_total{reason="shutdown"}`.
pub async fn drain_charges(charges: &TaskTracker, budget: Duration) -> usize {
    charges.close();
    if tokio::time::timeout(budget, charges.wait()).await.is_ok() {
        return 0;
    }
    let lost = charges.len();
    counter!(names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, "reason" => ChargeDropReason::Shutdown.as_label()).increment(u64::try_from(lost).unwrap_or(u64::MAX));
    tracing::warn!(lost, "shutdown: limit charges still running after the drain budget were dropped");
    lost
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
        let result = supervise(servers, pending(), tx, None).await;

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
        let result = supervise(servers, ready(()), tx, None).await;

        assert!(result.is_ok(), "clean shutdown must drain all tasks to Ok, got {result:?}");
    }

    #[tokio::test]
    async fn early_clean_return_warns_not_errors() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        // A task that returns Ok before any shutdown — the warn branch, not an error.
        servers.spawn(async { Ok(()) });

        let result = supervise(servers, pending(), tx, None).await;

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
        let result = supervise(servers, ready(()), tx, None).await;

        let err = result.expect_err("the error must survive even when shutdown wins the select");
        assert_eq!(err.to_string(), "late boom");
    }

    use crate::test_support::counter;
    use metrics_util::debugging::DebuggingRecorder;
    use paigasus_observability::names;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio_util::task::TaskTracker;

    #[test]
    fn the_drain_budget_is_five_seconds() {
        assert_eq!(CHARGE_DRAIN_BUDGET, Duration::from_secs(5));
    }

    /// D16: a charge still running after the budget is lost and counted as `reason="shutdown"`.
    #[tokio::test]
    async fn a_charge_still_running_after_the_budget_is_counted_as_dropped() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let charges = TaskTracker::new();
        charges.spawn(std::future::pending::<()>());
        assert_eq!(drain_charges(&charges, Duration::from_millis(50)).await, 1);
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, &[("reason", "shutdown")]), Some(1));
    }

    #[tokio::test]
    async fn supervise_drains_the_charges_after_the_servers() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        let charges = TaskTracker::new();
        let landed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&landed);
        charges.spawn(async move {
            tokio::task::yield_now().await;
            flag.store(true, Ordering::SeqCst);
        });
        let result = supervise(servers, ready(()), tx, Some(charges)).await;
        assert!(result.is_ok());
        assert!(landed.load(Ordering::SeqCst), "supervise waits for an in-flight charge");
    }
}
