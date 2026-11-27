// SPDX-License-Identifier: Apache-2.0

//! The Anthropic wire readers for the charge (SMA-750 D10, § 7): the stream scanner, the
//! non-stream body reader and the request estimate. None of them changes a byte that reaches
//! the client.

use serde_json::Value;

use super::usage::{BodyUsage, FALLBACK_REQUEST_CAP, MAX_RECORD_BYTES, NON_TEXT_PART_TOKENS, bytes_to_tokens};

/// The three D10 input fields. `output_tokens` is the fourth.
const INPUT_FIELDS: [&str; 3] = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"];

/// The D10 numbers of one `usage` object. Only a JSON number counts. `null` and an object
/// (`server_tool_use`) are ignored.
#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    input: [u64; 3],
    output: u64,
}

impl Counts {
    fn input_sum(&self) -> u64 {
        self.input.iter().fold(0u64, |acc, n| acc.saturating_add(*n))
    }

    fn sum(&self) -> u64 {
        self.input_sum().saturating_add(self.output)
    }

    /// Each field present in `usage` replaces the earlier value.
    fn replace_from(&mut self, usage: &Value) {
        for (slot, key) in self.input.iter_mut().zip(INPUT_FIELDS) {
            if let Some(n) = usage.get(key).and_then(Value::as_u64) {
                *slot = n;
            }
        }
        if let Some(n) = usage.get("output_tokens").and_then(Value::as_u64) {
            self.output = n;
        }
    }
}

const DATA_PREFIX: &[u8] = b"data:";

/// Reads a copy of a forwarded Anthropic SSE stream (§ 7.2). It dispatches on the top-level
/// `type` of each complete `data:` record, never on a substring.
#[derive(Debug, Default)]
pub struct AnthropicUsageScanner {
    line: Vec<u8>,
    line_len: usize,
    after_cr: bool,
    data: Vec<u8>,
    has_data: bool,
    record_bytes: usize,
    oversized: bool,
    /// The first bytes of the current line, kept even after the record is oversized.
    line_head: Vec<u8>,
    /// The record has a `data:` line. It is tracked apart from the buffers, which an oversized
    /// record clears. A record with only comment lines is not a completion record.
    saw_data: bool,
    counts: Counts,
    started: bool,
    delta_seen: bool,
    /// Complete data records after `message_start`.
    records: u64,
}

impl AnthropicUsageScanner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &[u8]) {
        for &byte in chunk {
            if self.after_cr {
                self.after_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\n' => self.end_line(),
                b'\r' => {
                    self.end_line();
                    self.after_cr = true;
                }
                _ => {
                    self.line_len = self.line_len.saturating_add(1);
                    self.record_bytes = self.record_bytes.saturating_add(1);
                    if self.line_len <= DATA_PREFIX.len() {
                        self.line_head.push(byte);
                        if self.line_head == DATA_PREFIX {
                            self.saw_data = true;
                        }
                    }
                    if self.record_bytes > MAX_RECORD_BYTES {
                        self.oversized = true;
                        self.line.clear();
                        self.data.clear();
                    } else {
                        self.line.push(byte);
                    }
                }
            }
        }
    }

    /// The upstream stream ended or failed. Close a pending line and record. A second call
    /// does nothing.
    pub fn finish(&mut self) {
        self.feed(b"\n\n");
    }

    /// Complete data records after `message_start`.
    pub fn completion_records(&self) -> u64 {
        self.records
    }

    /// `None` before `message_start`. Before a `message_delta`: the input sum plus
    /// `max(output_tokens, records)`. After it: the exact D10 sum.
    pub fn reported_total(&self) -> Option<u64> {
        if !self.started {
            return None;
        }
        if self.delta_seen {
            return Some(self.counts.sum());
        }
        Some(self.counts.input_sum().saturating_add(self.counts.output.max(self.records)))
    }

    fn end_line(&mut self) {
        self.line_head.clear();
        if self.line_len == 0 {
            self.end_record();
            return;
        }
        self.line_len = 0;
        let line = std::mem::take(&mut self.line);
        if self.oversized {
            return;
        }
        if let Some(value) = line.strip_prefix(b"data:") {
            let value = value.strip_prefix(b" ").unwrap_or(value);
            if self.has_data {
                self.data.push(b'\n');
            }
            self.data.extend_from_slice(value);
            self.has_data = true;
        }
    }

    fn end_record(&mut self) {
        if self.oversized {
            if self.saw_data {
                self.count_record();
            }
        } else if self.has_data {
            self.dispatch();
        }
        self.data.clear();
        self.has_data = false;
        self.oversized = false;
        self.saw_data = false;
        self.record_bytes = 0;
    }

    fn count_record(&mut self) {
        if self.started {
            self.records = self.records.saturating_add(1);
        }
    }

    fn dispatch(&mut self) {
        let value = serde_json::from_slice::<Value>(&self.data).ok();
        match value.as_ref().and_then(|v| v.get("type")).and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(usage) = value.as_ref().and_then(|v| v.pointer("/message/usage")).filter(|u| u.is_object()) {
                    self.counts = Counts::default();
                    self.counts.replace_from(usage);
                    self.started = true;
                    self.delta_seen = false;
                    self.records = 0;
                }
            }
            Some("message_delta") if self.started => {
                if let Some(usage) = value.as_ref().and_then(|v| v.get("usage")).filter(|u| u.is_object()) {
                    self.counts.replace_from(usage);
                    self.delta_seen = true;
                }
            }
            _ => self.count_record(),
        }
    }
}

