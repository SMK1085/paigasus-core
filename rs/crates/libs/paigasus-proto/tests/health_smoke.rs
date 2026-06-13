// SPDX-License-Identifier: Apache-2.0
use paigasus_proto::paigasus::gateway::v1::CheckResponse;

#[test]
fn check_response_carries_status() {
    let resp = CheckResponse {
        status: "ok".to_string(),
    };
    assert_eq!(resp.status, "ok");
}
