// SPDX-License-Identifier: Apache-2.0

//! Tests of the domain value objects.

use jiff::Timestamp;
use uuid::Uuid;

use crate::deferral::PendingToolCallState::{self, *};
use crate::error::DomainError;
use crate::ids::{AgentId, InboxId, PrincipalId, TimerId};
use crate::inbox::Inbox;
use crate::outbox::OutboxPayload;
use crate::ports::{IdSource, RunClock, TimerRepository, TzResolver};
use crate::timer::fake::{FakeClock, InMemoryTimerRepository, MapTzResolver, SeqIds};
use crate::timer::{ApplyError, Note, NoteError, Timer, TimerCaps, TimerChanges, TimerKind, TimerOrigin, TimerParts, TimerStatus, TimerView};

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

fn ts(raw: &str) -> Timestamp {
    raw.parse().unwrap()
}

fn follow_up(id: u128, inbox: &InboxId, origin: TimerOrigin, next: Timestamp) -> Timer {
    Timer::new(TimerParts {
        id: TimerId::from_uuid(u(id)),
        inbox: inbox.clone(),
        kind: TimerKind::FollowUp,
        thread: None,
        note: Note::parse("n").unwrap(),
        origin,
        status: TimerStatus::Active,
        next_fire_at: next,
    })
}

fn view(timer: Timer, created_at: Timestamp) -> TimerView {
    TimerView {
        timer,
        fire_count: 0,
        last_fired_at: None,
        created_at,
    }
}

#[test]
fn timer_fire_event_id_is_deterministic() {
    let inbox = InboxId::new(u(1), u(2));
    let timer = follow_up(1, &inbox, TimerOrigin::Agent, ts("2026-03-29T01:00:00Z"));
    let id = timer.fire_event_id(ts("2026-03-29T01:00:00Z"));
    assert_eq!(id, "timer:00000000-0000-0000-0000-000000000001:2026-03-29T01:00:00Z");
    assert_eq!(id.len(), 63);
}

#[test]
fn next_fire_at_is_rounded_up() {
    let inbox = InboxId::new(u(1), u(2));
    let t = follow_up(1, &inbox, TimerOrigin::Agent, ts("2026-03-29T01:00:00.000001Z"));
    assert_eq!(t.next_fire_at(), ts("2026-03-29T01:00:01Z"));
    let t = follow_up(1, &inbox, TimerOrigin::Agent, ts("2026-03-29T01:00:00Z"));
    assert_eq!(t.next_fire_at(), ts("2026-03-29T01:00:00Z"));
}

#[test]
fn note_rules() {
    assert!(Note::parse(&"\u{e9}".repeat(1000)).is_ok());
    assert_eq!(Note::parse(&"\u{e9}".repeat(1001)), Err(NoteError::TooLong { chars: 1001 }));
    assert_eq!(Note::parse("a\0b"), Err(NoteError::Nul));
    assert_eq!(Note::parse("").unwrap().as_str(), "");
}

#[test]
fn status_and_origin_round_trip() {
    for s in [TimerStatus::Active, TimerStatus::Done, TimerStatus::Cancelled, TimerStatus::Failed] {
        assert_eq!(TimerStatus::parse(s.as_str()), Some(s));
    }
    for o in [TimerOrigin::Agent, TimerOrigin::System] {
        assert_eq!(TimerOrigin::parse(o.as_str()), Some(o));
    }
    assert_eq!(TimerStatus::parse("x"), None);
    assert_eq!(TimerOrigin::parse("x"), None);
}

#[test]
fn kind_names() {
    assert_eq!(TimerKind::FollowUp.as_str(), "follow_up");
}

#[test]
fn caps_defaults() {
    let c = TimerCaps::default();
    assert_eq!(
        (
            c.min_schedule_interval_secs,
            c.max_active_per_inbox,
            c.max_horizon_days,
            c.min_follow_up_delay_secs,
            c.max_agent_timers_created_per_day
        ),
        (900, 50, 366, 60, 100)
    );
}

