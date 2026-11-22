// SPDX-License-Identifier: Apache-2.0

//! The `session_event` store (spec §6.4 and §6.6).
//!
//! `write_session` is the only function that inserts into `session_event`. The run commit calls
//! it inside its transaction. It takes no lock: the primary key `(thread_id, sequence)` makes a
//! second writer of the same number fail, and its transaction rolls back.

use paigasus_agent_host_core::run::{SessionRow, replace_nul};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseTransaction, DbErr, Statement, Value};
use serde_json::Map;
use uuid::Uuid;

const NEXT_SQL: &str = "SELECT COALESCE(max(sequence), -1) + 1 AS next FROM session_event WHERE thread_id = $1";

const INSERT_SQL: &str = "INSERT INTO session_event (inbox_id, thread_id, sequence, run_id, kind, payload) VALUES ($1, $2, $3, $4, $5, $6::jsonb)";

/// The payload crosses the boundary as JSON text, so an integer keeps all its digits.
const READ_SQL: &str = "SELECT payload::text AS payload FROM session_event WHERE thread_id = $1 ORDER BY sequence";

/// The rows as they are stored (spec §6.4 and §6.6).
///
/// In a `user_message` payload, every text part that starts with `<memory-{nonce}` is removed:
/// the run-start memory text of this run must not go into the session. The parts of every other
/// payload type stay. Then every U+0000 in a JSON string (a key or a value, at any depth) becomes
/// U+FFFD, because Postgres cannot store U+0000 in `jsonb`.
pub fn prepare_rows(rows: &[SessionRow], nonce: &str) -> Vec<SessionRow> {
    let prefix = format!("<memory-{nonce}");
    rows.iter()
        .map(|row| {
            let mut payload = row.payload.clone();
            if payload.get("type").and_then(serde_json::Value::as_str) == Some("user_message")
                && let Some(content) = payload.get_mut("content").and_then(serde_json::Value::as_array_mut)
            {
                content.retain(|part| !is_memory_part(part, &prefix));
            }
            SessionRow {
                kind: replace_nul(&row.kind),
                payload: without_nul(payload),
            }
        })
        .collect()
}

fn is_memory_part(part: &serde_json::Value, prefix: &str) -> bool {
    part.get("type").and_then(serde_json::Value::as_str) == Some("text") && part.get("text").and_then(serde_json::Value::as_str).is_some_and(|t| t.starts_with(prefix))
}

fn without_nul(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => serde_json::Value::String(replace_nul(&s)),
        serde_json::Value::Array(items) => serde_json::Value::Array(items.into_iter().map(without_nul).collect()),
        serde_json::Value::Object(map) => serde_json::Value::Object(map.into_iter().map(|(k, v)| (replace_nul(&k), without_nul(v))).collect::<Map<_, _>>()),
        other => other,
    }
}

/// Append the rows of a run to the session of `thread`, numbered from the last stored sequence
/// plus one, in order. Returns the number of rows written.
pub async fn write_session(txn: &DatabaseTransaction, inbox: Uuid, thread: Uuid, run: Uuid, nonce: &str, rows: &[SessionRow]) -> Result<usize, DbErr> {
    let prepared = prepare_rows(rows, nonce);
    let next_row = txn
        .query_one_raw(Statement::from_sql_and_values(DatabaseBackend::Postgres, NEXT_SQL, [thread.into()]))
        .await?
        .ok_or_else(|| DbErr::Custom("the next sequence query returned no row".to_string()))?;
    let next: i64 = next_row.try_get("", "next")?;
    for (i, row) in prepared.iter().enumerate() {
        let offset = i64::try_from(i).map_err(|_| DbErr::Custom("too many session rows".to_string()))?;
        let values: Vec<Value> = vec![inbox.into(), thread.into(), (next + offset).into(), run.into(), row.kind.clone().into(), row.payload.to_string().into()];
        txn.execute_raw(Statement::from_sql_and_values(DatabaseBackend::Postgres, INSERT_SQL, values)).await?;
    }
    Ok(prepared.len())
}

/// The committed payloads of `thread`, ordered by `sequence`.
pub async fn read_session(db: &impl ConnectionTrait, thread: Uuid) -> Result<Vec<serde_json::Value>, DbErr> {
    let rows = db.query_all_raw(Statement::from_sql_and_values(DatabaseBackend::Postgres, READ_SQL, [thread.into()])).await?;
    rows.iter()
        .map(|r| {
            let text: String = r.try_get("", "payload")?;
            serde_json::from_str(&text).map_err(|e| DbErr::Custom(format!("a session_event payload is not JSON: {e}")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use paigasus_agent_host_core::run::SessionRow;
    use serde_json::{Value, json};

    use super::prepare_rows;

    fn user_row(texts: &[&str]) -> SessionRow {
        let content: Vec<Value> = texts.iter().map(|t| json!({"type": "text", "text": t})).collect();
        SessionRow {
            kind: "user_message".into(),
            payload: json!({"type": "user_message", "content": content, "ts": "2026-11-22T00:00:00Z"}),
        }
    }

    fn assistant_row(text: &str) -> SessionRow {
        SessionRow {
            kind: "assistant_message".into(),
            payload: json!({"type": "assistant_message", "content": [{"type": "text", "text": text}], "ts": "2026-11-22T00:00:00Z"}),
        }
    }

    fn texts_of(row: &SessionRow) -> Vec<String> {
        row.payload["content"]
            .as_array()
            .expect("content")
            .iter()
            .map(|p| p["text"].as_str().expect("text").to_owned())
            .collect()
    }

    #[test]
    fn the_memory_part_of_this_run_is_removed() {
        let row = user_row(&["<memory-N1 path=\"/memories/a.md\">\nsecret\n</memory-N1>", "<event-N1 n=\"1\">x</event-N1>"]);
        let out = prepare_rows(&[row], "N1");
        let texts = texts_of(&out[0]);
        assert_eq!(texts, ["<event-N1 n=\"1\">x</event-N1>"]);
    }

    #[test]
    fn a_memory_part_of_another_nonce_stays() {
        let out = prepare_rows(&[user_row(&["<memory-N2>x</memory-N2>"])], "N1");
        assert_eq!(texts_of(&out[0]).len(), 1);
    }

    #[test]
    fn an_assistant_text_that_starts_with_the_prefix_stays() {
        let out = prepare_rows(&[assistant_row("<memory-N1 x>")], "N1");
        assert_eq!(out[0].payload, assistant_row("<memory-N1 x>").payload);
    }

    #[test]
    fn nul_is_replaced_at_every_depth() {
        let row = SessionRow {
            kind: "tool_called".into(),
            payload: json!({"type": "tool_called", "args": {"a\u{0}": ["x\u{0}y"]}}),
        };
        let out = prepare_rows(&[row], "N1");
        assert_eq!(out[0].payload, json!({"type": "tool_called", "args": {"a\u{FFFD}": ["x\u{FFFD}y"]}}));
    }
}
