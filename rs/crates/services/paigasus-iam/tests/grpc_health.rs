// SPDX-License-Identifier: Apache-2.0

//! gRPC health smoke test — the server reports SERVING for the overall service.

use std::time::Duration;
use tokio::net::TcpListener;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::{HealthCheckRequest, health_client::HealthClient};

#[tokio::test]
async fn grpc_health_reports_serving() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let (reporter, health) = paigasus_iam::adapters::grpc::health_service().await;
    let _ = &reporter; // keep the reporter alive for the server's lifetime
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .timeout(Duration::from_secs(5))
            .add_service(health)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    // tonic-health's vendored client codegen omits the `transport`-gated `connect` helper
    // (it's built with `default-features = false, features = ["codegen"]` on tonic), so
    // dial via `Endpoint`/`Channel` directly and hand the channel to `HealthClient::new`.
    let channel = tonic::transport::Endpoint::new(format!("http://{addr}")).unwrap().connect().await.unwrap();
    let mut client = HealthClient::new(channel);
    let resp = client.check(HealthCheckRequest { service: String::new() }).await.unwrap().into_inner();
    assert_eq!(resp.status, ServingStatus::Serving as i32);

    server.abort();
}
