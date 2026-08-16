// SPDX-License-Identifier: Apache-2.0

//! Standalone container helpers for the integration suites (SMA-521).
//!
//! Deliberately depends on NOTHING else in `support/`, so the four Redis-only test files that
//! have no `mod support;` can pull it in with `#[path = "support/docker.rs"] mod docker;`
//! without dragging in the 791-line support surface (axum, rcgen, the mock IdP) — and without
//! tripping the `dead_code` hard error that `[workspace.lints.rust] warnings = "deny"` makes of
//! `support/mod.rs`'s two non-`#[allow(dead_code)]` items.
//!
//! Every item here carries `#[allow(dead_code)]` for that same reason: most binaries that
//! include this file use only part of it.

use std::time::Duration;
use testcontainers::Image;
use testcontainers::core::ContainerAsync;

/// How long [`mapped_port`] waits for the container runtime to publish a host-side port mapping.
/// A LOAD BUDGET, not an expectation — it returns on the first success, which on an idle machine
/// is immediate. Matches the 90s ceiling `tests/nats_publisher.rs` already uses for the same race.
#[allow(dead_code)]
const PORT_READY_BUDGET: Duration = Duration::from_secs(90);

/// Anything that can report a host-side port for a container port.
///
/// Exists so [`mapped_port`]'s retry loop can be tested without Docker: production code uses the
/// `ContainerAsync<I>` impl below, and `tests/support_docker_retry.rs` substitutes a counter that
/// fails a fixed number of times first.
#[allow(dead_code)]
pub trait PortSource {
    fn host_port(&self, port: u16) -> impl std::future::Future<Output = Result<u16, String>> + Send;
}

impl<I: Image> PortSource for ContainerAsync<I> {
    async fn host_port(&self, port: u16) -> Result<u16, String> {
        self.get_host_port_ipv4(port).await.map_err(|e| e.to_string())
    }
}

/// Resolves a container's mapped host port, retrying until the runtime publishes it.
///
/// **Why this is not a bare `get_host_port_ipv4(..).unwrap()`** (which is what it replaced at 11
/// sites): `AsyncRunner::start` returns once the server has logged that it is listening, but the
/// runtime publishes the host-side port mapping independently — an inspect issued in that gap
/// comes back `PortNotExposed`. It is rare for one container and reproducible when the suite
/// races many of them (`tests/nats_publisher.rs:46-50` documents the same race).
///
/// This is the FAST failure class of SMA-521: it fails in milliseconds, so a nextest retry
/// budget cannot absorb it — all attempts land inside the same contention burst. Retrying here,
/// where the race actually is, is the fix; the retry budget is the backstop.
///
/// Panics after [`PORT_READY_BUDGET`] so a genuinely missing port still fails loudly.
#[allow(dead_code)]
pub async fn mapped_port(src: &impl PortSource, port: u16, what: &str) -> u16 {
    let deadline = std::time::Instant::now() + PORT_READY_BUDGET;
    loop {
        match src.host_port(port).await {
            Ok(mapped) => return mapped,
            Err(e) if std::time::Instant::now() >= deadline => {
                panic!("{what}: container port {port} was never published within {PORT_READY_BUDGET:?}: {e}")
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}