#[test]
fn timer_changes_is_empty() {
    let inbox = InboxId::new(u(1), u(2));
    assert!(TimerChanges::default().is_empty());
    let with_insert = TimerChanges {
        inserts: vec![follow_up(1, &inbox, TimerOrigin::Agent, Timestamp::UNIX_EPOCH)],
        cancels: vec![],
    };
    assert!(!with_insert.is_empty());
    let with_cancel = TimerChanges {
        inserts: vec![],
        cancels: vec![(TimerId::from_uuid(u(1)), TimerOrigin::Agent)],
    };
    assert!(!with_cancel.is_empty());
}

#[tokio::test]
async fn the_fake_repository_scopes_by_inbox() {
    let x = InboxId::new(u(1), u(2));
    let y = InboxId::new(u(1), u(3));
    let repo = InMemoryTimerRepository::default();
    let t0 = ts("2026-03-29T00:00:00Z");
    repo.seed(view(follow_up(10, &x, TimerOrigin::Agent, ts("2026-03-30T00:00:00Z")), ts("2026-03-29T10:00:00Z")));
    repo.seed(view(follow_up(11, &x, TimerOrigin::Agent, ts("2026-03-29T12:00:00Z")), ts("2026-03-29T11:00:00Z")));
    repo.seed(view(follow_up(12, &x, TimerOrigin::System, ts("2026-03-29T12:00:00Z")), ts("2026-03-29T11:00:00Z")));
    repo.seed(view(follow_up(13, &y, TimerOrigin::Agent, ts("2026-03-29T12:00:00Z")), ts("2026-03-29T11:00:00Z")));
    let listed = repo.list_active(&x, TimerOrigin::Agent).await.unwrap();
    let ids: Vec<_> = listed.iter().map(|v| v.timer.id()).collect();
    assert_eq!(ids, [TimerId::from_uuid(u(11)), TimerId::from_uuid(u(10))]);
    assert_eq!(repo.get(&y, TimerId::from_uuid(u(10))).await.unwrap(), None);
    assert!(repo.get(&x, TimerId::from_uuid(u(10))).await.unwrap().is_some());
    assert_eq!(repo.count_agent_created_since(&x, t0).await.unwrap(), 2);
    assert_eq!(repo.count_agent_created_since(&x, ts("2026-03-29T10:00:00Z")).await.unwrap(), 1);
    assert_eq!(repo.active_agent_ids(&x).await.unwrap().len(), 2);
    repo.set_status(TimerId::from_uuid(u(10)), TimerStatus::Cancelled);
    assert_eq!(repo.active_agent_ids(&x).await.unwrap(), [TimerId::from_uuid(u(11))]);
    assert_eq!(repo.get(&x, TimerId::from_uuid(u(10))).await.unwrap().unwrap().timer.status(), TimerStatus::Cancelled);
}

#[test]
fn apply_error_display_pins_the_text() {
    let id = TimerId::from_uuid(u(1));
    let cases = [
        (ApplyError::InboxMismatch { id }, format!("timer {id:?} belongs to another inbox")),
        (ApplyError::InvalidInsert { id }, format!("timer {id:?} is not a valid insert")),
        (
            ApplyError::CapExceeded { before: 49, after: 51, limit: 50 },
            "active timer limit exceeded: 49 before, 51 after, limit 50".to_string(),
        ),
        (ApplyError::BudgetExceeded { created: 101, limit: 100 }, "daily timer budget exceeded: 101 made, limit 100".to_string()),
        (ApplyError::Store("db down".to_string()), "store error: db down".to_string()),
    ];
    for (err, text) in cases {
        assert_eq!(err.to_string(), text);
    }
}

#[test]
fn the_fakes_clock_ids_and_zones() {
    let clock = FakeClock::new(ts("2026-03-29T00:00:00Z"));
    clock.advance(jiff::SignedDuration::from_secs(90));
    assert_eq!(clock.now(), ts("2026-03-29T00:01:30Z"));
    let ids = SeqIds::default();
    assert_eq!(ids.new_uuid(), u(0x1000));
    assert_eq!(ids.new_uuid(), u(0x1001));
    let tz = MapTzResolver::new(&[("UTC", jiff::tz::TimeZone::UTC)]);
    assert!(tz.resolve("UTC").is_ok());
    assert!(tz.resolve("Europe/Berlin").is_err());
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
