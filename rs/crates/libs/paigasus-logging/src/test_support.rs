// SPDX-License-Identifier: Apache-2.0

//! Test-only log capture for tests that assert on a `tracing` line (SMA-689).
//!
//! This is the one copy of this helper in the workspace. Do not write a new `LogBuffer` in a
//! service crate. Turn the helper on from `[dev-dependencies]` only, so that the production
//! binary never contains it:
//!
//! ```toml
//! [dev-dependencies]
//! paigasus-logging = { path = "../../libs/paigasus-logging", version = "0.0.0", features = ["test-support"] }
//! ```
//!
//! # Usage rules
//!
//! - Bind the guard to a named variable: `let (logs, _guard) = capture_logs();`. Never bind it
//!   to `_`: `let (logs, _) = capture_logs();` drops the guard at once, and the capture then
//!   sees nothing. No lint catches this.
//! - The subscriber is thread-local. `#[tokio::test]` is current-thread, so the subscriber sees
//!   every task of the test, spawned tasks included. A `multi_thread` runtime test, or a line
//!   logged on another OS thread, is NOT captured.
//! - [`capture_logs`] captures at `TRACE`. A "this is never logged" assertion is sound only at
//!   `TRACE`, because a lower level hides the lines that it does not capture.
//!
//! # Limits
//!
//! - **Supported runner: `cargo nextest`** (one process per test). `tracing` caches callsite
//!   interest for the whole process. Under `cargo test` (one process, many test threads) a
//!   log assertion can miss a line and flake. The repo's Moon `test` task and CI use nextest.
//! - The captured text is the `fmt` text format with no ANSI escapes. It is NOT the JSON line
//!   format that [`crate::init`] writes in production, so captured text is not evidence of the
//!   production line format.

use std::sync::{Arc, Mutex};

/// A `tracing` writer that keeps every formatted line in memory. An implementation detail of
/// [`capture_logs`]; read it with [`LogBuffer::text`] only.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl LogBuffer {
    /// Everything written so far, as text.
    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

/// Installs a thread-local subscriber at `TRACE` and returns its buffer and guard. Bind the
/// guard to a named variable (`_guard`), never to `_`. See the module doc for the limits.
#[must_use = "bind the guard to a named variable, or the capture stops at once"]
pub fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    capture_logs_at(tracing::Level::TRACE)
}

/// Installs a thread-local subscriber at `level` and returns its buffer and guard. Bind the
/// guard to a named variable (`_guard`), never to `_`. See the module doc for the limits.
#[must_use = "bind the guard to a named variable, or the capture stops at once"]
pub fn capture_logs_at(level: tracing::Level) -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt().with_writer(buffer.clone()).with_ansi(false).with_max_level(level).finish();
    (buffer, tracing::subscriber::set_default(subscriber))
}

#[cfg(test)]
mod tests {
    use super::{capture_logs, capture_logs_at};
    use std::sync::{Mutex, MutexGuard, PoisonError};
    use tracing::Level;

    // Every test here installs a scoped subscriber, and one test mutates `NO_COLOR`. Under
    // `cargo test` (thread-parallel) that can flake (see the module doc), so every test holds
    // this lock for its whole body. `cargo nextest` does not need it; it is harmless there. A
    // panicking test poisons the lock; take it anyway, so one failure does not red every test.
    static LOCK: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[test]
    fn capture_logs_sees_trace() {
        let _serial = serial();
        let (logs, _guard) = capture_logs();
        tracing::trace!("probe-trace");
        assert!(logs.text().contains("probe-trace"), "capture_logs must capture TRACE: {:?}", logs.text());
    }

    #[test]
    fn capture_logs_at_info_drops_debug() {
        let _serial = serial();
        let (logs, _guard) = capture_logs_at(Level::INFO);
        tracing::debug!("d-probe");
        tracing::info!("i-probe");
        let text = logs.text();
        assert!(text.contains("i-probe"), "INFO must be captured: {text:?}");
        assert!(!text.contains("d-probe"), "DEBUG must not be captured at INFO: {text:?}");
    }

    #[test]
    fn capture_has_no_ansi_escape() {
        let _serial = serial();
        // `tracing-subscriber` 0.3.23 turns ANSI on by default (the `ansi` feature) unless
        // `NO_COLOR` is set. Remove it, so this test proves `with_ansi(false)` and not the
        // runner's environment.
        // SAFETY: `serial()` holds the lock that every test of this module takes; no other
        // test of this crate reads `NO_COLOR`.
        unsafe { std::env::remove_var("NO_COLOR") };
        let (logs, _guard) = capture_logs();
        tracing::info!("ansi-probe");
        let text = logs.text();
        assert!(text.contains("ansi-probe"), "the line must be captured: {text:?}");
        assert!(!text.contains('\u{1b}'), "the capture must have no ANSI escape: {text:?}");
    }

    #[test]
    fn guard_drop_stops_capture() {
        let _serial = serial();
        let (logs, guard) = capture_logs();
        tracing::info!("before-drop");
        drop(guard);
        tracing::info!("after-drop");
        let text = logs.text();
        assert!(text.contains("before-drop"), "a line before the drop is captured: {text:?}");
        assert!(!text.contains("after-drop"), "a line after the drop is not captured: {text:?}");
    }

    // Review Focus 3: the module doc says `_` drops the guard at once. Pin that.
    #[test]
    fn underscore_guard_captures_nothing() {
        let _serial = serial();
        let (logs, _) = capture_logs();
        tracing::info!("underscore-probe");
        assert!(!logs.text().contains("underscore-probe"), "a `_` guard drops at once: {:?}", logs.text());
    }

    // Review Focus 1: the subscriber is thread-local.
    #[test]
    fn other_thread_is_not_captured() {
        let _serial = serial();
        let (logs, _guard) = capture_logs();
        std::thread::spawn(|| tracing::info!("other-thread-probe")).join().unwrap();
        tracing::info!("this-thread-probe");
        let text = logs.text();
        assert!(text.contains("this-thread-probe"), "this thread is captured: {text:?}");
        assert!(!text.contains("other-thread-probe"), "another thread is not captured: {text:?}");
    }

    // Review Focus 2: an inner capture restores the outer one when its guard drops.
    #[test]
    fn nested_capture_restores_the_outer_capture() {
        let _serial = serial();
        let (outer, _outer_guard) = capture_logs();
        {
            let (inner, _inner_guard) = capture_logs_at(Level::INFO);
            tracing::info!("inner-probe");
            assert!(inner.text().contains("inner-probe"), "inner capture: {:?}", inner.text());
        }
        tracing::info!("outer-probe");
        let text = outer.text();
        assert!(text.contains("outer-probe"), "the outer capture is active again: {text:?}");
        assert!(!text.contains("inner-probe"), "the inner line went only to the inner buffer: {text:?}");
    }

    // Review Focus 4: `text()` decodes UTF-8; a non-ASCII value must round-trip.
    #[test]
    fn non_ascii_text_round_trips() {
        let _serial = serial();
        let (logs, _guard) = capture_logs();
        tracing::info!(name = "Jürgen 🚀", "unicode-probe");
        let text = logs.text();
        assert!(text.contains("unicode-probe") && text.contains("Jürgen 🚀"), "non-ASCII survives: {text:?}");
    }
}