/// § 7.1: the D10 sum of `usage`, or the completion estimate.
pub fn non_stream_usage(body: &[u8]) -> BodyUsage {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return BodyUsage::Completion(0);
    };
    if let Some(usage) = value.get("usage").filter(|u| u.is_object()) {
        let mut counts = Counts::default();
        counts.replace_from(usage);
        return BodyUsage::Reported(counts.sum());
    }
    let mut bytes = 0usize;
    for block in value.get("content").and_then(Value::as_array).into_iter().flatten() {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    bytes = bytes.saturating_add(text.len());
                }
            }
            Some("tool_use") => {
                if let Some(input) = block.get("input") {
                    bytes = bytes.saturating_add(compact_len(input));
                }
            }
            _ => {}
        }
    }
    BodyUsage::Completion(bytes_to_tokens(bytes))
}

fn compact_len(v: &Value) -> usize {
    serde_json::to_string(v).map(|s| s.len()).unwrap_or(0)
}

/// Text bytes and non-text blocks of one `content` value (a string or an array of blocks).
/// `None` for any other shape.
fn measure_content(content: &Value, text_bytes: &mut usize, other: &mut u64) -> Option<()> {
    match content {
        Value::String(s) => *text_bytes = text_bytes.saturating_add(s.len()),
        Value::Array(blocks) => {
            for block in blocks {
                measure_block(block, text_bytes, other)?;
            }
        }
        _ => return None,
    }
    Some(())
}

fn measure_block(block: &Value, text_bytes: &mut usize, other: &mut u64) -> Option<()> {
    let kind = block.get("type").and_then(Value::as_str)?;
    match kind {
        "text" => {
            let text = block.get("text").and_then(Value::as_str)?;
            *text_bytes = text_bytes.saturating_add(text.len());
        }
        "tool_use" => {
            if let Some(input) = block.get("input") {
                *text_bytes = text_bytes.saturating_add(compact_len(input));
            }
        }
        "tool_result" => match block.get("content") {
            None | Some(Value::Null) => {}
            Some(inner) => measure_content(inner, text_bytes, other)?,
        },
        _ => *other = other.saturating_add(1),
    }
    Some(())
}

