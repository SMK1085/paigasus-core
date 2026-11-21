// SPDX-License-Identifier: Apache-2.0

//! The texts that the timer tools show to the model.

use std::time::Duration;

use super::workspace::{FollowUpSet, ListedTimer, ScheduleSet, TimerRefusal};
use super::{NOTE_MAX_CHARS, NoteError, TimerKind, format_fire_at};
use crate::ids::TimerId;

/// The text of a follow-up that is set. `thread_ttl` is the life of a conversation.
#[must_use]
pub fn follow_up_set_text(set: &FollowUpSet, thread_ttl: Duration) -> String {
    let mut text = format!(
        "Follow-up {} is set for {} (UTC). When it fires, you get an event with your note. The conversation can be gone by then, so the note must hold the context that you need.",
        set.id.uuid().as_hyphenated(),
        format_fire_at(set.due),
    );
    if set.beyond_thread_ttl {
        text.push_str(&format!(
            " The due time is more than {} hours from now. This conversation may be gone when the timer fires, and only the note may remain.",
            thread_ttl.as_secs() / 3600,
        ));
    }
    text
}

/// The text of a schedule that is set.
#[must_use]
pub fn schedule_set_text(set: &ScheduleSet) -> String {
    let tz = set.schedule.tz_name();
    let mut text = format!("Schedule {} is set: \"{}\" in {tz}. The next three slots:", set.id.uuid().as_hyphenated(), set.schedule.text());
    for (local, utc) in &set.next {
        // The instant, read in the zone of the schedule, is the real local fire time. It differs
        // from the nominal slot when the slot falls in a gap of the zone.
        let actual = set.schedule.time_zone().to_datetime(*utc);
        let shown = if actual == *local {
            local.strftime("%Y-%m-%d %H:%M").to_string()
        } else if actual.date() == local.date() {
            format!("{} (fires {})", local.strftime("%Y-%m-%d %H:%M"), actual.strftime("%H:%M"))
        } else {
            format!("{} (fires {})", local.strftime("%Y-%m-%d %H:%M"), actual.strftime("%Y-%m-%d %H:%M"))
        };
        text.push_str(&format!("\n- {shown} {tz} ({} UTC)", utc.strftime("%Y-%m-%d %H:%M")));
    }
    text
}

/// The text of a list of timers.
#[must_use]
pub fn list_text(timers: &[ListedTimer]) -> String {
    if timers.is_empty() {
        return "There are no active timers.".to_string();
    }
    let lines: Vec<String> = timers.iter().map(list_line).collect();
    lines.join("\n")
}

fn list_line(listed: &ListedTimer) -> String {
    let t = &listed.timer;
    let id = t.id().uuid().as_hyphenated().to_string();
    let at = format_fire_at(t.next_fire_at());
    // A line break in the note must not start a new record: indent each continuation line.
    let note = t.note().as_str().replace("\r\n", "\n").replace('\r', "\n").replace('\n', "\n  ");
    let mut line = match t.kind() {
        TimerKind::FollowUp => format!("- {id}: follow-up, due {at} (UTC). Note: {note}"),
        TimerKind::Schedule { schedule, .. } => {
            format!("- {id}: schedule \"{}\" in {}, next {at} (UTC). Note: {note}", schedule.text(), schedule.tz_name())
        }
    };
    if listed.buffered {
        line.push_str(" (set in this run)");
    }
    line
}

/// The text of a cancelled timer. `stored` is true when the timer is in the store. The cancel of a
/// stored timer is buffered, so it takes effect only if the run commits its timer changes. The timer can fire before then.
#[must_use]
pub fn cancelled_text(id: TimerId, stored: bool) -> String {
    if stored {
        format!(
            "Timer {} is marked to cancel. The cancel takes effect only if this run commits its timer changes. The timer can still fire before then.",
            id.uuid().as_hyphenated()
        )
    } else {
        format!("Timer {} is cancelled.", id.uuid().as_hyphenated())
    }
}

