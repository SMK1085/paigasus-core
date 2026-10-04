// SPDX-License-Identifier: Apache-2.0

//! The OpenAI wire readers for the charge (SMA-677 D14, § 4.6): the request estimate, the
//! non-stream completion estimate, and `UsageScanner`, which reads a COPY of a forwarded SSE
//! stream. None of them changes a byte that reaches the client (A5).

use serde_json::Value;

/// § 4.6: the bound on one partial SSE record. A larger record is skipped, not buffered, and
/// still counts as one record. The same bound as the console parser (`chat-stream.ts`).
pub const MAX_RECORD_BYTES: usize = 64 * 1024;
/// D14: the fallback request estimate is capped.
const FALLBACK_REQUEST_CAP: u64 = 32_768;
/// D14: an image, audio or other non-text part counts OpenAI's low-detail image cost.
const NON_TEXT_PART_TOKENS: u64 = 85;

fn bytes_to_tokens(bytes: usize) -> u64 {
    u64::try_from(bytes).unwrap_or(u64::MAX).div_ceil(4)
}

/// D14: `ceil(text_bytes / 4)` over every string `content` and every `text` part, plus 85 per
/// non-text part. A `null` or absent `content` counts 0. Any other shape falls back to
/// `ceil(body_len / 4)`, capped at 32768.
pub fn request_tokens_estimate(messages: &[Value], body_len: usize) -> u64 {
    let fallback = bytes_to_tokens(body_len).min(FALLBACK_REQUEST_CAP);
    let mut text_bytes = 0usize;
    let mut other_parts = 0u64;
    for message in messages {
        let Some(message) = message.as_object() else { return fallback };
        match message.get("content") {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) => text_bytes = text_bytes.saturating_add(text.len()),
            Some(Value::Array(parts)) => {
                for part in parts {
                    match (part.get("type").and_then(Value::as_str), part.get("text").and_then(Value::as_str)) {
                        (Some("text"), Some(text)) => text_bytes = text_bytes.saturating_add(text.len()),
                        (Some(_), _) => other_parts = other_parts.saturating_add(1),
                        (None, _) => return fallback,
                    }
                }
            }
            Some(_) => return fallback,
        }
    }
    bytes_to_tokens(text_bytes).saturating_add(other_parts.saturating_mul(NON_TEXT_PART_TOKENS))
}

/// What a non-stream `2xx` body says about its tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyUsage {
    /// `usage.total_tokens`.
    Reported(u64),
    /// D14: `ceil(len / 4)` of every `choices[].message.content` and tool-call `arguments`.
    Completion(u64),
}

pub fn non_stream_usage(body: &[u8]) -> BodyUsage {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return BodyUsage::Completion(0);
    };
    if let Some(total) = value.pointer("/usage/total_tokens").and_then(Value::as_u64) {
        return BodyUsage::Reported(total);
    }
    let mut bytes = 0usize;
    for choice in value.get("choices").and_then(Value::as_array).into_iter().flatten() {
        if let Some(content) = choice.pointer("/message/content").and_then(Value::as_str) {
            bytes = bytes.saturating_add(content.len());
        }
        for call in choice.pointer("/message/tool_calls").and_then(Value::as_array).into_iter().flatten() {
            if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
                bytes = bytes.saturating_add(arguments.len());
            }
        }
    }
    BodyUsage::Completion(bytes_to_tokens(bytes))
}

/// Reads a copy of a forwarded SSE stream (§ 4.6). Every complete `data:` record that is not
/// `[DONE]` and holds no usage object adds one to the completion estimate (an OpenAI chunk
/// usually carries one token). A record that holds `"usage":{` is parsed, and the last
/// `usage.total_tokens` seen wins.
#[derive(Debug, Default)]
pub struct UsageScanner {
    /// The current partial line, bounded by `MAX_RECORD_BYTES`.
    line: Vec<u8>,
    /// Bytes of the current line, also when they were not buffered.
    line_len: usize,
    /// The last byte was CR: an LF right after it belongs to the same terminator.
    after_cr: bool,
    /// The current record's `data` value (lines joined with LF), bounded.
    data: Vec<u8>,
    has_data: bool,
    record_bytes: usize,
    oversized: bool,
    records: u64,
    reported: Option<u64>,
}

