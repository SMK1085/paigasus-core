// SPDX-License-Identifier: Apache-2.0

//! The event that one firing of a timer makes (spec §11.2).
//!
//! The scheduler builds one event per firing. The `source` depends on the origin of the timer,
//! and only the scheduler writes these values. The envelope has no `actorprn` and no `subject`.

use jiff::Timestamp;
use paigasus_agent_host_core::ids::EventId;
use paigasus_agent_host_core::inbox::NewInboxEvent;
use paigasus_agent_host_core::timer::{Timer, TimerKind, TimerOrigin, format_fire_at};
use paigasus_cloudevents::{CloudEvent, ExtensionValue, SchemaError};
use serde_json::{Value, json};

/// The CloudEvents `type` of a timer event.
pub const TIMER_FIRED_TYPE: &str = "agents.timer.fired";
/// The `source` of the event of a timer that the agent made.
pub const SOURCE_AGENT: &str = "urn:paigasus:agents:timer:agent";
/// The `source` of the event of a timer that the system made.
pub const SOURCE_SYSTEM: &str = "urn:paigasus:agents:timer:system";

/// The `source` of the event of a timer with this origin.
#[must_use]
pub fn source_for(origin: TimerOrigin) -> &'static str {
    match origin {
        TimerOrigin::Agent => SOURCE_AGENT,
        TimerOrigin::System => SOURCE_SYSTEM,
    }
}

/// The `data` of the event: the proto3 JSON of `paigasus.agents.v1.TimerFired`.
/// `cron` and `tz` are only in the data of a schedule.
#[must_use]
pub fn timer_fired_data(timer: &Timer, fire_at: Timestamp) -> Value {
    let origin = match timer.origin() {
        TimerOrigin::Agent => "TIMER_ORIGIN_AGENT",
        TimerOrigin::System => "TIMER_ORIGIN_SYSTEM",
    };
    let mut data = json!({
        "timerId": timer.id().uuid().hyphenated().to_string(),
        "origin": origin,
        "fireAt": format_fire_at(fire_at),
        "note": timer.note().as_str(),
    });
    match timer.kind() {
        TimerKind::FollowUp => {
            data["kind"] = json!("TIMER_KIND_FOLLOW_UP");
        }
        TimerKind::Schedule { schedule, .. } => {
            data["kind"] = json!("TIMER_KIND_SCHEDULE");
            data["cron"] = json!(schedule.text());
            data["tz"] = json!(schedule.tz_name());
        }
    }
    data
}

