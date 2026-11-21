// SPDX-License-Identifier: Apache-2.0

//! The entry point of the timer tools. It reads the JSON arguments of the model, calls the
//! workspace and makes the reply text.

use jiff::Timestamp;
use serde_json::Value;

use super::text::{cancelled_text, follow_up_set_text, list_text, schedule_set_text};
use super::workspace::{FollowUpDue, TimerRefusal, TimerWorkspace};

/// The name of a timer tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerToolName {
    /// Set a follow-up.
    ScheduleFollowUp,
    /// Set a schedule.
    CreateSchedule,
    /// List the active timers.
    ListTimers,
    /// Cancel a timer.
    CancelTimer,
}

impl TimerToolName {
    /// Every timer tool.
    pub const ALL: [TimerToolName; 4] = [TimerToolName::ScheduleFollowUp, TimerToolName::CreateSchedule, TimerToolName::ListTimers, TimerToolName::CancelTimer];

    /// The name that the model uses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TimerToolName::ScheduleFollowUp => "schedule_follow_up",
            TimerToolName::CreateSchedule => "create_schedule",
            TimerToolName::ListTimers => "list_timers",
            TimerToolName::CancelTimer => "cancel_timer",
        }
    }

    pub(super) fn argument_summary(self) -> &'static str {
        match self {
            TimerToolName::ScheduleFollowUp => "at (an RFC 3339 time with an offset) or in_seconds (an integer), but not both, note (text), and new_thread (true or false, optional)",
            TimerToolName::CreateSchedule => "cron (text), tz (an IANA time zone name) and note (text)",
            TimerToolName::ListTimers => "no arguments",
            TimerToolName::CancelTimer => "timer_id (text)",
        }
    }
}

/// The reply of a timer tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerToolReply {
    /// The text for the model.
    pub text: String,
    /// True when the call was refused.
    pub refused: bool,
    /// The cause of a store failure, for the log. It is not for the model.
    pub cause: Option<String>,
}

const AT_DETAIL: &str = "at must be an RFC 3339 time with an offset, for example 2026-11-22T09:00:00+01:00";

/// Run one timer tool. A refusal gives a reply with `refused` set, never a panic.
pub async fn run_tool(ws: &mut TimerWorkspace, tool: TimerToolName, args: Value) -> TimerToolReply {
    match run(ws, tool, &args).await {
        Ok(text) => TimerToolReply { text, refused: false, cause: None },
        Err(e) => TimerToolReply {
            text: e.to_string(),
            refused: true,
            cause: e.cause().map(str::to_string),
        },
    }
}

async fn run(ws: &mut TimerWorkspace, tool: TimerToolName, args: &Value) -> Result<String, TimerRefusal> {
    let bad = |detail: &str| TimerRefusal::Arguments { tool, detail: detail.to_string() };
    let text_arg = |key: &str| -> Result<&str, TimerRefusal> {
        match args.get(key) {
            Some(Value::String(s)) => Ok(s.as_str()),
            Some(_) => Err(bad(&format!("{key} must be a text"))),
            None => Err(bad(&format!("{key} is missing"))),
        }
    };
    if !args.is_object() {
        return Err(bad("the arguments must be a JSON object"));
    }
    match tool {
        TimerToolName::ScheduleFollowUp => {
            let due = match (args.get("at"), args.get("in_seconds")) {
                (Some(_), Some(_)) => return Err(bad("give either at or in_seconds, not both")),
                (None, None) => return Err(bad("give either at or in_seconds")),
                (Some(Value::String(s)), None) => FollowUpDue::At(s.parse::<Timestamp>().map_err(|_| bad(AT_DETAIL))?),
                (Some(_), None) => return Err(bad(AT_DETAIL)),
                (None, Some(v)) => FollowUpDue::InSeconds(v.as_i64().ok_or_else(|| bad("in_seconds must be an integer"))?),
            };
            let note = text_arg("note")?;
            let new_thread = match args.get("new_thread") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => return Err(bad("new_thread must be true or false")),
            };
            let set = ws.schedule_follow_up(due, note, new_thread).await?;
            Ok(follow_up_set_text(&set, ws.thread_ttl()))
        }
        TimerToolName::CreateSchedule => {
            let (cron, tz, note) = (text_arg("cron")?, text_arg("tz")?, text_arg("note")?);
            Ok(schedule_set_text(&ws.create_schedule(cron, tz, note).await?))
        }
        TimerToolName::ListTimers => Ok(list_text(&ws.list().await?)),
        TimerToolName::CancelTimer => {
            let id = text_arg("timer_id")?;
            Ok(cancelled_text(ws.cancel(id).await?))
        }
    }
}
