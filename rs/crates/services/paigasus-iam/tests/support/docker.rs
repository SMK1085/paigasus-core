// SPDX-License-Identifier: Apache-2.0

//! Standalone container helpers for the integration suites (SMA-521).
//!
//! Deliberately depends on nothing else in `support/`, so the test files that have no
//! `mod support;` can pull it in with `#[path = "support/docker.rs"] mod docker;`
//! without dragging in the 776-line support surface (axum, rcgen, the mock IdP) — and without
//! tripping the `dead_code` hard error that `[workspace.lints.rust] warnings = "deny"` makes of
//! `support/mod.rs`'s two non-`#[allow(dead_code)]` items.
//!
//! Every item here carries `#[allow(dead_code)]` for that same reason: most binaries that
//! include this file use only part of it.

use std::ffi::OsStr;
use std::io::ErrorKind;
use std::time::Duration;
use testcontainers::Image;
use testcontainers::bollard::errors::Error as BollardError;
use testcontainers::core::ContainerAsync;
use testcontainers::core::ContainerRequest;
use testcontainers::core::error::{ClientError, TestcontainersError};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::redis::Redis;

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

/// Parses a human-typed on/off environment variable: `1`, `true` or `yes`, case-insensitively
/// and ignoring surrounding whitespace. Everything else — including `0`, the empty string and
/// unset — is off.
///
/// Deliberately NOT the presence-based form the adjacent `CI` check uses. `CI` is set by a
/// platform and any value it carries means "in CI"; these two are typed by a human, for whom
/// `PAIGASUS_REQUIRE_DOCKER=0` silently meaning "on" would be a footgun.
///
/// Takes the raw value rather than reading the environment itself so it can be unit-tested
/// without `unsafe { std::env::set_var(..) }` (unsafe under edition 2024) and without assuming
/// anything about process isolation between tests.
#[allow(dead_code)]
pub fn env_flag(raw: Option<&OsStr>) -> bool {
    let Some(v) = raw.and_then(OsStr::to_str) else {
        return false;
    };
    matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")
}

/// Whether the caller has explicitly accepted skipping the Docker-backed suites.
///
/// `CI` outranks it: a stray `PAIGASUS_SKIP_DOCKER` in a workflow file must not be able to
/// green a CI run that tested nothing.
#[allow(dead_code)]
pub fn skip_docker() -> bool {
    std::env::var_os("CI").is_none() && env_flag(std::env::var_os("PAIGASUS_SKIP_DOCKER").as_deref())
}

/// Whether a missing daemon must be a hard failure rather than a skip. `CI` implies it.
#[allow(dead_code)]
pub fn require_docker() -> bool {
    std::env::var_os("CI").is_some() || env_flag(std::env::var_os("PAIGASUS_REQUIRE_DOCKER").as_deref())
}

/// Whether `e` or anything in its `source()` chain is an [`std::io::Error`] with
/// [`ErrorKind::PermissionDenied`].
///
/// Exists because `hyper_util`'s error keeps the real cause behind `source()` and exposes no
/// accessor for it: `is_connect()` reports only that the failure happened during connect. A
/// docker socket we are not permitted to open fails during connect, and must NOT be treated as
/// "no daemon there" — it is a misconfiguration a developer needs to see.
///
/// Takes `&dyn Error` rather than a concrete type so it is testable:
/// `hyper_util::client::legacy::Error` has no public constructor, so the hyper path itself cannot
/// be unit-tested, but the chain-walking logic can be, and is
/// (`tests/support_docker_policy.rs`).
#[allow(dead_code)]
pub fn source_chain_is_permission_denied(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if let Some(io) = err.downcast_ref::<std::io::Error>()
            && io.kind() == ErrorKind::PermissionDenied
        {
            return true;
        }
        cur = err.source();
    }
    false
}

