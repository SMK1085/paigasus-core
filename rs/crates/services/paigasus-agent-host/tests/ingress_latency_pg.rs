// SPDX-License-Identifier: Apache-2.0

//! The handler latency test (spec §10.4). The built binary runs against a real Postgres and a
//! tonic fake IAM, with the worker off. 1000 POSTs at a concurrency of 16 must give a p99 under
//! 50 ms. This proves the done-when "the handler p99 stays under 50 ms with the agent turned off".

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use support::fake_iam::{FakeIam, serve_grpc};
use support::inbox::insert_inbox;
use tokio::sync::Semaphore;
use uuid::Uuid;

const READY_BUDGET: Duration = Duration::from_secs(90);
const TOKEN: &str = "latency-secret";
const REQUESTS: usize = 1000;
const CONCURRENCY: usize = 16;
const WARMUP: usize = 20;
const P99_LIMIT: Duration = Duration::from_millis(50);

fn event(id: &str) -> String {
    serde_json::json!({"specversion": "1.0", "id": id, "source": "caller", "type": "user.message", "data": {"text": "hi"}}).to_string()
}

#[tokio::test]
async fn handler_p99_is_under_50_ms_with_the_worker_off() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping the latency test: Docker unavailable");
        return;
    };
    let owner = Uuid::from_u128(4);
    let inbox = Uuid::from_u128(2);
    let iam = FakeIam::new(TOKEN, owner);
    let (addr, server) = serve_grpc(iam.clone()).await;

    let (mut child, port) = support::spawn_service_with(
        &url,
        &[
            ("AGENT_HOST_IAM__GRPC_ADDR", format!("http://{addr}")),
            ("AGENT_HOST_IAM__TLS__MODE", "loopback_insecure".to_owned()),
            ("AGENT_HOST_WORKER__ENABLED", "false".to_owned()),
        ],
    );
    support::wait_for_status(&mut child, port, "/readyz", 200, READY_BUDGET).await;

    let db = support::connect(&url).await;
    insert_inbox(&db, Uuid::from_u128(1), inbox, Uuid::from_u128(3), owner).await;

    // reqwest 0.13 needs an installed CryptoProvider before it builds a `Client` (SMA-809).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::new();
    let endpoint = format!("http://127.0.0.1:{port}/v1/inboxes/{inbox}/events");

    let post = |id: String| {
        let client = client.clone();
        let endpoint = endpoint.clone();
        async move {
            let started = Instant::now();
            let resp = client
                .post(&endpoint)
                .header("content-type", "application/cloudevents+json")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(event(&id))
                .send()
                .await
                .expect("send");
            let status = resp.status().as_u16();
            let _ = resp.bytes().await;
            (status, started.elapsed())
        }
    };

    for n in 0..WARMUP {
        let (status, _) = post(format!("warm-{n}")).await;
        assert_eq!(status, 202, "warm-up request {n}; log:\n{}", child.tail());
    }

    let gate = Arc::new(Semaphore::new(CONCURRENCY));
    let mut tasks = Vec::with_capacity(REQUESTS);
    for n in 0..REQUESTS {
        let permit = gate.clone().acquire_owned().await.expect("permit");
        let fut = post(format!("evt-{n}"));
        tasks.push(tokio::spawn(async move {
            let out = fut.await;
            drop(permit);
            out
        }));
    }
    let mut times = Vec::with_capacity(REQUESTS);
    for task in tasks {
        let (status, elapsed) = task.await.expect("task");
        assert_eq!(status, 202, "every request must be accepted; log:\n{}", child.tail());
        times.push(elapsed);
    }
    times.sort_unstable();
    let p50 = times[REQUESTS / 2];
    let p99 = times[REQUESTS * 99 / 100 - 1];
    let max = *times.last().expect("times");
    println!("ingress latency over {REQUESTS} requests at concurrency {CONCURRENCY}: p50={p50:?} p99={p99:?} max={max:?}");

    assert!(iam.introspections() >= REQUESTS, "every request must call IAM; got {}", iam.introspections());
    assert!(p99 < P99_LIMIT, "p99 {p99:?} is not under {P99_LIMIT:?} (p50={p50:?}, max={max:?})");
    server.abort();
}
