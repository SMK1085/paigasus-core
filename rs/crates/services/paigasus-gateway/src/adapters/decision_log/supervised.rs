// SPDX-License-Identifier: Apache-2.0

//! The loop that keeps a decision-log task alive (SMA-755 § 5.6, A2). The writer and the relay both run
//! inside it. One iteration that panics is caught, logged and counted, and the loop goes on. The loop
//! returns only after the shutdown signal.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures::FutureExt;
use metrics::counter;
use paigasus_observability::names;
use tokio::sync::watch;

/// The pause after a panic, so that an iteration that panics at once does not spin.
const RESTART_PAUSE: Duration = Duration::from_millis(50);

/// Whether the shutdown signal was sent. A dropped sender counts as the signal.
#[must_use]
pub fn shutdown_requested(shutdown: &watch::Receiver<()>) -> bool {
    shutdown.has_changed().unwrap_or(true)
}

/// Run `iteration` again and again until the shutdown signal. An iteration must return when the signal
/// comes, and it must use its own clone of the receiver: this function reads the signal with `has_changed`,
/// so it never consumes the signal. A panic in an iteration counts `task_restarts{task}` and the loop goes on.
pub async fn run_supervised<F, Fut>(task: &'static str, shutdown: watch::Receiver<()>, mut iteration: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    while !shutdown_requested(&shutdown) {
        if AssertUnwindSafe(iteration()).catch_unwind().await.is_err() {
            tracing::error!(task, "a decision-log task iteration panicked; the task goes on");
            counter!(names::GATEWAY_DECISION_LOG_TASK_RESTARTS_TOTAL, "task" => task).increment(1);
            let mut sd = shutdown.clone();
            tokio::select! {
                () = tokio::time::sleep(RESTART_PAUSE) => {}
                _ = sd.changed() => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::counter as read_counter;
    use metrics_util::debugging::DebuggingRecorder;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn a_panic_in_one_iteration_counts_a_restart_and_goes_on() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let (tx, rx) = watch::channel(());
        let tx = Arc::new(tx);
        let calls = Arc::new(AtomicUsize::new(0));
        let ok_runs = Arc::new(AtomicUsize::new(0));
        let (c, o) = (Arc::clone(&calls), Arc::clone(&ok_runs));
        run_supervised("writer", rx, move || {
            let (c, o, tx) = (Arc::clone(&c), Arc::clone(&o), Arc::clone(&tx));
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    panic!("the first iteration panics");
                }
                if o.fetch_add(1, Ordering::SeqCst) + 1 == 3 {
                    tx.send(()).expect("a receiver is alive");
                }
            }
        })
        .await;
        assert_eq!(read_counter(&snapshotter, names::GATEWAY_DECISION_LOG_TASK_RESTARTS_TOTAL, &[("task", "writer")]), Some(1));
        assert_eq!(ok_runs.load(Ordering::SeqCst), 3, "the iterations after the panic ran");
    }

    #[tokio::test]
    async fn a_dropped_sender_ends_the_loop() {
        let (tx, rx) = watch::channel(());
        drop(tx);
        run_supervised("relay", rx, || async { unreachable!("no iteration runs after the sender is gone") }).await;
    }
}