impl UsageScanner {
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

    pub fn completion_records(&self) -> u64 {
        self.records
    }

    pub fn reported_total(&self) -> Option<u64> {
        self.reported
    }

    fn end_line(&mut self) {
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
            self.records = self.records.saturating_add(1);
        } else if self.has_data {
            self.dispatch();
        }
        self.data.clear();
        self.has_data = false;
        self.oversized = false;
        self.record_bytes = 0;
    }

    fn dispatch(&mut self) {
        if self.data.as_slice() == b"[DONE]" {
            return;
        }
        if has_usage_object(&self.data)
            && let Some(total) = serde_json::from_slice::<Value>(&self.data).ok().and_then(|v| v.pointer("/usage/total_tokens").and_then(Value::as_u64))
        {
            self.reported = Some(total);
            return;
        }
        self.records = self.records.saturating_add(1);
    }
}

/// `"usage"` + optional whitespace + `:` + optional whitespace + `{`. With `include_usage`,
/// OpenAI sends `"usage":null` in every chunk, so a match on `"usage"` alone would parse every
/// record.
fn has_usage_object(data: &[u8]) -> bool {
    const KEY: &[u8] = b"\"usage\"";
    let mut rest = data;
    while let Some(pos) = rest.windows(KEY.len()).position(|w| w == KEY) {
        rest = &rest[pos + KEY.len()..];
        if let Some(after_colon) = trim_start(rest).strip_prefix(b":")
            && trim_start(after_colon).first() == Some(&b'{')
        {
            return true;
        }
    }
    false
}

