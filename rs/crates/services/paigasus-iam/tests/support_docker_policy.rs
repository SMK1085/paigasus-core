// SPDX-License-Identifier: Apache-2.0

//! Docker-free unit tests for `support::docker`'s skip policy (SMA-538).
//!
//! Lives in its own test binary, included via `#[path]`, for the same reasons
//! `support_docker_retry.rs` does: a `#[cfg(test)]` module inside `docker.rs` would be
//! silently compiled out (`cfg(test)` is not set when rustc builds an integration-test
//! binary), and bare `#[tokio::test]` functions inside `docker.rs` would run once per
//! including binary — currently 67 of them, `mod support;` or a direct `#[path]` alike —
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

/// A live but heavily loaded daemon whose `create_container` stalls past testcontainers' 120s
/// per-request timeout must be a hard failure, not a skip — the timeout cannot tell "stalled but
/// alive" apart from "never answered", and `rs/.config/nextest.toml` documents container-startup
/// runs stretching to 123.4s under contention, which is exactly this case, not a dead daemon.
#[test]
fn request_timeout_is_not_unreachable() {
    let e = TestcontainersError::Client(ClientError::CreateContainer(BollardError::RequestTimeoutError));
    assert!(!is_daemon_unreachable(&e), "a request timeout against a live daemon must never be classified as unreachable");
}

/// With `DOCKER_TLS_VERIFY` set, a missing `key.pem`/`cert.pem`/`ca.pem` surfaces as
/// `fs::File::open(path)?` inside `ClientError::Init`, which bollard reports as `IOError { err:
/// NotFound }` — a client TLS misconfiguration against a healthy daemon, not an absent socket
/// (the genuine missing-socket case is `SocketNotFoundError`, covered separately). Structurally
/// identical to the `PermissionDenied` case above: a real, diagnosable failure, not a skip.
#[test]
fn missing_tls_cert_file_is_not_unreachable() {
    let e = TestcontainersError::Client(ClientError::Init(BollardError::IOError {
        err: IoError::new(ErrorKind::NotFound, "No such file or directory (os error 2)"),
    }));
    assert!(!is_daemon_unreachable(&e), "a missing TLS cert file against a healthy daemon must never be classified as unreachable");
}

// --------------------------------------- source_chain_is_permission_denied

/// A two-level chain, so the walk has to recurse rather than only inspect the head.
#[derive(Debug)]
struct Wrapper(IoError);

impl std::fmt::Display for Wrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "wrapper")
    }
}

impl std::error::Error for Wrapper {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// EACCES on the docker socket is a CONNECT-phase failure, so `is_connect()` is true for it and
/// it would otherwise skip. It is a misconfiguration against a daemon that may be perfectly
/// healthy — the same case the `IOError` arm hard-fails — so it has to red instead.
///
/// This covers the chain-walking logic only. `hyper_util::client::legacy::Error` has no public
/// constructor, so the wrapper this guard actually runs against cannot be built in a test; that
/// gap is why the logic is factored out into a `&dyn Error` helper rather than inlined.
#[test]
fn permission_denied_is_found_through_the_source_chain() {
    let e = Wrapper(IoError::new(ErrorKind::PermissionDenied, "permission denied"));
    assert!(docker::source_chain_is_permission_denied(&e));
}

#[test]
fn a_refused_connection_is_not_permission_denied() {
    let e = Wrapper(IoError::new(ErrorKind::ConnectionRefused, "connection refused"));
    assert!(!docker::source_chain_is_permission_denied(&e));
}

#[test]
fn an_error_chain_without_a_permission_error_is_not_flagged() {
    let e = IoError::other("nothing permission-related in here");
    assert!(!docker::source_chain_is_permission_denied(&e));
}
