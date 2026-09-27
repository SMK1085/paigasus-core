// SPDX-License-Identifier: Apache-2.0

//! Test-only log capture, shared by every unit test module in this crate. A test module can
//! import a `#[cfg(test)]` item from another module of the same crate (as
//! `adapters/retryable.rs`'s `tests_support` shows). Integration tests cannot, so
//! `tests/support/mod.rs` keeps its own copy.

use std::sync::{Arc, Mutex};

/// A `tracing` writer that keeps every formatted line in memory.
#[derive(Clone, Default)]
pub(crate) struct LogBuffer(Arc<Mutex<Vec<u8>>>);

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
    pub(crate) fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

/// Installs a thread-local subscriber at TRACE, so a "no personal data" assertion sees every
/// level. `#[tokio::test]` is current-thread, so the subscriber sees every task of the test.
/// Bind the guard to a named variable (`_guard`), never to `_`, or it drops at once.
pub(crate) fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt().with_writer(buffer.clone()).with_ansi(false).with_max_level(tracing::Level::TRACE).finish();
    (buffer, tracing::subscriber::set_default(subscriber))
}
