// SPDX-License-Identifier: Apache-2.0

//! The canary that makes a Docker-less run of this crate impossible to miss (SMA-538).
//!
//! 57 of this crate's 60 integration binaries start a container, and each returns early when
//! Docker is unavailable — reporting PASS in under a second having executed nothing. The
//! `SKIP[docker-unavailable]` markers those suites print cannot fix that: nextest discards a
//! PASSING test's stderr (`success-output` defaults to `never`) and Moon discards a passing
//! TASK's output (`buffer-only-failure` in `.moon/tasks.yml`).
//!
//! So this test FAILS instead. A failure is shown by both. One red, named for the actual
//! problem, in place of 56 silent greens.
//!
//! It starts a real Redis rather than pinging the daemon: testcontainers exposes no ping, and
//! merely constructing a client is not a probe — that succeeds when the endpoint exists with
//! nothing listening. Redis is already pulled by five other suites, so this costs no new image.
//! Reusing `start_redis_or_skip` means the canary exercises the very policy it guards.

#[path = "support/docker.rs"]
mod docker;

#[tokio::test]
async fn docker_backed_suites_can_actually_run() {
    if docker::skip_docker() {
        eprintln!("SKIP[docker-unavailable] docker_preflight: PAIGASUS_SKIP_DOCKER is set");
        return;
    }

    assert!(
        docker::start_redis_or_skip("docker_preflight").await.is_some(),
        "Docker is unreachable, so 56 of this crate's 60 integration suites will report PASS \
         having executed nothing.\n  \
         Start the daemon, or re-run with PAIGASUS_SKIP_DOCKER=1 to accept the skips."
    );
}
