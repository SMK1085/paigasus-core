// SPDX-License-Identifier: Apache-2.0

//! The writer task (SMA-755 § 5.6): it reads the queue and runs `XADD` in a pipeline of up to 64 commands.
//! It never retries. A failed event counts as a drop. At or above `max_len` entries in the stream it drops each
//! new event (the backlog cap).

use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::FutureExt;
use metrics::counter;
use paigasus_observability::names;
use tokio::sync::{Mutex, mpsc, watch};
use tokio::time::Instant;

use super::redis_conn::RedisSlot;
use super::supervised::run_supervised;
use super::{DropReason, count_dropped};

/// Events in one pipeline.
const BATCH: usize = 64;
/// How often the writer reads `XLEN` (P-19).
const SAMPLE_EVERY: Duration = Duration::from_secs(1);

/// What the writer needs to know about the stream.
#[derive(Debug, Clone)]
pub struct WriterSettings {
    /// `<prefix>:decisions`.
    pub key: String,
    /// The backlog cap. The stream holds at most this many entries from this writer's view.
    pub max_len: u64,
}

/// The writer's view of the backlog. `XLEN` is sampled once per second. Between two samples the writer adds
/// what it wrote, so that the cap holds also inside one second. The relay deletes entries, so the estimate can
/// be higher than the real length for up to one second.
#[derive(Debug, Default)]
struct Backlog {
    sampled_at: Option<Instant>,
    sampled: u64,
    written_since: u64,
}

struct State {
    rx: Mutex<mpsc::Receiver<Bytes>>,
    backlog: Mutex<Backlog>,
    slot: RedisSlot,
    settings: WriterSettings,
}

fn classify(err: &redis::RedisError) -> DropReason {
    if err.to_string().contains(paigasus_redis::BREAKER_OPEN_MESSAGE) {
        DropReason::BreakerOpen
    } else {
        DropReason::WriteError
    }
}

/// Run `fut`, the write of a batch of `n` events. A panic inside it counts the batch as `write_error`: the batch
/// already left the queue, so no other counter sees it. The panic is returned for the caller to handle.
async fn counted<F: Future<Output = ()>>(n: u64, fut: F) -> Result<(), Box<dyn Any + Send>> {
    let result = AssertUnwindSafe(fut).catch_unwind().await;
    if result.is_err() {
        count_dropped(DropReason::WriteError, n);
    }
    result
}

impl State {
    /// Write one batch, or count each event of it as dropped. A panic counts the batch as `write_error`.
    async fn write_batch(&self, batch: Vec<Bytes>) -> Result<(), Box<dyn Any + Send>> {
        let n = batch.len() as u64;
        counted(n, self.write_batch_inner(batch)).await
    }

    async fn write_batch_inner(&self, batch: Vec<Bytes>) {
        let n = batch.len() as u64;
        #[cfg(test)]
        if self.settings.key == tests::PANIC_KEY {
            panic!("a test panic inside write_batch");
        }
        let Some(handle) = self.slot.get() else {
            count_dropped(DropReason::RedisUnavailable, n);
            return;
        };
        let mut conn = handle.clone();
        let mut backlog = self.backlog.lock().await;
        if backlog.sampled_at.is_none_or(|t| t.elapsed() >= SAMPLE_EVERY) {
            match redis::cmd("XLEN").arg(&self.settings.key).query_async::<u64>(&mut conn).await {
                Ok(len) => {
                    backlog.sampled_at = Some(Instant::now());
                    backlog.sampled = len;
                    backlog.written_since = 0;
                }
                Err(err) => {
                    count_dropped(classify(&err), n);
                    return;
                }
            }
        }
        let estimate = backlog.sampled.saturating_add(backlog.written_since);
        let room = self.settings.max_len.saturating_sub(estimate);
        let take = usize::try_from(room).unwrap_or(usize::MAX).min(batch.len());
        count_dropped(DropReason::BacklogFull, (batch.len() - take) as u64);
        if take == 0 {
            return;
        }
        let guard = self.settings.max_len.saturating_mul(2);
        let mut pipe = redis::pipe();
        for event in &batch[..take] {
            pipe.cmd("XADD")
                .arg(&self.settings.key)
                .arg("MAXLEN")
                .arg("~")
                .arg(guard)
                .arg("*")
                .arg("e")
                .arg(event.as_ref())
                .ignore();
        }
        match pipe.query_async::<()>(&mut conn).await {
            Ok(()) => {
                backlog.written_since += take as u64;
                counter!(names::GATEWAY_DECISION_LOG_WRITTEN_TOTAL).increment(take as u64);
            }
            Err(err) => count_dropped(classify(&err), take as u64),
        }
    }

