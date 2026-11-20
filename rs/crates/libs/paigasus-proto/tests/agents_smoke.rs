// SPDX-License-Identifier: Apache-2.0
use paigasus_proto::paigasus::agents::v1::OutboxMessage;

#[test]
fn outbox_message_carries_run_prn_and_text() {
    let msg = OutboxMessage {
        run_prn: "prn:pgs:agents::<org>:run/<id>".into(),
        text: "hi".into(),
    };
    assert_eq!(msg.run_prn, "prn:pgs:agents::<org>:run/<id>");
    assert_eq!(msg.text, "hi");
}
