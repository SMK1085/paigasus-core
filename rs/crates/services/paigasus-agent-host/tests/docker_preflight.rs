// SPDX-License-Identifier: Apache-2.0

//! The canary that makes a Docker-less run of this crate impossible to miss. `boot_pg`,
//! `migration_isolation_pg`, `schema_pg` and `inbox_repository_pg` return early when Docker is unavailable and report PASS having run
//! nothing. Nextest and Moon both discard the output of a passing test. So this test FAILS
//! instead: one red result, named for the actual problem.

mod support;

#[tokio::test]
async fn docker_backed_suites_can_actually_run() {
    if paigasus_test_docker::skip_docker() {
        eprintln!("SKIP[docker-unavailable] docker_preflight: PAIGASUS_SKIP_DOCKER is set");
        return;
    }
    assert!(
        support::start_raw_postgres().await.is_some(),
        "Docker is unreachable, so the 4 Docker-backed test binaries of this crate (boot_pg, \
         migration_isolation_pg, schema_pg, inbox_repository_pg) will report PASS having run nothing.\n  \
         Start the daemon, or run again with PAIGASUS_SKIP_DOCKER=1 to accept the skips."
    );
}