    async fn iteration(&self, mut shutdown: watch::Receiver<()>) {
        let mut rx = self.rx.lock().await;
        let first = tokio::select! {
            biased;
            _ = shutdown.changed() => return,
            m = rx.recv() => m,
        };
        let Some(first) = first else {
            // Every sender is gone: wait for the shutdown signal.
            let _ = shutdown.changed().await;
            return;
        };
        let mut batch = vec![first];
        while batch.len() < BATCH {
            match rx.try_recv() {
                Ok(b) => batch.push(b),
                Err(_) => break,
            }
        }
        drop(rx);
        // The batch is counted already. The panic goes on to the supervisor, which counts the restart.
        if let Err(panic) = self.write_batch(batch).await {
            std::panic::resume_unwind(panic);
        }
    }

    /// After the shutdown signal: close the queue, write what is left inside `budget`, and count the rest.
    async fn drain(&self, budget: Duration) {
        let deadline = Instant::now() + budget;
        let mut rx = self.rx.lock().await;
        rx.close();
        loop {
            let mut batch = Vec::new();
            while batch.len() < BATCH {
                match rx.try_recv() {
                    Ok(b) => batch.push(b),
                    Err(_) => break,
                }
            }
            if batch.is_empty() {
                return;
            }
            let n = batch.len() as u64;
            if Instant::now() >= deadline {
                count_dropped(DropReason::Shutdown, n);
                continue;
            }
            // A timeout drops the batch that was in flight: its result is not known, so it counts as shutdown.
            match tokio::time::timeout_at(deadline, self.write_batch(batch)).await {
                Err(_) => count_dropped(DropReason::Shutdown, n),
                Ok(Err(_)) => tracing::error!("a decision-log writer batch panicked while it drained"),
                Ok(Ok(())) => {}
            }
        }
    }
}

/// Run the writer until the shutdown signal, then drain for at most `budget`.
pub async fn run_writer(rx: mpsc::Receiver<Bytes>, slot: RedisSlot, s: WriterSettings, shutdown: watch::Receiver<()>, budget: Duration) {
    let state = Arc::new(State {
        rx: Mutex::new(rx),
        backlog: Mutex::new(Backlog::default()),
        slot,
        settings: s,
    });
    let it_state = Arc::clone(&state);
    let it_shutdown = shutdown.clone();
    run_supervised("writer", shutdown, move || {
        let state = Arc::clone(&it_state);
        let sd = it_shutdown.clone();
        async move { state.iteration(sd).await }
    })
    .await;
    state.drain(budget).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};

    /// A key that makes `write_batch_inner` panic.
    pub(super) const PANIC_KEY: &str = "panic-in-write-batch";

    #[tokio::test]
    async fn a_panic_inside_write_batch_counts_the_batch_as_write_error() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let (tx, rx) = mpsc::channel(8);
        for _ in 0..3 {
            tx.send(Bytes::from_static(b"e")).await.expect("the queue is open");
        }
        let (stop, shutdown) = watch::channel(());
        let task = tokio::spawn(run_writer(
            rx,
            RedisSlot::new(),
            WriterSettings {
                key: PANIC_KEY.to_owned(),
                max_len: 10,
            },
            shutdown,
            Duration::from_millis(100),
        ));
        // The three events are queued before the first read, so they are one batch.
        tokio::time::sleep(Duration::from_millis(300)).await;
        stop.send(()).expect("the writer is alive");
        task.await.expect("the writer task ends");
        // One snapshot: a read of the recorder resets the counters it returns.
        let snap = snapshotter.snapshot().into_vec();
        let value = |name: &str, label: (&str, &str)| {
            snap.iter().find_map(|(key, _, _, v)| {
                let key = key.key();
                let same = key.name() == name && key.labels().any(|l| l.key() == label.0 && l.value() == label.1);
                match (same, v) {
                    (true, DebugValue::Counter(n)) => Some(*n),
                    _ => None,
                }
            })
        };
        assert_eq!(value(names::GATEWAY_DECISION_LOG_DROPPED_TOTAL, ("reason", "write_error")), Some(3));
        assert_eq!(value(names::GATEWAY_DECISION_LOG_TASK_RESTARTS_TOTAL, ("task", "writer")), Some(1));
    }
}