/// Whether a failed `start()` means the Docker daemon could not be reached at all, as opposed
/// to a container that genuinely failed with a healthy daemon.
///
/// **Classifies by TYPE, never by message text.** An earlier draft of SMA-538 substring-matched
/// the rendered error for markers like `connection refused`, which fails OPEN: bollard's
/// `DockerResponseServerError` interpolates daemon-authored free text into its `Display`, and
/// `async_runner.rs:343-358` pulls an uncached image whenever `create_container` returns 404,
/// so a healthy daemon that cannot reach the registry relays the registry's own
/// `connect: connection refused` and every suite would have skipped with Docker running.
///
/// It also removes any need to know that `client.rs:259` mis-maps a container-START failure to
/// `ClientError::Init`: that error carries a daemon RESPONSE, so it lands on the `false` side
/// structurally.
///
/// **The split, plainly:** `true` means "the daemon never answered" (a raw transport error —
/// socket absent, connection refused/reset/aborted); `false` means "the daemon answered, or we
/// are misconfigured" — including a request timeout. A timeout is deliberately on the `false`
/// (failure) side: it cannot distinguish a daemon that is stalled-but-alive under load from one
/// that never answered at all, and treating it as unreachable would silently skip a real,
/// diagnosable problem. `PAIGASUS_SKIP_DOCKER` is the escape hatch for a genuinely dead
/// `DOCKER_HOST`, not this classifier.
#[allow(dead_code)]
pub fn is_daemon_unreachable(e: &TestcontainersError) -> bool {
    let TestcontainersError::Client(client) = e else {
        // WaitContainer (which carries container LOG output), PortNotExposed, Exec, MissingInfo,
        // Io and Other are all failures of a daemon that answered us.
        return false;
    };

    // EXHAUSTIVE on purpose — no `_` arm. `ClientError` is not `#[non_exhaustive]`, so a
    // testcontainers upgrade that adds a variant becomes a COMPILE ERROR here rather than a
    // silent reclassification. If rustc reports a missing variant, decide which side it belongs
    // on: does it carry a raw transport error, or did the daemon answer?
    let bollard: &BollardError = match client {
        // The daemon never answered — these wrap a raw transport error.
        ClientError::Init(b)
        | ClientError::ListContainers(b)
        | ClientError::CreateContainer(b)
        | ClientError::RemoveContainer(b)
        | ClientError::StartContainer(b)
        | ClientError::StopContainer(b)
        | ClientError::PauseContainer(b)
        | ClientError::UnpauseContainer(b)
        | ClientError::InspectContainer(b)
        | ClientError::CreateNetwork(b)
        | ClientError::InspectNetwork(b)
        | ClientError::ListNetworks(b)
        | ClientError::RemoveNetwork(b)
        | ClientError::InitExec(b)
        | ClientError::InspectExec(b)
        | ClientError::UploadToContainerError(b) => b,

        // The daemon ANSWERED, or we never reached it for a reason of our own making. Never a
        // skip — `PullImage` in particular is where the fail-open lived.
        ClientError::PullImage { .. }
        | ClientError::BuildImage { .. }
        | ClientError::Configuration(_)
        | ClientError::InvalidDockerHost(_)
        | ClientError::PortMapping(_)
        | ClientError::CopyToContainerError(_)
        | ClientError::CopyFromContainerError(_) => return false,
    };

    // The split below is "the daemon never answered" (`true`) vs "the daemon answered, or we
    // are misconfigured" (`false`) — never message text.
    match bollard {
        BollardError::SocketNotFoundError(_) => true,
        // `is_connect()` separates a genuine connect failure from a post-connect protocol error —
        // but it is only `matches!(self.kind, ErrorKind::Connect)` (hyper-util 0.1.20), so it says
        // NOTHING about why the connect failed. EACCES on the docker socket is a connect-phase
        // failure, so `is_connect()` alone would skip on it. That is the same misconfiguration the
        // `IOError` arm below deliberately hard-fails, and this is the path it actually travels:
        // `connect_with_unix` goes through the hyper-util client, not through `IOError`. Hence the
        // source-chain check — without it, "permission denied fails closed" would be true only on
        // a variant nothing reaches (CodeRabbit, SMA-538).
        BollardError::HyperLegacyError { err } => err.is_connect() && !source_chain_is_permission_denied(err),
        // `NotFound` is deliberately absent here, and so is `PermissionDenied`. The genuine
        // missing-socket case is `SocketNotFoundError` above — bollard checks the path first and
        // returns that, never an io `NotFound` (verified empirically, and in bollard 0.20.2's
        // `connect_with_unix`). So an io error reaching this arm did NOT come from an absent
        // socket, and the only kinds that mean "nobody answered" are the connection ones.
        //
        // No producer of `IOError { NotFound }` is known on the reachable paths in bollard 0.20.2
        // — a missing TLS `ca.pem` maps to `CertPathError`, not to this variant, and falls to the
        // catch-all below. That is a reason this arm is right, not a reason to widen it: anything
        // unrecognised must red rather than skip, exactly like the `PermissionDenied` case, which
        // is a client misconfiguration against a perfectly healthy daemon.
        BollardError::IOError { err } => matches!(err.kind(), ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted),
        // Deliberately `false`, not `true`. testcontainers sets a 120s per-request timeout
        // (testcontainers' `bollard_client.rs`) and bollard raises this from a `tokio::time::timeout`
        // wrapped around the whole request — so it fires on a LIVE daemon whose `create_container`
        // merely stalled past 120s under load (`rs/.config/nextest.toml` documents runs stretching
        // to 123.4s under contention). A timeout cannot distinguish "stalled but alive" from "never
        // answered", so it goes on the failure side: `PAIGASUS_SKIP_DOCKER` is the recourse for a
        // genuinely blackholed `DOCKER_HOST`, not a silent skip that hides a real slowdown.
        BollardError::RequestTimeoutError => false,
        // NOT exhaustive here, unlike the match above: several bollard variants are
        // `#[cfg(feature = ...)]`-gated (ssl_providerless, websocket, http, ssh, pipe), so an
        // exhaustive match would stop compiling whenever a feature toggles anywhere in the
        // workspace. `false` fails CLOSED — an unrecognised bollard error reds, never skips.
        // Notably this is where `DockerResponseServerError` lands.
        _ => false,
    }
}