/// The text of a refusal.
pub(super) fn refusal_text(r: &TimerRefusal) -> String {
    match r {
        TimerRefusal::Sealed => "The run is ending. Timers are read-only now.".to_string(),
        TimerRefusal::NotInFuture { due, now } => {
            format!(
                "Error: The due time {} (UTC) is not in the future. The time now is {} (UTC).",
                format_fire_at(*due),
                format_fire_at(*now)
            )
        }
        TimerRefusal::TooSoon { secs, min } => format!("Error: The due time is {secs} seconds from now. The minimum is {min} seconds."),
        TimerRefusal::TooLate { max, .. } => format!("Error: The due time is more than {max} days from now. The maximum is {max} days."),
        TimerRefusal::Note(NoteError::TooLong { chars }) => format!("Error: The note has {chars} characters. The maximum is {NOTE_MAX_CHARS}."),
        TimerRefusal::Note(NoteError::Nul) => "Error: The note contains a NUL character (U+0000). Remove it and try again.".to_string(),
        TimerRefusal::CountCap { after, limit } => {
            format!("Error: This inbox would have {after} active timers. The maximum is {limit}. Cancel a timer first.")
        }
        TimerRefusal::Budget { after, limit } => {
            format!("Error: This inbox would have created {after} timers in the last 24 hours. The maximum is {limit}. Try again later.")
        }
        TimerRefusal::UnknownTimer { id } => format!("Error: No timer {id} exists in this inbox."),
        TimerRefusal::NotActive { id, status } => {
            format!("Error: Timer {} is {}, so it cannot be cancelled.", id.uuid().as_hyphenated(), status.as_str())
        }
        TimerRefusal::Cron { cron, error } => format!("Error: The cron expression \"{cron}\" is not accepted: {error}."),
        TimerRefusal::UnknownZone { name } => format!("Error: \"{name}\" is not a known IANA time zone name, for example Europe/Berlin."),
        TimerRefusal::IntervalTooShort { cron, gap, min } => format!("Error: Two slots of \"{cron}\" are {gap} seconds apart. The minimum is {min} seconds."),
        TimerRefusal::NoSlot { cron, days } => format!("Error: \"{cron}\" has no slot in the next {days} days."),
        TimerRefusal::Arguments { tool, detail } => format!("Error: {detail}. {} takes: {}.", tool.as_str(), tool.argument_summary()),
        TimerRefusal::Store { .. } => "The timer store is not available now. Try again later.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;
    use jiff::civil;

    use super::*;
    use crate::ids::InboxId;
    use crate::timer::{BundledTzResolver, CronSchedule, Note, Timer, TimerOrigin, TimerParts, TimerStatus};

    fn timer_with_note(note: &str) -> Timer {
        Timer::new(TimerParts {
            id: TimerId::from_uuid(uuid::Uuid::from_u128(0x1000)),
            inbox: InboxId::new(uuid::Uuid::from_u128(1), uuid::Uuid::from_u128(2)),
            kind: TimerKind::FollowUp,
            thread: None,
            note: Note::parse(note).unwrap(),
            origin: TimerOrigin::Agent,
            status: TimerStatus::Active,
            next_fire_at: "2026-06-01T02:00:00Z".parse().unwrap(),
        })
    }

    #[test]
    fn a_gap_slot_shows_the_real_local_fire_time() {
        let schedule = CronSchedule::parse("30 2 * * *", "Europe/Berlin", &BundledTzResolver::new()).unwrap();
        let utc: Timestamp = "2026-03-29T01:00:00Z".parse().unwrap();
        let set = ScheduleSet {
            id: TimerId::from_uuid(uuid::Uuid::from_u128(0x2000)),
            schedule,
            next: vec![
                (civil::date(2026, 3, 29).at(2, 30, 0, 0), utc),
                (civil::date(2026, 3, 30).at(2, 30, 0, 0), "2026-03-30T00:30:00Z".parse().unwrap()),
            ],
        };
        let text = schedule_set_text(&set);
        assert!(text.contains("\n- 2026-03-29 02:30 (fires 03:00) Europe/Berlin (2026-03-29 01:00 UTC)"), "{text}");
        assert!(text.contains("\n- 2026-03-30 02:30 Europe/Berlin (2026-03-30 00:30 UTC)"), "{text}");
    }

    #[test]
    fn a_multiline_note_stays_inside_its_record() {
        let listed = ListedTimer {
            timer: timer_with_note("line one\nline two\r\nline three"),
            buffered: false,
        };
        let text = list_text(&[listed]);
        assert!(text.ends_with("Note: line one\n  line two\n  line three"), "{text}");
        assert!(text.lines().skip(1).all(|l| l.starts_with("  ")), "{text}");
    }

    #[test]
    fn the_stored_cancel_text_names_the_commit() {
        let text = cancelled_text(TimerId::from_uuid(uuid::Uuid::from_u128(0x1000)), true);
        assert!(text.contains("takes effect only if this run commits its timer changes"), "{text}");
        assert!(text.contains("The timer can still fire before then."), "{text}");
    }
}
