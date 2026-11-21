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

#[test]
fn timer_fired_json_names() {
    use paigasus_proto::paigasus::agents::v1::FILE_DESCRIPTOR_SET;
    use prost::Message;
    use prost_types::FileDescriptorSet;

    let set = FileDescriptorSet::decode(FILE_DESCRIPTOR_SET).expect("the descriptor set decodes");
    let file = set.file.iter().find(|f| f.package() == "paigasus.agents.v1").expect("the agents file is in the set");
    let message = file.message_type.iter().find(|m| m.name() == "TimerFired").expect("TimerFired is in the file");
    let names: Vec<&str> = message.field.iter().map(|f| f.json_name()).collect();
    assert_eq!(names, ["timerId", "kind", "origin", "fireAt", "note", "cron", "tz"]);

    let values = |name: &str| -> Vec<(String, i32)> {
        file.enum_type
            .iter()
            .find(|e| e.name() == name)
            .unwrap_or_else(|| panic!("{name} is in the file"))
            .value
            .iter()
            .map(|v| (v.name().to_string(), v.number()))
            .collect()
    };
    assert_eq!(
        values("TimerKind"),
        [
            ("TIMER_KIND_UNSPECIFIED".to_string(), 0),
            ("TIMER_KIND_FOLLOW_UP".to_string(), 1),
            ("TIMER_KIND_SCHEDULE".to_string(), 2),
        ]
    );
    assert_eq!(
        values("TimerOrigin"),
        [
            ("TIMER_ORIGIN_UNSPECIFIED".to_string(), 0),
            ("TIMER_ORIGIN_AGENT".to_string(), 1),
            ("TIMER_ORIGIN_SYSTEM".to_string(), 2),
        ]
    );
}
