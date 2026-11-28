// SPDX-License-Identifier: Apache-2.0

//! `PgRunSession`: the Helikon `Session` of one run (spec §6.6).
//!
//! The session reads the committed events of the thread from `session_event` and keeps the
//! events of this run in a buffer. `append` only buffers and never fails (spec §5): the run commit
//! takes the buffer with `take_buffer` and writes it with `write_session` in its transaction.

use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use paigasus_agent_host_core::ids::ThreadId;
use paigasus_agent_host_core::run::SessionRow;
use paigasus_helikon::core::{ConversationSnapshot, SequenceId, Session, SessionError, SessionEvent, project};
use sea_orm::DatabaseConnection;

use crate::adapters::persistence::session_store::read_session;

/// The session of one run: the committed events of the thread, then the buffer of this run.
pub struct PgRunSession {
    db: DatabaseConnection,
    thread: ThreadId,
    /// A fresh thread has no committed events, so the read is skipped.
    fresh: bool,
    buffer: Mutex<Vec<SessionEvent>>,
}

impl PgRunSession {
    pub fn new(db: DatabaseConnection, thread: ThreadId, fresh_thread: bool) -> Self {
        Self {
            db,
            thread,
            fresh: fresh_thread,
            buffer: Mutex::new(Vec::new()),
        }
    }

    /// Give the buffered events to the caller and leave the buffer empty.
    pub fn take_buffer(&self) -> Vec<SessionEvent> {
        std::mem::take(&mut *self.lock())
    }

    /// A panic while the lock is held cannot leave the vector half-changed (`extend_from_slice`
    /// and `take` do not panic in the middle), so a poisoned lock is still usable.
    fn lock(&self) -> MutexGuard<'_, Vec<SessionEvent>> {
        self.buffer.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn committed(&self) -> Result<Vec<SessionEvent>, SessionError> {
        if self.fresh {
            return Ok(Vec::new());
        }
        let payloads = read_session(&self.db, self.thread.uuid()).await.map_err(SessionError::backend)?;
        payloads.into_iter().map(|p| serde_json::from_value::<SessionEvent>(p).map_err(SessionError::backend)).collect()
    }
}

#[async_trait]
impl Session for PgRunSession {
    async fn append(&self, events: &[SessionEvent]) -> Result<(), SessionError> {
        self.lock().extend_from_slice(events);
        Ok(())
    }

    /// The committed events, then the buffer. Positions start at 0 over the combined list, and
    /// `since` is exclusive.
    async fn events(&self, since: Option<SequenceId>) -> Result<Vec<SessionEvent>, SessionError> {
        let mut all = self.committed().await?;
        all.extend(self.lock().iter().cloned());
        let skip = match since {
            None => 0,
            Some(SequenceId(n)) => usize::try_from(n).map_or(usize::MAX, |n| n.saturating_add(1)),
        };
        Ok(all.into_iter().skip(skip).collect())
    }

    async fn snapshot(&self) -> Result<ConversationSnapshot, SessionError> {
        Ok(project(&self.events(None).await?))
    }
}

/// The stored form of an event: `kind` is the serde tag, `payload` is the whole event.
pub fn to_session_row(event: &SessionEvent) -> Result<SessionRow, serde_json::Error> {
    Ok(SessionRow {
        kind: event.kind().to_owned(),
        payload: serde_json::to_value(event)?,
    })
}

#[cfg(test)]
mod tests {
    use paigasus_helikon::core::SessionEvent;

    use super::to_session_row;

    /// A row that Helikon core 0.6.0 wrote (SMA-851 Task 1 Step 1). The new core must read it
    /// unchanged.
    const CORE_0_6_ROW: &str = r#"{"type":"user_message","content":[{"type":"text","text":"hi"}],"ts":"2025-11-24T16:00:00.123456789Z"}"#;

    #[test]
    fn a_core_0_6_row_with_a_fractional_ts_reads_with_the_new_core() {
        let event: SessionEvent = serde_json::from_str(CORE_0_6_ROW).expect("the new core reads the 0.6 row");
        let SessionEvent::UserMessage { ts, .. } = &event else { panic!("{event:?}") };
        assert_eq!(ts.as_nanosecond(), 1_764_000_000_123_456_789);
        let back = to_session_row(&event).expect("row").payload;
        assert_eq!(back, serde_json::from_str::<serde_json::Value>(CORE_0_6_ROW).expect("json"), "the new core writes the same ts text");
    }
}
