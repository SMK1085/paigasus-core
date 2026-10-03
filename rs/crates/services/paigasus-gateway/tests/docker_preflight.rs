// SPDX-License-Identifier: Apache-2.0

//! The canary that makes a Docker-less run of this crate impossible to miss (SMA-538, SMA-677
//! D22). `limits_store_redis` and `limits_redis_e2e` each return early when Docker is unavailable,
//! reporting PASS having executed nothing, and nextest and Moon both discard a passing test's
//! output. So this test FAILS instead: one red, named for the actual problem.

#[tokio::test]
async fn docker_backed_suites_can_actually_run() {
    if paigasus_test_docker::skip_docker() {
        eprintln!("SKIP[docker-unavailable] docker_preflight: PAIGASUS_SKIP_DOCKER is set");
        return;
    }
    assert!(
        paigasus_test_docker::start_redis_or_skip("docker_preflight").await.is_some(),
        "Docker is unreachable, so this crate's 2 Redis-backed test binaries (limits_store_redis, \
         limits_redis_e2e) will report PASS having executed nothing.\n  \
         Start the daemon, or re-run with PAIGASUS_SKIP_DOCKER=1 to accept the skips."
    );
}
