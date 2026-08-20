// SPDX-License-Identifier: Apache-2.0

//! Docker-free unit tests for `support::docker::mapped_port`'s retry loop (SMA-521).
//!
//! Lives in its OWN test binary, included via `#[path]`, for two reasons. A `#[cfg(test)]`
//! module inside `docker.rs` would be silently compiled out — `cfg(test)` is not enabled when
//! rustc builds an integration-test binary — and would therefore never run. Plain
//! `#[tokio::test]` functions inside `docker.rs` would instead run once per binary that
//! includes it — currently 60 of them, same count `support_docker_policy.rs` cites for the
//! identical reason — duplicating the same assertions instead of asserting them once, here.

#[path = "support/docker.rs"]
mod docker;

use docker::{PortSource, mapped_port};
use std::sync::atomic::{AtomicU32, Ordering};

/// Fails its first `fails` probes, then reports `port` — the shape of a container whose runtime
/// has not yet published the host-side mapping (`PortNotExposed`).
struct FlakyPort {
    remaining_failures: AtomicU32,
    port: u16,
}

impl PortSource for FlakyPort {
    fn host_port(&self, _port: u16) -> impl std::future::Future<Output = Result<u16, String>> + Send {
        let left = self.remaining_failures.load(Ordering::SeqCst);
        let result = if left > 0 {
            self.remaining_failures.store(left - 1, Ordering::SeqCst);
            Err("PortNotExposed".to_string())
        } else {
            Ok(self.port)
        };
        async move { result }
    }
}

#[tokio::test]
async fn mapped_port_retries_until_the_mapping_is_published() {
    let src = FlakyPort {
        remaining_failures: AtomicU32::new(3),
        port: 54321,
    };

    let port = mapped_port(&src, 6379, "flaky test source").await;

    assert_eq!(port, 54321, "must return the port once the source finally reports it");
    assert_eq!(src.remaining_failures.load(Ordering::SeqCst), 0, "must have consumed every simulated failure");
}

#[tokio::test]
async fn mapped_port_returns_immediately_when_the_mapping_is_already_published() {
    let src = FlakyPort {
        remaining_failures: AtomicU32::new(0),
        port: 5432,
    };

    let port = mapped_port(&src, 5432, "ready test source").await;

    assert_eq!(port, 5432);
}
