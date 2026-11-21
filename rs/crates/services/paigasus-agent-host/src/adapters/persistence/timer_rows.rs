// SPDX-License-Identifier: Apache-2.0

//! The row codec of the `timer` table (SMA-788 spec §7.3). Times cross the boundary as epoch
//! microseconds, and the civil `next_slot` as text.

use jiff::civil;
use paigasus_agent_host_core::ids::{InboxId, ThreadId, TimerId};
use paigasus_agent_host_core::ports::TzResolver;
use paigasus_agent_host_core::timer::{CronSchedule, Note, Slot, Timer, TimerKind, TimerOrigin, TimerParts, TimerStatus, TimerView};
use sea_orm::QueryResult;
use uuid::Uuid;

use super::time::from_micros;

/// Every column of `timer`, each with the prefix `t.`, so the list works for `FROM timer t` and
/// for a join. Times are epoch microseconds. `next_slot` is text.
pub(crate) const TIMER_COLUMNS: &str = "t.id, t.inbox_id, t.kind, t.origin, t.status, t.thread_id, t.cron, t.tz, \
     to_char(t.next_slot, 'YYYY-MM-DD\"T\"HH24:MI:SS') AS next_slot, t.next_slot_second, t.note, \
     (EXTRACT(EPOCH FROM t.next_fire_at) * 1000000)::bigint AS next_fire_us, \
     (EXTRACT(EPOCH FROM t.last_fired_at) * 1000000)::bigint AS last_fired_us, \
     t.fire_count, t.fire_attempts::bigint AS fire_attempts, t.failure, \
     (EXTRACT(EPOCH FROM t.created_at) * 1000000)::bigint AS created_us, \
     (EXTRACT(EPOCH FROM t.updated_at) * 1000000)::bigint AS updated_us";

/// A row that cannot be read. The reason never holds the note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowError {
    pub reason: String,
}

fn bad(reason: impl Into<String>) -> RowError {
    RowError { reason: reason.into() }
}

/// The text that a `$n::timestamp` parameter takes for a civil time.
pub(crate) fn civil_param(dt: civil::DateTime) -> String {
    dt.strftime("%Y-%m-%dT%H:%M:%S").to_string()
}

fn col<T: sea_orm::TryGetable>(row: &QueryResult, name: &str) -> Result<T, RowError> {
    row.try_get("", name).map_err(|e| bad(format!("column {name}: {e}")))
}

/// Map a row with `TIMER_COLUMNS` to a view. `org` is the org of the inbox that the caller read.
pub(crate) fn row_to_view(row: &QueryResult, org: Uuid, tz: &dyn TzResolver) -> Result<TimerView, RowError> {
    let id: Uuid = col(row, "id")?;
    let inbox: Uuid = col(row, "inbox_id")?;
    let kind: String = col(row, "kind")?;
    let origin: String = col(row, "origin")?;
    let status: String = col(row, "status")?;
    let thread: Option<Uuid> = col(row, "thread_id")?;
    let note: String = col(row, "note")?;
    let next_fire_us: i64 = col(row, "next_fire_us")?;
    let last_fired_us: Option<i64> = col(row, "last_fired_us")?;
    let fire_count: i64 = col(row, "fire_count")?;
    let created_us: i64 = col(row, "created_us")?;

    let kind = match kind.as_str() {
        "follow_up" => TimerKind::FollowUp,
        "schedule" => {
            let cron: String = col::<Option<String>>(row, "cron")?.ok_or_else(|| bad("a schedule row has no cron"))?;
            let zone: String = col::<Option<String>>(row, "tz")?.ok_or_else(|| bad("a schedule row has no zone"))?;
            let slot: String = col::<Option<String>>(row, "next_slot")?.ok_or_else(|| bad("a schedule row has no slot"))?;
            let second: bool = col::<Option<bool>>(row, "next_slot_second")?.ok_or_else(|| bad("a schedule row has no slot flag"))?;
            let schedule = CronSchedule::parse(&cron, &zone, tz).map_err(|e| bad(format!("the stored schedule is not valid: {e}")))?;
            let civil = slot.parse::<civil::DateTime>().map_err(|e| bad(format!("the stored slot is not valid: {e}")))?;
            TimerKind::Schedule {
                schedule,
                next_slot: Slot { civil, second },
            }
        }
        other => return Err(bad(format!("unknown timer kind {other:?}"))),
    };
    let timer = Timer::new(TimerParts {
        id: TimerId::from_uuid(id),
        inbox: InboxId::new(org, inbox),
        kind,
        thread: thread.map(ThreadId::from_uuid),
        note: Note::parse(&note).map_err(|_| bad("the stored note is not valid"))?,
        origin: TimerOrigin::parse(&origin).ok_or_else(|| bad(format!("unknown timer origin {origin:?}")))?,
        status: TimerStatus::parse(&status).ok_or_else(|| bad(format!("unknown timer status {status:?}")))?,
        next_fire_at: from_micros(next_fire_us).map_err(|e| bad(e.0))?,
    });
    Ok(TimerView {
        timer,
        fire_count: u64::try_from(fire_count).map_err(|_| bad("fire_count is negative"))?,
        last_fired_at: last_fired_us.map(from_micros).transpose().map_err(|e| bad(e.0))?,
        created_at: from_micros(created_us).map_err(|e| bad(e.0))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_civil_time_binds_as_iso_text() {
        assert_eq!(civil_param(civil::date(2030, 1, 2).at(9, 5, 0, 0)), "2030-01-02T09:05:00");
    }
}