/// Build the inbox event of one firing. `now` is the time of the actual firing. `fire_at` is the
/// scheduled time and gives the event identity, so the same firing always has the same `ce_id`.
///
/// # Errors
/// Returns an error when `paigasus-cloudevents` refuses an attribute. The values of a valid timer
/// never cause this.
pub fn build_fire_event(timer: &Timer, fire_at: Timestamp, now: Timestamp, event_id: EventId) -> Result<NewInboxEvent, SchemaError> {
    let mut event = CloudEvent::new_text_id(&timer.fire_event_id(fire_at), source_for(timer.origin()), TIMER_FIRED_TYPE, now)?
        .with_datacontenttype("application/json")?
        .with_data(timer_fired_data(timer, fire_at));
    event.set_extension("schemaversion", ExtensionValue::Integer(1))?;
    if let Some(thread) = timer.thread() {
        event.set_extension("threadid", ExtensionValue::String(thread.uuid().hyphenated().to_string()))?;
    }
    Ok(NewInboxEvent {
        id: event_id,
        inbox: timer.inbox().clone(),
        source: event.source().to_owned(),
        ce_id: event.id().to_owned(),
        ce_type: event.ty().to_owned(),
        thread: timer.thread(),
        envelope: event.to_json(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::Timestamp;
    use paigasus_agent_host_core::ids::{EventId, InboxId, ThreadId, TimerId};
    use paigasus_agent_host_core::timer::{BundledTzResolver, CronSchedule, Note, Slot, Timer, TimerKind, TimerOrigin, TimerParts, TimerStatus};
    use paigasus_proto::paigasus::agents::v1::FILE_DESCRIPTOR_SET;
    use prost::Message;
    use prost_types::FileDescriptorSet;
    use serde_json::json;
    use uuid::Uuid;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn follow_up(origin: TimerOrigin) -> Timer {
        Timer::new(TimerParts {
            id: TimerId::from_uuid(Uuid::from_u128(1)),
            inbox: InboxId::new(Uuid::from_u128(2), Uuid::from_u128(3)),
            kind: TimerKind::FollowUp,
            thread: Some(ThreadId::from_uuid(Uuid::from_u128(4))),
            note: Note::parse("check the reply").unwrap(),
            origin,
            status: TimerStatus::Active,
            next_fire_at: ts("2026-03-29T01:00:00Z"),
        })
    }

    fn schedule() -> Timer {
        let schedule = CronSchedule::parse("0 9 * * *", "Europe/Berlin", &BundledTzResolver::new()).unwrap();
        let next_slot = Slot {
            civil: "2026-03-29T09:00:00".parse().unwrap(),
            second: false,
        };
        Timer::new(TimerParts {
            id: TimerId::from_uuid(Uuid::from_u128(1)),
            inbox: InboxId::new(Uuid::from_u128(2), Uuid::from_u128(3)),
            kind: TimerKind::Schedule { schedule, next_slot },
            thread: None,
            note: Note::parse("check the reply").unwrap(),
            origin: TimerOrigin::System,
            status: TimerStatus::Active,
            next_fire_at: ts("2026-03-29T07:00:00Z"),
        })
    }

    const FIRE_AT: &str = "2026-03-29T01:00:00Z";

    #[test]
    fn sources_and_type_are_pinned() {
        assert_eq!(TIMER_FIRED_TYPE, "agents.timer.fired");
        assert_eq!(SOURCE_AGENT, "urn:paigasus:agents:timer:agent");
        assert_eq!(SOURCE_SYSTEM, "urn:paigasus:agents:timer:system");
        assert_eq!(source_for(TimerOrigin::Agent), "urn:paigasus:agents:timer:agent");
        assert_eq!(source_for(TimerOrigin::System), "urn:paigasus:agents:timer:system");
    }

    #[test]
    fn the_event_id_text() {
        let event = build_fire_event(&follow_up(TimerOrigin::Agent), ts(FIRE_AT), ts(FIRE_AT), EventId::from_uuid(Uuid::from_u128(9))).unwrap();
        let text = "timer:00000000-0000-0000-0000-000000000001:2026-03-29T01:00:00Z";
        assert_eq!(event.ce_id, text);
        assert_eq!(text.len(), 63);
        assert_eq!(event.envelope["id"], text);
    }

    #[test]
    fn the_data_is_the_golden_json() {
        assert_eq!(
            timer_fired_data(&follow_up(TimerOrigin::Agent), ts(FIRE_AT)),
            json!({
                "timerId": "00000000-0000-0000-0000-000000000001",
                "kind": "TIMER_KIND_FOLLOW_UP",
                "origin": "TIMER_ORIGIN_AGENT",
                "fireAt": "2026-03-29T01:00:00Z",
                "note": "check the reply",
            })
        );
        assert_eq!(
            timer_fired_data(&schedule(), ts(FIRE_AT)),
            json!({
                "timerId": "00000000-0000-0000-0000-000000000001",
                "kind": "TIMER_KIND_SCHEDULE",
                "origin": "TIMER_ORIGIN_SYSTEM",
                "fireAt": "2026-03-29T01:00:00Z",
                "note": "check the reply",
                "cron": "0 9 * * *",
                "tz": "Europe/Berlin",
            })
        );
    }

    #[test]
    fn the_data_matches_the_timer_fired_descriptor() {
        let set = FileDescriptorSet::decode(FILE_DESCRIPTOR_SET).unwrap();
        let file = set.file.iter().find(|f| f.package() == "paigasus.agents.v1").unwrap();
        let message = file.message_type.iter().find(|m| m.name() == "TimerFired").unwrap();
        let enum_values = |name: &str| -> Vec<String> { file.enum_type.iter().find(|e| e.name() == name).unwrap().value.iter().map(|v| v.name().to_string()).collect() };
        for timer in [follow_up(TimerOrigin::Agent), follow_up(TimerOrigin::System), schedule()] {
            let data = timer_fired_data(&timer, ts(FIRE_AT));
            let object = data.as_object().unwrap();
            for key in object.keys() {
                assert!(message.field.iter().any(|f| f.json_name() == key), "`{key}` is not a field of TimerFired");
            }
            assert!(enum_values("TimerKind").contains(&object["kind"].as_str().unwrap().to_string()));
            assert!(enum_values("TimerOrigin").contains(&object["origin"].as_str().unwrap().to_string()));
            assert_eq!(object["fireAt"].as_str().unwrap().parse::<Timestamp>().unwrap(), ts(FIRE_AT));
        }
    }

    #[test]
    fn the_envelope_round_trips() {
        let now = ts("2026-03-29T01:00:03Z");
        let agent = build_fire_event(&follow_up(TimerOrigin::Agent), ts(FIRE_AT), now, EventId::from_uuid(Uuid::from_u128(9))).unwrap();
        assert_eq!(agent.id, EventId::from_uuid(Uuid::from_u128(9)));
        assert_eq!(agent.source, SOURCE_AGENT);
        assert_eq!(agent.ce_type, TIMER_FIRED_TYPE);
        assert_eq!(agent.thread, Some(ThreadId::from_uuid(Uuid::from_u128(4))));
        let parsed = paigasus_cloudevents::parse(&serde_json::to_vec(&agent.envelope).unwrap()).unwrap();
        assert_eq!(parsed.id(), "timer:00000000-0000-0000-0000-000000000001:2026-03-29T01:00:00Z");
        assert_eq!(parsed.source(), SOURCE_AGENT);
        assert_eq!(parsed.ty(), TIMER_FIRED_TYPE);
        assert_eq!(parsed.time(), Some(now));
        assert_eq!(parsed.schemaversion(), Some(1));
        assert_eq!(parsed.datacontenttype(), Some("application/json"));
        assert_eq!(parsed.actorprn(), None);
        assert_eq!(parsed.subject(), None);
        assert_eq!(parsed.threadid(), Some("00000000-0000-0000-0000-000000000004"));
        assert_eq!(parsed.data(), Some(&timer_fired_data(&follow_up(TimerOrigin::Agent), ts(FIRE_AT))));

        let system = build_fire_event(&schedule(), ts(FIRE_AT), now, EventId::from_uuid(Uuid::from_u128(10))).unwrap();
        assert_eq!(system.source, SOURCE_SYSTEM);
        assert_eq!(system.thread, None);
        let parsed = paigasus_cloudevents::parse(&system.envelope.to_string().into_bytes()).unwrap();
        assert_eq!(parsed.threadid(), None);
        assert_eq!(parsed.source(), SOURCE_SYSTEM);
    }
}
