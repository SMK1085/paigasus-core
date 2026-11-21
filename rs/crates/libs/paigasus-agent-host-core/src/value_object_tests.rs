// SPDX-License-Identifier: Apache-2.0

//! Tests of the domain value objects.

use jiff::Timestamp;
use uuid::Uuid;

use crate::deferral::PendingToolCallState::{self, *};
use crate::error::DomainError;
use crate::ids::{AgentId, InboxId, PrincipalId, TimerId};
use crate::inbox::Inbox;
use crate::outbox::OutboxPayload;
use crate::timer::{Timer, TimerKind};

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

#[test]
fn inbox_new_rejects_an_agent_of_another_org() {
    let r = Inbox::new(InboxId::new(u(1), u(2)), AgentId::new(u(9), u(3)), PrincipalId::from_uuid(u(4)), Timestamp::UNIX_EPOCH);
    assert_eq!(r, Err(DomainError::OrgMismatch { inbox_org: u(1), agent_org: u(9) }));
}

#[test]
fn inbox_new_takes_its_org_from_the_inbox_prn() {
    let inbox = Inbox::new(InboxId::new(u(1), u(2)), AgentId::new(u(1), u(3)), PrincipalId::from_uuid(u(4)), Timestamp::UNIX_EPOCH).unwrap();
    assert_eq!(inbox.org(), u(1));
}

#[test]
fn timer_fire_event_id_is_deterministic() {
    let timer = Timer {
        id: TimerId::from_uuid(u(1)),
        inbox: InboxId::new(u(1), u(2)),
        kind: TimerKind::FollowUp { due_at: Timestamp::UNIX_EPOCH },
        thread: None,
        fire_seq: 7,
    };
    assert_eq!(timer.fire_event_id(), "timer:00000000-0000-0000-0000-000000000001:7");
}

#[test]
fn pending_tool_call_state_allows_only_the_adr_transitions() {
    type Method = fn(PendingToolCallState) -> Result<PendingToolCallState, DomainError>;
    let methods: [(&str, Method, PendingToolCallState); 5] = [
        ("approve", PendingToolCallState::approve, Approved),
        ("deny", PendingToolCallState::deny, Denied),
        ("expire", PendingToolCallState::expire, Expired),
        ("supersede", PendingToolCallState::supersede, Superseded),
        ("resolve", PendingToolCallState::resolve, Resolved),
    ];
    for from in [Open, Approved, Denied, Expired, Superseded, Resolved] {
        for (name, method, to) in methods {
            let allowed = if name == "resolve" {
                matches!(from, Approved | Denied | Expired | Superseded)
            } else {
                from == Open
            };
            let got = method(from);
            if allowed {
                assert_eq!(got, Ok(to), "{from:?}.{name}");
            } else {
                assert_eq!(got, Err(DomainError::InvalidTransition { from, to }), "{from:?}.{name}");
            }
        }
    }
}

#[test]
fn pending_tool_call_state_serializes_in_snake_case() {
    assert_eq!(serde_json::to_string(&Open).unwrap(), "\"open\"");
    assert_eq!(serde_json::to_string(&Superseded).unwrap(), "\"superseded\"");
}

#[test]
fn outbox_payload_message_serializes_with_a_kind_tag() {
    let payload = OutboxPayload::Message { text: "hi".to_string() };
    assert_eq!(serde_json::to_string(&payload).unwrap(), r#"{"kind":"message","text":"hi"}"#);
}
