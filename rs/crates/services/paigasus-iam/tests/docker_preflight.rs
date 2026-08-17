// SPDX-License-Identifier: Apache-2.0

//! The canary that makes a Docker-less run of this crate impossible to miss (SMA-538).
//!
//! 58 of this crate's 62 integration binaries start a container, and each returns early when
//! Docker is unavailable — reporting PASS in under a second having executed nothing. The
//! `SKIP[docker-unavailable]` markers those suites print cannot fix that: nextest discards a
//! PASSING test's stderr (`success-output` defaults to `never`) and Moon discards a passing
//! TASK's output (`buffer-only-failure` in `.moon/tasks.yml`).
//!
//! So this test FAILS instead. A failure is shown by both. One red, named for the actual
//! problem, in place of 57 silent greens.
//!
//! **These counts are derived, not typed by hand.** Re-derive them from `tests/` before editing
//! this file, and update all four sites this figure appears at (this doc comment twice, the
//! assertion message below, and `CLAUDE.md`/`docs/dev-setup.md`'s Docker gotchas):
//! `ls *.rs | wc -l` for the total (62); `grep -Ln "start_or_skip\|start_redis_or_skip\|
//! start_migrated_postgres\|start_raw_postgres" *.rs` for the binaries that never start a
//! container (currently `grpc_health.rs`, `health.rs`, `support_docker_policy.rs`,
//! `support_docker_retry.rs` — 4 of them). Docker-backed = total minus that count (58). The
//! assertion message below subtracts one more, for this binary itself, since when THIS test is
//! the one reporting red it is not among the suites silently passing (57).
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
        "Docker is unreachable, so 57 of this crate's 62 integration suites will report PASS \
         having executed nothing.\n  \
         Start the daemon, or re-run with PAIGASUS_SKIP_DOCKER=1 to accept the skips."
    );
}
