// SPDX-License-Identifier: Apache-2.0

//! The canary that makes a Docker-less run of this crate impossible to miss. `boot_pg`,
//! `migration_isolation_pg`, `schema_pg`, `inbox_repository_pg`, `lease_pg`, `claim_pg`, `complete_pg`, `keeper_pg`, `lease_property_pg`, `lease_stress_pg`, `lease_crash_pg`, `ingress_pg`, `ingress_latency_pg`, `nats_source`, `timer_store_pg`, `scheduler_pg` and `timer_wake_pg` return early when Docker is unavailable and report PASS having run
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
        "Docker is unreachable, so the 17 Docker-backed test binaries of this crate (boot_pg, \
         migration_isolation_pg, schema_pg, inbox_repository_pg, lease_pg, claim_pg, complete_pg, keeper_pg, \
         lease_property_pg, lease_stress_pg, lease_crash_pg, ingress_pg, ingress_latency_pg, nats_source, timer_store_pg, scheduler_pg, timer_wake_pg) will report PASS having run nothing.\n  \
         Start the daemon, or run again with PAIGASUS_SKIP_DOCKER=1 to accept the skips."
    );
}