/// § 7.3: text counts `ceil(bytes/4)`, a non-text block 85. The result is at least
/// `min(ceil(body_len/4), 32768)`, which is also the answer for an unreadable shape.
pub fn request_tokens_estimate(v: &Value, body_len: usize) -> u64 {
    let floor = bytes_to_tokens(body_len).min(FALLBACK_REQUEST_CAP);
    let mut text_bytes = 0usize;
    let mut other = 0u64;
    match v.get("system") {
        None | Some(Value::Null) => {}
        Some(system) => {
            if measure_content(system, &mut text_bytes, &mut other).is_none() {
                return floor;
            }
        }
    }
    for message in v.get("messages").and_then(Value::as_array).into_iter().flatten() {
        let Some(message) = message.as_object() else { return floor };
        match message.get("content") {
            None | Some(Value::Null) => {}
            Some(content) => {
                if measure_content(content, &mut text_bytes, &mut other).is_none() {
                    return floor;
                }
            }
        }
    }
    for tool in v.get("tools").and_then(Value::as_array).into_iter().flatten() {
        text_bytes = text_bytes.saturating_add(compact_len(tool));
    }
    bytes_to_tokens(text_bytes).saturating_add(other.saturating_mul(NON_TEXT_PART_TOKENS)).max(floor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(v: Value) -> String {
        format!("event: x\ndata: {v}\n\n")
    }

    fn start(i: u64, cc: u64, cr: u64, o: u64) -> String {
        rec(json!({"type":"message_start","message":{"usage":{
            "input_tokens":i,"cache_creation_input_tokens":cc,"cache_read_input_tokens":cr,"output_tokens":o}}}))
    }

    fn delta(usage: Value) -> String {
        rec(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":usage}))
    }

    fn content() -> String {
        rec(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}))
    }

    fn scan(s: &str) -> AnthropicUsageScanner {
        let mut sc = AnthropicUsageScanner::new();
        sc.feed(s.as_bytes());
        sc.finish();
        sc
    }

    #[test]
    fn a_full_stream_reports_the_d10_sum() {
        let s = format!("{}{}{}", start(10, 2, 3, 1), content(), delta(json!({"output_tokens":7})));
        assert_eq!(scan(&s).reported_total(), Some(10 + 2 + 3 + 7));
    }

    #[test]
    fn a_delta_replaces_each_present_field() {
        let s = format!("{}{}", start(10, 2, 3, 1), delta(json!({"input_tokens":12,"output_tokens":7})));
        assert_eq!(scan(&s).reported_total(), Some(12 + 2 + 3 + 7));
    }

    #[test]
    fn a_stream_cut_after_message_start_estimates_the_output() {
        let tail = content().repeat(4);
        assert_eq!(scan(&format!("{}{tail}", start(10, 2, 3, 1))).reported_total(), Some(15 + 4));
        assert_eq!(scan(&format!("{}{tail}", start(10, 2, 3, 9))).reported_total(), Some(15 + 9));
    }

    #[test]
    fn no_message_start_reports_nothing() {
        assert_eq!(scan(&format!("{}{}", content(), delta(json!({"output_tokens":7})))).reported_total(), None);
        assert_eq!(scan("").reported_total(), None);
    }

    /// Review Focus 4.
    #[test]
    fn null_and_object_usage_fields_are_ignored() {
        let s = format!(
            "{}{}",
            rec(json!({"type":"message_start","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":null,
                "server_tool_use":{"web_search_requests":1},"output_tokens":1}}})),
            delta(json!({"output_tokens":null,"cache_read_input_tokens":null,"server_tool_use":{"web_search_requests":1}}))
        );
        let sc = scan(&s);
        assert_eq!(sc.reported_total(), Some(11));
        assert_eq!(sc.completion_records(), 0, "message_start and message_delta are not content records");
        let only_object = scan(&format!("{}{}", start(10, 0, 0, 1), delta(json!({"server_tool_use":{"web_search_requests":1}}))));
        assert_eq!(only_object.reported_total(), Some(11), "the record still counts as the delta");
    }

    #[test]
    fn the_type_decides_not_a_substring() {
        let tricky = rec(json!({"type":"content_block_delta","delta":{"type":"text_delta",
            "text":"{\"type\":\"message_delta\",\"usage\":{\"output_tokens\":999}}"}}));
        let sc = scan(&format!("{}{tricky}", start(10, 0, 0, 1)));
        assert_eq!(sc.reported_total(), Some(10 + 1), "no delta seen");
        assert_eq!(sc.completion_records(), 1);
    }

    #[test]
    fn line_endings_cr_lf_and_crlf_split_records() {
        let lf = format!("{}{}{}", start(10, 2, 3, 1), content(), delta(json!({"output_tokens":7})));
        for ending in ["\n", "\r", "\r\n"] {
            let s = lf.replace('\n', ending);
            assert_eq!(scan(&s).reported_total(), Some(22), "{ending:?}");
            let mut sc = AnthropicUsageScanner::new();
            for b in s.as_bytes() {
                sc.feed(std::slice::from_ref(b));
            }
            sc.finish();
            assert_eq!(sc.reported_total(), Some(22), "byte by byte {ending:?}");
        }
    }

    #[test]
    fn an_oversized_record_counts_once_and_is_not_parsed() {
        let mut sc = AnthropicUsageScanner::new();
        sc.feed(start(10, 0, 0, 1).as_bytes());
        sc.feed(b"data: ");
        for _ in 0..10 {
            sc.feed(&vec![b'x'; 8 * 1024]);
        }
        assert!(sc.line.len() <= MAX_RECORD_BYTES && sc.data.len() <= MAX_RECORD_BYTES);
        sc.feed(b"\n\n");
        assert_eq!((sc.completion_records(), sc.reported_total()), (1, Some(11)));
    }

    /// An oversized comment has no `data` field. It is not a completion record.
    #[test]
    fn an_oversized_comment_is_not_a_completion_record() {
        let mut sc = AnthropicUsageScanner::new();
        sc.feed(start(10, 0, 0, 0).as_bytes());
        sc.feed(b": ");
        for _ in 0..10 {
            sc.feed(&vec![b'x'; 8 * 1024]);
        }
        sc.feed(b"\n\n");
        sc.finish();
        assert_eq!((sc.completion_records(), sc.reported_total()), (0, Some(10)));
    }

    #[test]
    fn a_recorded_stream_reports_the_final_delta() {
        let raw = include_bytes!("../../../../../libs/paigasus-gateway-core/tests/fixtures/anthropic/recorded/text_stream.sse");
        let mut sc = AnthropicUsageScanner::new();
        sc.feed(raw);
        sc.finish();
        assert!(sc.reported_total().is_some_and(|t| t > 0));
        assert!(sc.delta_seen);
    }

    #[test]
    fn the_body_reports_usage_or_estimates() {
        let with = json!({"usage":{"input_tokens":10,"output_tokens":7,"server_tool_use":{"x":1}},"content":[]});
        assert_eq!(non_stream_usage(with.to_string().as_bytes()), BodyUsage::Reported(17));
        let full = json!({"usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":4}});
        assert_eq!(non_stream_usage(full.to_string().as_bytes()), BodyUsage::Reported(10));
        let content = json!([{"type":"text","text":"abcdefgh"},{"type":"tool_use","input":{"a":1}},{"type":"thinking","thinking":"zzzzzzzz"}]);
        // 8 + len(`{"a":1}`) = 8 + 7 = 15 bytes -> 4 tokens
        for body in [json!({"content":content}), json!({"usage":null,"content":content}), json!({"usage":"x","content":content})] {
            assert_eq!(non_stream_usage(body.to_string().as_bytes()), BodyUsage::Completion(4), "{body}");
        }
        assert_eq!(non_stream_usage(b"nope"), BodyUsage::Completion(0));
    }

    #[test]
    fn the_request_estimate_follows_section_7_3() {
        let est = |v: Value| request_tokens_estimate(&v, 0);
        assert_eq!(est(json!({"system":"abcdefgh","messages":[]})), 2);
        assert_eq!(est(json!({"system":[{"type":"text","text":"abcdefgh"}],"messages":[]})), 2);
        assert_eq!(est(json!({"messages":[{"role":"user","content":"abcdefgh"}]})), 2);
        assert_eq!(est(json!({"messages":[{"role":"user","content":[{"type":"text","text":"abcdefgh"}]}]})), 2);
        // `{"a":1}` is 7 bytes.
        assert_eq!(est(json!({"messages":[{"role":"assistant","content":[{"type":"tool_use","input":{"a":1}}]}]})), 2);
        assert_eq!(est(json!({"messages":[{"role":"user","content":[{"type":"tool_result","content":"abcdefgh"}]}]})), 2);
        assert_eq!(
            est(json!({"messages":[{"role":"user","content":[{"type":"tool_result","content":[{"type":"text","text":"abcdefgh"}]}]}]})),
            2
        );
        assert_eq!(est(json!({"messages":[],"tools":[{"name":"t"}]})), 3); // `{"name":"t"}` = 12 bytes
        assert_eq!(est(json!({"messages":[{"role":"user","content":[{"type":"image","source":{}}]}]})), 85);
        // The floor, and the cap on the floor.
        assert_eq!(request_tokens_estimate(&json!({"messages":[{"role":"user","content":"a"}]}), 400), 100);
        assert_eq!(request_tokens_estimate(&json!({"messages":[]}), 10_000_000), 32_768);
        assert_eq!(request_tokens_estimate(&json!({"messages":["x"]}), 400), 100);
        assert_eq!(request_tokens_estimate(&json!({"messages":[{"role":"user","content":7}]}), 400), 100);
    }
}
