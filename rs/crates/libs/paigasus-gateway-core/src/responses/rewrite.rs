// SPDX-License-Identifier: Apache-2.0
//! The rewrite of a blocked Responses item (spec § 6.5): one completed `message` item that holds
//! the notice, and the four events that a stream carries for it. Key order is the order of § 6.5.

use serde_json::{Map, Value, json};

/// The id of the message that replaces the blocked item at `output_index` `i`.
pub(crate) fn message_id(i: u32) -> String {
    format!("msg_paigasus_{i}")
}

/// The completed message item. It is the item in `output_item.done` and in a body's `output[]`.
pub(crate) fn completed_message(i: u32, notice: &str) -> Value {
    json!({
        "type": "message",
        "id": message_id(i),
        "role": "assistant",
        "status": "completed",
        "content": [{"type": "output_text", "text": notice, "annotations": [], "logprobs": []}],
    })
}

/// An event object with `fields`, `sequence_number` after them when the stream has one.
fn event(fields: Vec<(&'static str, Value)>, seq: Option<&Value>, tail: Vec<(&'static str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in fields {
        m.insert(k.to_owned(), v);
    }
    if let Some(s) = seq {
        m.insert("sequence_number".to_owned(), s.clone());
    }
    for (k, v) in tail {
        m.insert(k.to_owned(), v);
    }
    Value::Object(m)
}

/// The three events that replace the `added` frame of a blocked item. Each repeats `seq`.
pub(crate) fn added_events(i: u32, notice: &str, seq: Option<&Value>) -> [Value; 3] {
    let id = message_id(i);
    [
        event(
            vec![("type", json!("response.output_item.added")), ("output_index", json!(i))],
            seq,
            vec![("item", json!({"type": "message", "id": id, "role": "assistant", "status": "in_progress", "content": []}))],
        ),
        event(
            vec![
                ("type", json!("response.content_part.added")),
                ("item_id", json!(id)),
                ("output_index", json!(i)),
                ("content_index", json!(0)),
            ],
            seq,
            vec![("part", json!({"type": "output_text", "text": "", "annotations": [], "logprobs": []}))],
        ),
        event(
            vec![
                ("type", json!("response.output_text.delta")),
                ("item_id", json!(id)),
                ("output_index", json!(i)),
                ("content_index", json!(0)),
            ],
            seq,
            vec![("delta", json!(notice)), ("logprobs", json!([]))],
        ),
    ]
}

/// The event that replaces the `done` frame of a blocked item.
pub(crate) fn done_event(i: u32, notice: &str, seq: Option<&Value>) -> Value {
    event(
        vec![("type", json!("response.output_item.done")), ("output_index", json!(i))],
        seq,
        vec![("item", completed_message(i, notice))],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rewrite_json_is_exact() {
        let n = "N";
        let s = json!(7);
        let [a, b, c] = added_events(3, n, Some(&s));
        let d = done_event(3, n, Some(&s));
        let got: Vec<String> = [a, b, c, d].iter().map(|v| serde_json::to_string(v).unwrap()).collect();
        let want = [
            r#"{"type":"response.output_item.added","output_index":3,"sequence_number":7,"item":{"type":"message","id":"msg_paigasus_3","role":"assistant","status":"in_progress","content":[]}}"#,
            r#"{"type":"response.content_part.added","item_id":"msg_paigasus_3","output_index":3,"content_index":0,"sequence_number":7,"part":{"type":"output_text","text":"","annotations":[],"logprobs":[]}}"#,
            r#"{"type":"response.output_text.delta","item_id":"msg_paigasus_3","output_index":3,"content_index":0,"sequence_number":7,"delta":"N","logprobs":[]}"#,
            r#"{"type":"response.output_item.done","output_index":3,"sequence_number":7,"item":{"type":"message","id":"msg_paigasus_3","role":"assistant","status":"completed","content":[{"type":"output_text","text":"N","annotations":[],"logprobs":[]}]}}"#,
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn a_stream_without_sequence_numbers_gets_none() {
        let [a, b, c] = added_events(0, "N", None);
        for v in [a, b, c, done_event(0, "N", None)] {
            assert!(v.get("sequence_number").is_none(), "{v}");
        }
        assert_eq!(message_id(12), "msg_paigasus_12");
    }

    #[test]
    fn the_notice_is_escaped_as_json() {
        let v = done_event(1, "a\"b\\c\n", None);
        assert_eq!(v["item"]["content"][0]["text"], "a\"b\\c\n");
        assert!(serde_json::to_string(&v).unwrap().contains(r#"a\"b\\c\n"#));
    }
}
