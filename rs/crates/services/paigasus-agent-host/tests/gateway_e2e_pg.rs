// SPDX-License-Identifier: Apache-2.0

//! The real binary on a fake gateway (spec G5, §7.6). It fails when the `register()` call in
//! `main.rs` is deleted: the boot then refuses the agent, because its model is not registered.

mod support;

use std::time::{Duration, Instant};

use support::fake_gateway::{FakeGateway, Reply, TEXT_STREAM};
use support::inbox::insert_inbox;
use support::lease::insert_pending_event;
use uuid::Uuid;

const READY_BUDGET: Duration = Duration::from_secs(90);
const DONE_BUDGET: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(50);
const KEY: &str = "sk-gw-e2e-0123456789abcdef";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_real_binary_runs_an_agent_through_the_gateway() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping the gateway e2e test: Docker unavailable");
        return;
    };
    let (org, inbox, agent, principal, event) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let fake = FakeGateway::start(vec![Reply::Sse(TEXT_STREAM.to_owned())]).await;

    // The key file and the config file are in one temp dir, which is the working dir of the child.
    let dir = tempfile::tempdir().expect("temp dir");
    let key_file = dir.path().join("gateway-key");
    std::fs::write(&key_file, KEY).expect("write the key file");
    let id = paigasus_agent_host_core::ids::AgentId::new(org, agent).as_prn().canonical();
    std::fs::write(
        dir.path().join("agent-host.toml"),
        format!(
            "[gateway]\nbase_url = \"http://{addr}\"\napi_key_file = \"{key}\"\nread_timeout_ms = 5000\n\n\
             [[models]]\nname = \"claude\"\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-5-5\"\nmax_output_tokens = 16000\n\n\
             [[agents]]\nid = \"{id}\"\nname = \"e2e\"\nmodel = \"claude\"\ninstructions = \"Answer.\"\ntools = []\n\n\
             [worker]\nconcurrency = 1\npoll_interval_ms = 200\n",
            addr = fake.addr,
            key = key_file.display(),
        ),
    )
    .expect("write agent-host.toml");

    let (mut child, port) = support::spawn_service_in(&url, Some(dir.path()), &[("RUST_LOG", "paigasus_agent_host=info,warn".to_owned())]);
    support::wait_for_status(&mut child, port, "/readyz", 200, READY_BUDGET).await;

    let db = support::connect(&url).await;
    insert_inbox(&db, org, inbox, agent, principal).await;
    insert_pending_event(&db, inbox, event, 0).await;

    let done = format!("SELECT count(*) AS n FROM inbox_event WHERE id = '{event}' AND status = 'done'");
    let started = Instant::now();
    while support::scalar_i64(&db, &done).await != 1 {
        if let Some(exit) = child.child.try_wait().expect("try_wait") {
            panic!("the service exited ({exit}) before the event was done; log:\n{}", child.tail());
        }
        assert!(started.elapsed() < DONE_BUDGET, "the event was not done within {DONE_BUDGET:?}; log:\n{}", child.tail());
        tokio::time::sleep(POLL).await;
    }

    let log = child.tail();
    let run = format!("FROM run WHERE inbox_id = '{inbox}'");
    assert_eq!(
        support::scalar_i64(&db, &format!("SELECT count(*) AS n {run} AND status = 'completed'")).await,
        1,
        "one completed run; log:\n{log}"
    );
    assert_eq!(
        support::scalar_i64(&db, &format!("SELECT count(*) AS n {run} AND final_output = 'Hello, it''s nice to meet you!'")).await,
        1,
        "the final output is the text of the fixture; log:\n{log}"
    );
    assert_eq!(support::scalar_i64(&db, &format!("SELECT input_tokens::bigint AS n {run}")).await, 19, "input tokens; log:\n{log}");
    assert_eq!(support::scalar_i64(&db, &format!("SELECT output_tokens::bigint AS n {run}")).await, 15, "output tokens; log:\n{log}");

    let requests = fake.requests();
    assert!(!requests.is_empty(), "the model sent a request; log:\n{log}");
    assert_eq!(requests[0].headers.get("x-api-key").and_then(|v| v.to_str().ok()), Some(KEY), "the key header");
    assert_eq!(requests[0].body["max_tokens"], 16000, "the max_tokens of the model entry");
    let probes = fake.models_requests();
    assert_eq!(probes.len(), 1, "the boot probe asked once for the models");
    assert!(probes[0].contains_key("anthropic-version"), "the probe carries anthropic-version");
    assert!(!log.contains(KEY), "the key must not reach the log; log:\n{log}");
}
