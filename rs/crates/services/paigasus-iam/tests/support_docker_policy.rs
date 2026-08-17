// SPDX-License-Identifier: Apache-2.0

//! Docker-free unit tests for `support::docker`'s skip policy (SMA-538).
//!
//! Lives in its own test binary, included via `#[path]`, for the same reasons
//! `support_docker_retry.rs` does: a `#[cfg(test)]` module inside `docker.rs` would be
//! silently compiled out (`cfg(test)` is not set when rustc builds an integration-test
//! binary), and bare `#[tokio::test]` functions inside `docker.rs` would run once per
//! including binary — currently 60 of them, `mod support;` or a direct `#[path]` alike —
//! duplicating these assertions instead of asserting them once, here.

#[path = "support/docker.rs"]
mod docker;

use docker::{env_flag, is_daemon_unreachable};
use std::ffi::OsStr;
use std::io::{Error as IoError, ErrorKind};
use testcontainers::bollard::errors::Error as BollardError;
use testcontainers::core::error::{ClientError, TestcontainersError};

// ---------------------------------------------------------------- env_flag

#[test]
fn env_flag_accepts_the_three_documented_truthy_spellings() {
    for on in ["1", "true", "yes", "TRUE", "Yes", "  true  "] {
        assert!(env_flag(Some(OsStr::new(on))), "{on:?} must parse as on");
    }
}

#[test]
fn env_flag_rejects_everything_else_including_zero_and_unset() {
    assert!(!env_flag(None), "unset must be off");
    for off in ["0", "", "no", "false", "maybe", "2", "on"] {
        assert!(!env_flag(Some(OsStr::new(off))), "{off:?} must parse as off");
    }
}

// ------------------------------------------------- is_daemon_unreachable

/// F1 row 1: the socket file is absent. Observed as
/// `failed to initialize a docker client: Socket not found: /nonexistent/docker.sock`.
#[test]
fn missing_socket_is_unreachable() {
    let e = TestcontainersError::Client(ClientError::Init(BollardError::SocketNotFoundError("/nonexistent/docker.sock".to_string())));
    assert!(is_daemon_unreachable(&e));
}

#[test]
fn connection_refused_on_the_transport_is_unreachable() {
    let e = TestcontainersError::Client(ClientError::CreateContainer(BollardError::IOError {
        err: IoError::new(ErrorKind::ConnectionRefused, "connection refused"),
    }));
    assert!(is_daemon_unreachable(&e));
}

/// THE REGRESSION TEST FOR THIS ISSUE'S OWN FIRST DRAFT (spec F3).
///
/// A healthy daemon that cannot reach the registry relays the registry's text verbatim
/// through `DockerResponseServerError`. Any classifier that substring-matched
/// "connection refused" would skip here — with Docker running — silently disabling every
/// Postgres/NATS/Keycloak suite. It must be a hard failure.
#[test]
fn registry_unreachable_through_a_healthy_daemon_is_not_unreachable() {
    let e = TestcontainersError::Client(ClientError::PullImage {
        descriptor: "redis:latest".to_string(),
        err: BollardError::DockerResponseServerError {
            status_code: 500,
            message: r#"Get "https://registry-1.docker.io/v2/": dial tcp 1.2.3.4:443: connect: connection refused"#.to_string(),
        },
    });
    assert!(!is_daemon_unreachable(&e), "a daemon that ANSWERED must never be classified as unreachable");
}

/// `client.rs:259` maps a genuine container-START failure to `ClientError::Init`. The
/// classifier must not be fooled by the variant name — the daemon answered, so this is hard.
#[test]
fn mis_tagged_container_start_failure_is_not_unreachable() {
    let e = TestcontainersError::Client(ClientError::Init(BollardError::DockerResponseServerError {
        status_code: 409,
        message: "container already started".to_string(),
    }));
    assert!(!is_daemon_unreachable(&e));
}

/// A socket we are not allowed to open is a misconfiguration worth seeing, not a skip.
#[test]
fn permission_denied_on_the_socket_is_not_unreachable() {
    let e = TestcontainersError::Client(ClientError::CreateContainer(BollardError::IOError {
        err: IoError::new(ErrorKind::PermissionDenied, "permission denied"),
    }));
    assert!(!is_daemon_unreachable(&e));
}

/// `WaitContainer` is the variant that carries container LOG output, which can contain
/// anything the server logged — including the words a naive classifier looks for. It is not a
/// `Client(_)` error at all, so it can never reach the transport check.
#[test]
fn wait_container_errors_are_never_unreachable() {
    let e = TestcontainersError::WaitContainer(testcontainers::core::error::WaitContainerError::StartupTimeout);
    assert!(!is_daemon_unreachable(&e));
}

/// A fixture file that `with_copy_to` cannot read is a real failure of the test's own setup.
#[test]
fn copy_to_container_failure_is_not_unreachable() {
    let e = TestcontainersError::Client(ClientError::CopyToContainerError(testcontainers::core::CopyToContainerError::IoError(IoError::new(
        ErrorKind::NotFound,
        "no such file or directory",
    ))));
    assert!(!is_daemon_unreachable(&e));
}