fn trim_start(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    &bytes[start..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scan(chunks: &[&str]) -> UsageScanner {
        let mut scanner = UsageScanner::new();
        for chunk in chunks {
            scanner.feed(chunk.as_bytes());
        }
        scanner
    }

    /// A real-shaped OpenAI chunk (spec § 5.4), optionally with `"usage":null`.
    fn chunk(content: &str, usage_null: bool) -> String {
        let usage = if usage_null { ",\"usage\":null" } else { "" };
        format!(
            "data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1790942400,\"model\":\"gpt-4o-mini\",\"system_fingerprint\":\"fp_1\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{content}\"}},\"finish_reason\":null}}]{usage}}}\n\n"
        )
    }

    fn usage_chunk(total: u64) -> String {
        format!("data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"choices\":[],\"usage\":{{\"prompt_tokens\":3,\"completion_tokens\":4,\"total_tokens\":{total}}}}}\n\n")
    }

    #[test]
    fn records_end_at_a_blank_line_with_every_line_ending() {
        for input in ["data: a\n\ndata: b\n\n", "data: a\r\n\r\ndata: b\r\n\r\n", "data: a\r\rdata: b\r\r", "data: a\r\n\ndata: b\n\r\n"] {
            assert_eq!(scan(&[input]).completion_records(), 2, "{input:?}");
        }
    }

    /// Review Focus 3.
    #[test]
    fn a_crlf_split_across_chunks_is_one_terminator() {
        assert_eq!(scan(&["data: a\r", "\n\r", "\n"]).completion_records(), 1);
        assert_eq!(scan(&["data: a\r\n\r", "\ndata: b\r\n\r\n"]).completion_records(), 2);
        // A lone CR followed by a non-LF byte is a terminator of its own.
        assert_eq!(scan(&["data: a\r", "\rdata: b\r\r"]).completion_records(), 2);
    }

    #[test]
    fn a_record_split_byte_by_byte_counts_once() {
        let record = chunk(" the", true);
        let mut scanner = UsageScanner::new();
        for byte in record.as_bytes() {
            scanner.feed(std::slice::from_ref(byte));
        }
        assert_eq!(scanner.completion_records(), 1);
    }

    #[test]
    fn done_comments_and_empty_records_are_not_counted() {
        assert_eq!(scan(&["\n\n: keep-alive\n\ndata: [DONE]\n\n"]).completion_records(), 0);
    }

    #[test]
    fn a_null_usage_is_a_record_and_a_usage_object_is_reported() {
        let stream = format!("{}{}{}{}data: [DONE]\n\n", chunk("Hel", true), chunk("lo", true), chunk("!", false), usage_chunk(7));
        let scanner = scan(&[stream.as_str()]);
        assert_eq!(scanner.completion_records(), 3, "`\"usage\":null` must not be read as a usage record");
        assert_eq!(scanner.reported_total(), Some(7));
    }

    #[test]
    fn the_last_usage_wins_and_whitespace_after_the_colon_matches() {
        let stream = format!("{}data: {{\"choices\":[],\"usage\": {{\"total_tokens\":9}}}}\n\n", usage_chunk(7));
        assert_eq!(scan(&[stream.as_str()]).reported_total(), Some(9));
    }

    #[test]
    fn a_usage_record_that_does_not_parse_counts_as_a_record() {
        let scanner = scan(&["data: {\"usage\":{\"total_tokens\":\n\n"]);
        assert_eq!((scanner.completion_records(), scanner.reported_total()), (1, None));
    }

    #[test]
    fn an_oversized_record_counts_once_and_is_not_buffered() {
        let mut scanner = UsageScanner::new();
        scanner.feed(b"data: ");
        let filler = vec![b'x'; 8 * 1024];
        for _ in 0..10 {
            scanner.feed(&filler);
        }
        assert!(scanner.line.len() <= MAX_RECORD_BYTES && scanner.data.len() <= MAX_RECORD_BYTES, "the tail stays bounded");
        scanner.feed(b"\n\n");
        scanner.feed(chunk("next", true).as_bytes());
        assert_eq!(scanner.completion_records(), 2, "the oversized record counts as one, and the next record still parses");
    }

    /// Review Focus 4 and D14.
    #[test]
    fn request_estimate_shapes() {
        let est = |messages: serde_json::Value, body_len: usize| request_tokens_estimate(messages.as_array().expect("an array"), body_len);
        // String contents: 5 + 2 bytes → ceil(7 / 4) = 2.
        assert_eq!(est(json!([{"role": "system", "content": "hello"}, {"role": "user", "content": "hi"}]), 999), 2);
        // A text part and an image part: ceil(4 / 4) + 85.
        assert_eq!(
            est(
                json!([{"role": "user", "content": [{"type": "text", "text": "abcd"}, {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}]}]),
                999
            ),
            86
        );
        // `content: null` with tool calls counts 0, and is not a reason for the fallback.
        assert_eq!(
            est(json!([{"role": "assistant", "content": null, "tool_calls": [{"id": "c1"}]}, {"role": "user", "content": "abcd"}]), 999),
            1
        );
        // An unknown shape uses ceil(body_len / 4), capped at 32768.
        assert_eq!(est(json!([{"role": "user", "content": 42}]), 400), 100);
        assert_eq!(est(json!(["not an object"]), 400), 100);
        assert_eq!(est(json!([{"role": "user", "content": [{"text": "no type"}]}]), 401), 101);
        assert_eq!(est(json!([{"role": "user", "content": 42}]), 1_000_000), 32_768);
        assert_eq!(est(json!([]), 10), 0);
    }

    #[test]
    fn non_stream_usage_reads_usage_or_estimates_the_completion() {
        assert_eq!(non_stream_usage(br#"{"choices":[],"usage":{"total_tokens":12}}"#), BodyUsage::Reported(12));
        // 8 content bytes + 7 argument bytes (`{"a":1}`) → ceil(15 / 4) = 4.
        let body = br#"{"choices":[{"message":{"content":"abcdefgh","tool_calls":[{"function":{"name":"f","arguments":"{\"a\":1}"}}]}}]}"#;
        assert_eq!(non_stream_usage(body), BodyUsage::Completion(4));
        assert_eq!(non_stream_usage(b"not json"), BodyUsage::Completion(0));
        assert_eq!(non_stream_usage(br#"{"usage":{"total_tokens":"many"},"choices":[]}"#), BodyUsage::Completion(0));
    }
}