/// The SINGLE definition of what happens when a container will not start (SMA-538).
///
/// Ordered rules, first match wins:
///   1. `Ok` -> `Some(node)`
///   2. `CI` present -> panic. Docker is mandatory in CI, and `CI` outranks
///      `PAIGASUS_SKIP_DOCKER` so no workflow-file env var can green a run that tested nothing.
///   3. `PAIGASUS_SKIP_DOCKER` on -> skip. The escape hatch for a Docker Hub rate limit or a
///      daemon restart; it outranks REQUIRE because it is the recourse of last resort.
///   4. `PAIGASUS_REQUIRE_DOCKER` on -> panic.
///   5. daemon unreachable -> skip.
///   6. otherwise -> panic. A container that failed with a REACHABLE daemon is a real failure,
///      not a reason to skip (AC 2).
///
/// A skip emits `SKIP[docker-unavailable] {what}: {e}` on stderr. That line is discarded by
/// nextest on the passing path (`success-output` defaults to `never`) and again by Moon
/// (`buffer-only-failure`), which is exactly why `tests/docker_preflight.rs` exists: it turns a
/// Docker-less run into one loud red instead of 64 quiet passes.
#[allow(dead_code)]
pub async fn start_or_skip<T, I>(image: T, what: &str) -> Option<ContainerAsync<I>>
where
    T: Into<ContainerRequest<I>> + Send,
    I: Image,
{
    match image.start().await {
        Ok(node) => Some(node),
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("{what}: Docker is required in CI: {e}");
            }
            if skip_docker() {
                eprintln!("SKIP[docker-unavailable] {what}: {e}");
                return None;
            }
            if require_docker() {
                panic!("{what}: PAIGASUS_REQUIRE_DOCKER is set and Docker is unusable: {e}");
            }
            if is_daemon_unreachable(&e) {
                eprintln!("SKIP[docker-unavailable] {what}: {e}");
                return None;
            }
            panic!("{what}: the Docker daemon is reachable but the container failed to start, which is a real failure, not a reason to skip. Set PAIGASUS_SKIP_DOCKER=1 to skip anyway: {e}");
        }
    }
}

/// An ephemeral Redis plus its connection URL — the shape six suites each hand-rolled.
#[allow(dead_code)]
pub async fn start_redis_or_skip(what: &str) -> Option<(ContainerAsync<Redis>, String)> {
    let node = start_or_skip(Redis::default(), what).await?;
    let port = mapped_port(&node, 6379, "redis").await;
    Some((node, format!("redis://127.0.0.1:{port}")))
}
