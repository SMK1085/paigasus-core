// SPDX-License-Identifier: Apache-2.0

//! The OpenAI Responses API readers for the charge (SMA-769 D14, § 8.2): the request estimate,
//! the non-stream completion estimate, and [`ResponsesUsageScanner`], which reads a COPY of a
//! forwarded SSE stream. None of them changes a byte that reaches the client.
//!
//! A terminal event (`response.completed`, `response.incomplete`, `response.failed`) carries the
//! whole response, so it can be many MiB. The scanner therefore never buffers a record. It reads
//! `response.usage.total_tokens` with a byte-level JSON scan that holds only fixed state, so it has
//! no record size limit (R-g).

use serde_json::Value;

use super::usage::{BodyUsage, FALLBACK_REQUEST_CAP, NON_TEXT_PART_TOKENS, bytes_to_tokens};

/// The three event types whose `response.usage` is the charge (§ 8.2).
const TERMINAL_TYPES: [&[u8]; 3] = [b"response.completed", b"response.incomplete", b"response.failed"];
/// A `type` string longer than this is not a terminal type, and is not held.
const MAX_TYPE_BYTES: usize = 64;
/// `u64::MAX` has 20 digits. A longer `total_tokens` is not charged.
const MAX_TOTAL_DIGITS: u32 = 20;
/// serde_json 1.0's recursion limit: it accepts at most 127 nested containers. A deeper record is
/// malformed here too, and the container stack fits in one `u128`.
const MAX_DEPTH: u32 = 127;
/// The longest key on the usage path, `total_tokens`. A longer key matches nothing.
const MAX_KEY_BYTES: usize = 12;
/// The key on the usage path at depth 1, 2 and 3.
const USAGE_PATH: [&[u8]; 3] = [b"response", b"usage", b"total_tokens"];
/// The key of the event type, at depth 1.
const TYPE_KEY: &[u8] = b"type";

/// Reads a copy of a forwarded Responses SSE stream (§ 8.2). The SSE rules are those of
/// [`super::usage::UsageScanner`]: CR, LF and CRLF end a line, a `data:` line adds its value (one
/// optional space removed), data lines of one record are joined with LF, and a blank line ends the
/// record. Every complete data record that is not `[DONE]` and gives no usage adds one to the
/// completion estimate. A terminal record that is well-formed JSON with a `u64` at
/// `/response/usage/total_tokens` gives its total, and the last such total wins.
///
/// Every field is fixed size: the scanner holds no heap buffer, whatever the record size. Two
/// compile-time checks below keep it so: the type is `Copy` (a `Vec`, `Box` or `String` field
/// cannot be `Copy`), and its size is under 1024 bytes.
#[derive(Debug, Default, Clone, Copy)]
pub struct ResponsesUsageScanner {
    /// The current line has at least one byte.
    line_started: bool,
    /// The last byte was CR: an LF right after it belongs to the same terminator.
    after_cr: bool,
    line: LineState,
    /// The bytes of `data:` the current line has matched.
    prefix_len: usize,
    /// The current record has a data line.
    has_data: bool,
    /// The bytes of the record's data value, and its first six bytes (to match `[DONE]`).
    data_len: usize,
    head: [u8; 6],
    json: JsonScan,
    records: u64,
    reported: Option<u64>,
}

/// The scanner has no heap field: only a `Copy` type can satisfy this bound.
const fn assert_no_heap_fields<T: Copy>() {}
const _: () = assert_no_heap_fields::<ResponsesUsageScanner>();
/// The fixed state stays small; `held_bytes()` in the tests checks the same bound.
const _: () = assert!(std::mem::size_of::<ResponsesUsageScanner>() < 1024);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum LineState {
    /// Reading the first five bytes.
    #[default]
    Prefix,
    /// After `data:`: one space here is removed.
    Space,
    /// The data value.
    Data,
    /// Not a data line: the rest of the line is ignored.
    Ignore,
}

impl ResponsesUsageScanner {
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
                _ => self.line_byte(byte),
            }
        }
    }

    /// The upstream stream ended, or failed. Close a pending line and a pending record, as a
    /// blank line does (SMA-730). A second call does nothing.
    pub fn finish(&mut self) {
        self.feed(b"\n\n");
    }

    pub fn completion_records(&self) -> u64 {
        self.records
    }

    pub fn reported_total(&self) -> Option<u64> {
        self.reported
    }

    /// The bytes the scanner holds. It has no heap buffer, so this is its fixed size.
    #[cfg(test)]
    fn held_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn line_byte(&mut self, byte: u8) {
        self.line_started = true;
        match self.line {
            LineState::Data => self.data_byte(byte),
            LineState::Space => {
                self.line = LineState::Data;
                if byte != b' ' {
                    self.data_byte(byte);
                }
            }
            LineState::Prefix => {
                const DATA: &[u8; 5] = b"data:";
                if byte != DATA[self.prefix_len] {
                    self.line = LineState::Ignore;
                    return;
                }
                self.prefix_len += 1;
                if self.prefix_len == DATA.len() {
                    self.line = LineState::Space;
                    if self.has_data {
                        self.data_byte(b'\n');
                    }
                    self.has_data = true;
                }
            }
            LineState::Ignore => {}
        }
    }

    fn data_byte(&mut self, byte: u8) {
        if let Some(slot) = self.head.get_mut(self.data_len) {
            *slot = byte;
        }
        self.data_len = self.data_len.saturating_add(1);
        self.json.feed(byte);
    }

    fn end_line(&mut self) {
        if !self.line_started {
            self.end_record();
            return;
        }
        self.line_started = false;
        self.line = LineState::Prefix;
        self.prefix_len = 0;
    }

    fn end_record(&mut self) {
        if self.has_data {
            if self.data_len == self.head.len() && &self.head == b"[DONE]" {
                // Not a completion record.
            } else if let Some(total) = self.json.terminal_total() {
                self.reported = Some(total);
            } else {
                self.records = self.records.saturating_add(1);
            }
        }
        self.has_data = false;
        self.data_len = 0;
        self.json = JsonScan::default();
    }
}

/// What the JSON grammar allows next.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// Any value: the top level, after `:`, or after `,` in an array.
    #[default]
    Value,
    /// A value or `]`: right after `[`.
    ValueOrEnd,
    /// A key or `}`: right after `{`.
    KeyOrEnd,
    /// A key: after `,` in an object.
    Key,
    Colon,
    /// `,` or the end of the container.
    CommaOrEnd,
    /// The top-level value is complete. Only whitespace may follow.
    Done,
}

/// The token being read.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Token {
    #[default]
    None,
    /// A string; `key` is true for an object key.
    Str { key: bool, escape: Escape },
    /// A number.
    Number(NumberState),
    /// `true`, `false` or `null`, with the bytes matched so far.
    Literal { word: &'static [u8], matched: usize },
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Escape {
    #[default]
    None,
    /// After a backslash.
    Backslash,
    /// Inside `\uXXXX`, with the hex digits read so far.
    Hex(u8),
}

/// The JSON number grammar: `-? (0 | [1-9][0-9]*) (. [0-9]+)? ([eE] [+-]? [0-9]+)?`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumberState {
    Minus,
    Zero,
    Int,
    Dot,
    Frac,
    Exp,
    ExpSign,
    ExpDigits,
}

impl NumberState {
    fn start(byte: u8) -> Option<Self> {
        match byte {
            b'-' => Some(Self::Minus),
            b'0' => Some(Self::Zero),
            b'1'..=b'9' => Some(Self::Int),
            _ => None,
        }
    }

    fn next(self, byte: u8) -> Option<Self> {
        use NumberState::*;
        match (self, byte) {
            (Minus, b'0') => Some(Zero),
            (Minus, b'1'..=b'9') | (Int, b'0'..=b'9') => Some(Int),
            (Zero | Int, b'.') => Some(Dot),
            (Dot | Frac, b'0'..=b'9') => Some(Frac),
            (Zero | Int | Frac, b'e' | b'E') => Some(Exp),
            (Exp, b'+' | b'-') => Some(ExpSign),
            (Exp | ExpSign | ExpDigits, b'0'..=b'9') => Some(ExpDigits),
            _ => None,
        }
    }

    fn is_complete(self) -> bool {
        matches!(self, Self::Zero | Self::Int | Self::Frac | Self::ExpDigits)
    }
}

/// What the current value is captured as.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Capture {
    #[default]
    None,
    /// The top-level `type` string.
    Type,
    /// The `/response/usage/total_tokens` number.
    Total,
}

/// The captured `type` bytes (std has no `Default` for `[u8; 64]`).
#[derive(Debug, Clone, Copy)]
struct TypeBuf([u8; MAX_TYPE_BYTES]);

impl Default for TypeBuf {
    fn default() -> Self {
        Self([0; MAX_TYPE_BYTES])
    }
}

/// A byte-level JSON scan of one record's data value. It checks the JSON grammar (structure,
/// strings, escapes, numbers and literals, depth up to 127) and captures the top-level `type` and
/// `/response/usage/total_tokens`. The last value of a duplicate key wins, as in serde_json. It
/// does not check UTF-8, and it does not decode escapes: a key or a `type` with an escape matches
/// nothing, so the estimate applies.
#[derive(Debug, Default, Clone, Copy)]
struct JsonScan {
    error: bool,
    expect: Expect,
    token: Token,
    depth: u32,
    /// Bit `d - 1` is set when the container at depth `d` is an object.
    objects: u128,
    /// The current key's bytes, up to `MAX_KEY_BYTES`.
    key: [u8; MAX_KEY_BYTES],
    key_len: usize,
    /// The current key is longer than `MAX_KEY_BYTES` or has an escape.
    key_other: bool,
    /// The current key at depth 1, 2 and 3 is the key of `USAGE_PATH` at that depth.
    on_path: [bool; 3],
    /// The current key at depth 1 is `type`.
    at_type: bool,
    capture: Capture,
    type_buf: TypeBuf,
    type_len: usize,
    /// The `type` string being captured is longer than `MAX_TYPE_BYTES` or has an escape.
    type_other: bool,
    /// The total being captured, or `None` when it is not a `u64`.
    total_acc: Option<u64>,
    total_digits: u32,
    /// The last top-level `type` is a terminal type.
    terminal: bool,
    /// The last `/response/usage/total_tokens`, when it is a `u64`.
    total: Option<u64>,
}

impl JsonScan {
    /// The total to charge: the record is complete, well-formed JSON, its last top-level `type`
    /// is terminal, and its last `/response/usage/total_tokens` is a `u64`.
    fn terminal_total(&self) -> Option<u64> {
        if self.error || self.expect != Expect::Done || self.token != Token::None || !self.terminal {
            return None;
        }
        self.total
    }

    fn feed(&mut self, byte: u8) {
        if self.error {
            return;
        }
        match self.token {
            Token::Str { key, escape } => self.string_byte(key, escape, byte),
            Token::Number(state) => match state.next(byte) {
                Some(next) => {
                    self.token = Token::Number(next);
                    self.total_byte(byte);
                }
                None if state.is_complete() => {
                    self.end_scalar();
                    self.structural(byte);
                }
                None => self.error = true,
            },
            Token::Literal { word, matched } => {
                if matched == word.len() {
                    self.end_scalar();
                    self.structural(byte);
                } else if word[matched] == byte {
                    self.token = Token::Literal { word, matched: matched + 1 };
                } else {
                    self.error = true;
                }
            }
            Token::None => self.structural(byte),
        }
    }

    fn string_byte(&mut self, key: bool, escape: Escape, byte: u8) {
        let next = match escape {
            Escape::Backslash => match byte {
                b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => Escape::None,
                b'u' => Escape::Hex(0),
                _ => return self.error = true,
            },
            Escape::Hex(n) if byte.is_ascii_hexdigit() => {
                if n == 3 {
                    Escape::None
                } else {
                    Escape::Hex(n + 1)
                }
            }
            Escape::Hex(_) => return self.error = true,
            Escape::None => match byte {
                b'"' => return self.end_string(key),
                b'\\' => {
                    // An escaped key or type matches nothing.
                    if key {
                        self.key_other = true;
                    } else if self.capture == Capture::Type {
                        self.type_other = true;
                    }
                    Escape::Backslash
                }
                0x00..=0x1f => return self.error = true,
                _ => {
                    if key {
                        self.key_byte(byte);
                    } else if self.capture == Capture::Type {
                        self.type_byte(byte);
                    }
                    Escape::None
                }
            },
        };
        self.token = Token::Str { key, escape: next };
    }

    fn key_byte(&mut self, byte: u8) {
        match self.key.get_mut(self.key_len) {
            Some(slot) => {
                *slot = byte;
                self.key_len += 1;
            }
            None => self.key_other = true,
        }
    }

    fn type_byte(&mut self, byte: u8) {
        match self.type_buf.0.get_mut(self.type_len) {
            Some(slot) => {
                *slot = byte;
                self.type_len += 1;
            }
            None => self.type_other = true,
        }
    }

    fn total_byte(&mut self, byte: u8) {
        if self.capture != Capture::Total {
            return;
        }
        // Only plain digits: a sign, a fraction or an exponent is not a token count.
        self.total_digits = self.total_digits.saturating_add(1);
        self.total_acc = match (self.total_acc, byte) {
            (Some(acc), b'0'..=b'9') if self.total_digits <= MAX_TOTAL_DIGITS => acc.checked_mul(10).and_then(|v| v.checked_add(u64::from(byte - b'0'))),
            _ => None,
        };
    }

    fn end_string(&mut self, key: bool) {
        self.token = Token::None;
        if key {
            let name = &self.key[..self.key_len];
            let matched = |want: &[u8]| !self.key_other && name == want;
            match self.depth {
                1 => {
                    self.on_path[0] = matched(USAGE_PATH[0]);
                    self.at_type = matched(TYPE_KEY);
                }
                2 => self.on_path[1] = matched(USAGE_PATH[1]),
                3 => self.on_path[2] = matched(USAGE_PATH[2]),
                _ => {}
            }
            self.expect = Expect::Colon;
            return;
        }
        if self.capture == Capture::Type {
            let value = &self.type_buf.0[..self.type_len];
            self.terminal = !self.type_other && TERMINAL_TYPES.contains(&value);
        }
        self.end_value();
    }

    fn end_scalar(&mut self) {
        self.token = Token::None;
        if self.capture == Capture::Total {
            self.total = self.total_acc;
        }
        self.end_value();
    }

    /// A value is complete.
    fn end_value(&mut self) {
        self.capture = Capture::None;
        self.expect = if self.depth == 0 { Expect::Done } else { Expect::CommaOrEnd };
    }

    fn in_object(&self) -> bool {
        self.depth > 0 && self.objects & (1u128 << (self.depth - 1)) != 0
    }

    /// A value begins in the current container. Reset what it replaces (the last duplicate key
    /// wins) and pick what to capture. A value in an array matches nothing: `open` clears the
    /// key flags of an array's depth, and only an object key sets them again.
    fn begin_value(&mut self, byte: u8) {
        let on_path = self.on_path;
        let path = |n: usize| on_path[..n].iter().all(|&on| on);
        match self.depth {
            1 if self.at_type => {
                self.terminal = false;
                if byte == b'"' {
                    self.capture = Capture::Type;
                    self.type_len = 0;
                    self.type_other = false;
                }
            }
            1 | 2 if path(self.depth as usize) => self.total = None,
            3 if path(3) => {
                self.total = None;
                if byte.is_ascii_digit() {
                    self.capture = Capture::Total;
                    self.total_acc = Some(0);
                    self.total_digits = 0;
                }
            }
            _ => {}
        }
    }

    fn open(&mut self, object: bool) {
        if self.depth >= MAX_DEPTH {
            self.error = true;
            return;
        }
        self.depth += 1;
        let bit = 1u128 << (self.depth - 1);
        if object {
            self.objects |= bit;
            self.expect = Expect::KeyOrEnd;
        } else {
            self.objects &= !bit;
            self.expect = Expect::ValueOrEnd;
        }
        if let Some(on) = self.on_path.get_mut(self.depth as usize - 1) {
            *on = false;
        }
        if self.depth == 1 {
            self.at_type = false;
        }
    }

    fn close(&mut self, object: bool) {
        if self.depth == 0 || self.in_object() != object {
            self.error = true;
            return;
        }
        self.depth -= 1;
        self.end_value();
    }

    fn start_value(&mut self, byte: u8) {
        self.begin_value(byte);
        match byte {
            b'{' => self.open(true),
            b'[' => self.open(false),
            b'"' => self.token = Token::Str { key: false, escape: Escape::None },
            b't' => self.token = Token::Literal { word: b"true", matched: 1 },
            b'f' => self.token = Token::Literal { word: b"false", matched: 1 },
            b'n' => self.token = Token::Literal { word: b"null", matched: 1 },
            _ => match NumberState::start(byte) {
                Some(state) => {
                    self.token = Token::Number(state);
                    self.total_byte(byte);
                }
                None => self.error = true,
            },
        }
    }

    /// A byte outside a string or a scalar.
    fn structural(&mut self, byte: u8) {
        if matches!(byte, b' ' | b'\t' | b'\n' | b'\r') {
            return;
        }
        match (self.expect, byte) {
            (Expect::ValueOrEnd, b']') => self.close(false),
            (Expect::Value | Expect::ValueOrEnd, _) => self.start_value(byte),
            (Expect::KeyOrEnd, b'}') => self.close(true),
            (Expect::KeyOrEnd | Expect::Key, b'"') => {
                self.key_len = 0;
                self.key_other = false;
                self.token = Token::Str { key: true, escape: Escape::None };
            }
            (Expect::Colon, b':') => self.expect = Expect::Value,
            (Expect::CommaOrEnd, b',') => self.expect = if self.in_object() { Expect::Key } else { Expect::Value },
            (Expect::CommaOrEnd, b'}') => self.close(true),
            (Expect::CommaOrEnd, b']') => self.close(false),
            _ => self.error = true,
        }
    }
}

/// A non-stream `2xx` Responses body (§ 8.2): `usage.total_tokens`, or the completion estimate
/// from `output[].content[].text`, `output[].arguments` and `output[].input` by the chat rule.
pub fn non_stream_usage(body: &[u8]) -> BodyUsage {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return BodyUsage::Completion(0);
    };
    if let Some(total) = value.pointer("/usage/total_tokens").and_then(Value::as_u64) {
        return BodyUsage::Reported(total);
    }
    let mut bytes = 0usize;
    for item in value.get("output").and_then(Value::as_array).into_iter().flatten() {
        for part in item.get("content").and_then(Value::as_array).into_iter().flatten() {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                bytes = bytes.saturating_add(text.len());
            }
        }
        for field in ["arguments", "input"] {
            if let Some(text) = item.get(field).and_then(Value::as_str) {
                bytes = bytes.saturating_add(text.len());
            }
        }
    }
    BodyUsage::Completion(bytes_to_tokens(bytes))
}

/// The item fields the estimate knows. An item with none of them (all absent or `null`) counts
/// its serialized JSON length.
const KNOWN_ITEM_FIELDS: [&str; 5] = ["content", "summary", "arguments", "input", "output"];

/// The request estimate by the chat rule (§ 8.2, SMA-677 D14), which counts the text of every
/// role: `ceil(text_bytes / 4)` over `instructions`, a string `input`, and per `input` item:
/// - `content`: a string, or per part its `text` string (of any part type) or its `refusal`
///   string; another typed part (an image, a file) counts 85;
/// - every `summary[].text` string; a `summary` that is not an array counts its JSON length;
/// - `arguments` and `input` strings;
/// - `output`: a string by its bytes, any other value by its serialized JSON length;
/// - an item with none of these fields: its serialized JSON length.
///
/// A `null` or absent field counts 0. Any other shape (a non-object item, an untyped part, a
/// non-string `instructions`, `arguments` or `input`) falls back to `ceil(body_len / 4)`, capped
/// at 32768. Over-estimating is the safe direction: this estimate is the charge when no reported
/// total arrives.
pub fn request_tokens_estimate(v: &Value, body_len: usize) -> u64 {
    let fallback = bytes_to_tokens(body_len).min(FALLBACK_REQUEST_CAP);
    let mut text_bytes = 0usize;
    let mut other_parts = 0u64;
    if !add_text(v.get("instructions"), &mut text_bytes) {
        return fallback;
    }
    match v.get("input") {
        Some(Value::Array(items)) => {
            for item in items {
                let Some(item) = item.as_object() else { return fallback };
                if KNOWN_ITEM_FIELDS.iter().all(|field| item.get(*field).is_none_or(Value::is_null)) {
                    text_bytes = text_bytes.saturating_add(serde_json::to_vec(item).map_or(0, |bytes| bytes.len()));
                    continue;
                }
                for field in ["arguments", "input"] {
                    if !add_text(item.get(field), &mut text_bytes) {
                        return fallback;
                    }
                }
                match item.get("output") {
                    None | Some(Value::Null) => {}
                    Some(Value::String(text)) => text_bytes = text_bytes.saturating_add(text.len()),
                    Some(other) => text_bytes = text_bytes.saturating_add(json_len(other)),
                }
                match item.get("summary") {
                    None | Some(Value::Null) => {}
                    Some(Value::Array(parts)) => {
                        for text in parts.iter().filter_map(|part| part.get("text").and_then(Value::as_str)) {
                            text_bytes = text_bytes.saturating_add(text.len());
                        }
                    }
                    Some(other) => text_bytes = text_bytes.saturating_add(json_len(other)),
                }
                match item.get("content") {
                    Some(Value::Array(parts)) => {
                        for part in parts {
                            let text = part.get("text").and_then(Value::as_str).or_else(|| part.get("refusal").and_then(Value::as_str));
                            match (part.get("type").and_then(Value::as_str), text) {
                                (Some(_), Some(text)) => text_bytes = text_bytes.saturating_add(text.len()),
                                (Some(_), None) => other_parts = other_parts.saturating_add(1),
                                (None, _) => return fallback,
                            }
                        }
                    }
                    content => {
                        if !add_text(content, &mut text_bytes) {
                            return fallback;
                        }
                    }
                }
            }
        }
        input => {
            if !add_text(input, &mut text_bytes) {
                return fallback;
            }
        }
    }
    bytes_to_tokens(text_bytes).saturating_add(other_parts.saturating_mul(NON_TEXT_PART_TOKENS))
}

/// The serialized JSON length of a value, for the byte rule.
fn json_len(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(0, |bytes| bytes.len())
}

/// Adds the bytes of a string field. A `null` or absent field adds 0. Any other shape returns
/// `false`: the caller uses the fallback.
fn add_text(field: Option<&Value>, text_bytes: &mut usize) -> bool {
    match field {
        None | Some(Value::Null) => true,
        Some(Value::String(text)) => {
            *text_bytes = text_bytes.saturating_add(text.len());
            true
        }
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::usage::{StreamUsage, UsageScanner};
    use super::*;
    use serde_json::json;

    fn scan(chunks: &[&[u8]]) -> ResponsesUsageScanner {
        let mut scanner = ResponsesUsageScanner::new();
        for chunk in chunks {
            scanner.feed(chunk);
        }
        scanner
    }

    fn scan_str(input: &str) -> ResponsesUsageScanner {
        scan(&[input.as_bytes()])
    }

    fn result(scanner: &ResponsesUsageScanner) -> (u64, Option<u64>) {
        (scanner.completion_records(), scanner.reported_total())
    }

    /// One SSE event with the real `event:` line.
    fn event(data: &str) -> String {
        let kind = serde_json::from_str::<Value>(data)
            .ok()
            .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default();
        format!("event: {kind}\ndata: {data}\n\n")
    }

    /// A real-shaped terminal event (spec § 8.2), with `padding` bytes of `instructions` before
    /// `usage`.
    fn terminal(kind: &str, total: u64, padding: usize) -> String {
        let instructions = "x".repeat(padding);
        event(&format!(
            "{{\"type\":\"{kind}\",\"sequence_number\":9,\"response\":{{\"id\":\"resp_1\",\"object\":\"response\",\"created_at\":1790942400,\"status\":\"completed\",\"instructions\":\"{instructions}\",\"metadata\":{{}},\"output\":[{{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"Hi\",\"annotations\":[]}}]}}],\"usage\":{{\"input_tokens\":30,\"input_tokens_details\":{{\"cached_tokens\":0}},\"output_tokens\":12,\"output_tokens_details\":{{\"reasoning_tokens\":0}},\"total_tokens\":{total}}}}}}}"
        ))
    }

    fn delta(text: &str) -> String {
        event(&format!(
            "{{\"type\":\"response.output_text.delta\",\"sequence_number\":3,\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"{text}\"}}"
        ))
    }

    #[test]
    fn a_completed_event_reports_its_total() {
        for kind in ["response.completed", "response.incomplete", "response.failed"] {
            assert_eq!(result(&scan_str(&terminal(kind, 42, 0))), (0, Some(42)), "{kind}");
        }
        let stream = format!("{}{}{}", delta("Hi"), terminal("response.completed", 7, 0), terminal("response.failed", 9, 0));
        assert_eq!(result(&scan_str(&stream)), (1, Some(9)), "the last value wins");
        assert_eq!(scan_str(&terminal("response.created", 42, 0)).reported_total(), None, "a non-terminal type is not read");
    }

    /// AC 10: the scanner reads a terminal event of any size and holds only its fixed state.
    fn assert_large_terminal_is_read(padding: usize, chunk_len: usize) {
        let stream = terminal("response.completed", 4_242, padding);
        assert!(stream.len() > padding);
        let mut scanner = ResponsesUsageScanner::new();
        for chunk in stream.as_bytes().chunks(chunk_len) {
            scanner.feed(chunk);
            assert!(scanner.held_bytes() < 1024, "held {} bytes", scanner.held_bytes());
        }
        assert_eq!(result(&scanner), (0, Some(4_242)));
    }

    #[test]
    fn a_terminal_event_over_64_kib_is_read() {
        assert_large_terminal_is_read(70 * 1024, 1000);
    }

    #[test]
    fn a_terminal_event_over_1_mib_is_read() {
        assert_large_terminal_is_read(2 * 1024 * 1024, 1000);
    }

    /// Ruling F13: above the 16 MiB record limit of the hold, usage is still read (interception
    /// off forwards such a record, and it is still charged).
    #[test]
    fn a_terminal_event_over_16_mib_is_read() {
        assert_large_terminal_is_read(17 * 1024 * 1024, 64 * 1024);
    }

    #[test]
    fn usage_keys_elsewhere_do_not_count() {
        let usage = "\"usage\":{\"total_tokens\":5}";
        for data in [
            // Inside `response.metadata`.
            format!("{{\"type\":\"response.completed\",\"response\":{{\"metadata\":{{{usage}}}}}}}"),
            // Inside an output item.
            format!("{{\"type\":\"response.completed\",\"response\":{{\"output\":[{{\"type\":\"message\",{usage}}}]}}}}"),
            // At the top level of a terminal record.
            format!("{{\"type\":\"response.completed\",{usage},\"response\":{{}}}}"),
            // At the top level and under `response` of a delta record.
            format!("{{\"type\":\"response.output_text.delta\",{usage},\"response\":{{{usage}}}}}"),
            // One level too deep.
            "{\"type\":\"response.completed\",\"response\":{\"usage\":{\"x\":{\"total_tokens\":5}}}}".to_owned(),
            // `response` is an array.
            format!("{{\"type\":\"response.completed\",\"response\":[{{{usage}}}]}}"),
            // A later `type` decides: the type is read when the record ends.
            format!("{{\"type\":\"response.completed\",\"response\":{{{usage}}},\"type\":\"response.output_text.delta\"}}"),
            // A later `type` that is not a string is not terminal.
            format!("{{\"type\":\"response.completed\",\"response\":{{{usage}}},\"type\":null}}"),
        ] {
            assert_eq!(result(&scan_str(&event(&data))), (1, None), "{data}");
        }
        // A `type` key after the `response` key still decides.
        let late_type = format!("{{\"response\":{{{usage}}},\"type\":\"response.completed\"}}");
        assert_eq!(result(&scan_str(&event(&late_type))), (0, Some(5)));
        // The last duplicate key wins at every level, as in serde_json.
        for (data, want) in [
            ("{\"type\":\"response.completed\",\"response\":{\"usage\":{\"total_tokens\":5,\"total_tokens\":6}}}", Some(6)),
            ("{\"type\":\"response.completed\",\"response\":{\"usage\":{\"total_tokens\":5},\"usage\":{}}}", None),
            ("{\"type\":\"response.completed\",\"response\":{\"usage\":{\"total_tokens\":5}},\"response\":{}}", None),
        ] {
            assert_eq!(scan_str(&event(data)).reported_total(), want, "{data}");
            let serde = serde_json::from_str::<Value>(data).unwrap().pointer("/response/usage/total_tokens").and_then(Value::as_u64);
            assert_eq!(serde, want, "serde_json agrees: {data}");
        }
    }

    #[test]
    fn strings_with_braces_and_escapes_do_not_confuse_the_scan() {
        let tricky = r#"}\" \\ {\"usage\":{\"total_tokens\":1}} \\\" ] é \/ \n"#;
        let data = format!("{{\"type\":\"response.completed\",\"response\":{{\"instructions\":\"{tricky}\",\"usage\":{{\"total_tokens\":42}}}}}}");
        assert!(serde_json::from_str::<Value>(&data).is_ok(), "the fixture is valid JSON");
        assert_eq!(result(&scan_str(&event(&data))), (0, Some(42)));
        // A key with an escape matches nothing, so the estimate applies.
        let escaped_key = "{\"type\":\"response.completed\",\"response\":{\"us\\u0061ge\":{\"total_tokens\":42}}}";
        assert_eq!(result(&scan_str(&event(escaped_key))), (1, None));
    }

    #[test]
    fn other_records_over_64_kib_are_skipped() {
        let big = format!(
            "{{\"type\":\"response.output_text.delta\",\"delta\":\"{}\",\"response\":{{\"usage\":{{\"total_tokens\":5}}}}}}",
            "y".repeat(70 * 1024)
        );
        let stream = format!("{}{}", event(&big), delta("next"));
        let mut scanner = ResponsesUsageScanner::new();
        for chunk in stream.as_bytes().chunks(1000) {
            scanner.feed(chunk);
            assert!(scanner.held_bytes() < 1024);
        }
        assert_eq!(result(&scanner), (2, None), "the big record counts as one and gives no usage");
    }

    #[test]
    fn one_token_per_data_record() {
        // The SSE line rules are those of the chat scanner.
        for input in [
            "data: a\n\ndata: b\n\n",
            "data: a\r\n\r\ndata: b\r\n\r\n",
            "data: a\r\rdata: b\r\r",
            "data: a\r\n\ndata: b\n\r\n",
            "\n\n: keep-alive\n\ndata: [DONE]\n\n",
            "event: x\n\ndata:\n\ndata:a\ndata:b\n\n",
            "data\n\ndat\n\nxdata: a\n\n",
        ] {
            let mut chat = UsageScanner::new();
            chat.feed(input.as_bytes());
            assert_eq!(scan_str(input).completion_records(), chat.completion_records(), "{input:?}");
        }
        assert_eq!(scan(&[b"data: a\r", b"\n\r", b"\n"]).completion_records(), 1);
        assert_eq!(scan(&[b"data: a\r\n\r", b"\ndata: b\r\n\r\n"]).completion_records(), 2);
        // Each delta, `response.created` and a terminal without usage count one; comments and
        // event-only records count nothing.
        let created = event("{\"type\":\"response.created\",\"response\":{\"usage\":null}}");
        let no_usage = event("{\"type\":\"response.completed\",\"response\":{\"usage\":null}}");
        let stream = format!("{created}: ping\n\n{}event: x\n\n{}{no_usage}", delta("a"), delta("b"));
        assert_eq!(result(&scan_str(&stream)), (4, None));
        // Byte by byte, a record counts once and a terminal is still read.
        let stream = format!("{}{}", delta("a"), terminal("response.completed", 3, 10));
        let mut scanner = ResponsesUsageScanner::new();
        for byte in stream.as_bytes() {
            scanner.feed(std::slice::from_ref(byte));
        }
        assert_eq!(result(&scanner), (1, Some(3)));
        // `finish()` reads a last terminal record with no blank line, once.
        let body = terminal("response.completed", 8, 0);
        let mut scanner = scan_str(body.trim_end_matches('\n'));
        assert_eq!(scanner.reported_total(), None, "not read before finish");
        scanner.finish();
        scanner.finish();
        assert_eq!(result(&scanner), (0, Some(8)));
    }

    /// The scan reads untrusted bytes that drive billing: a total that is not a plain `u64` is
    /// not charged (the estimate applies), and no record panics.
    #[test]
    fn a_total_that_is_not_a_u64_is_not_charged() {
        let with_total = |total: &str| format!("{{\"type\":\"response.completed\",\"response\":{{\"usage\":{{\"total_tokens\":{total}}}}}}}");
        for total in [
            "-1",
            "-0",
            "1.0",
            "1e3",
            "1E3",
            "0.5",
            "18446744073709551616",
            "100000000000000000000",
            "\"42\"",
            "null",
            "true",
            "{}",
            "[42]",
        ] {
            let scanner = scan_str(&event(&with_total(total)));
            assert_eq!(result(&scanner), (1, None), "{total}");
        }
        assert_eq!(scan_str(&event(&with_total("18446744073709551615"))).reported_total(), Some(u64::MAX));
        assert_eq!(scan_str(&event(&with_total("0"))).reported_total(), Some(0));
        // A bad later terminal does not undo an earlier good one.
        let stream = format!("{}{}", terminal("response.completed", 7, 0), event(&with_total("-1")));
        assert_eq!(result(&scan_str(&stream)), (1, Some(7)));
    }

    #[test]
    fn a_malformed_record_is_not_charged() {
        let good = "{\"type\":\"response.completed\",\"response\":{\"usage\":{\"total_tokens\":5}}}";
        assert_eq!(scan_str(&event(good)).reported_total(), Some(5));
        // 128 containers: one more than serde_json accepts.
        let deep = format!("{}{}", "[".repeat(127), "]".repeat(127));
        let cases = [
            good.trim_end_matches('}').to_owned(),
            good[..good.len() - 1].to_owned(),
            good.replace("5}}}", "5},}}"),
            String::new(),
            format!("{good}}}"),
            format!("{good}x"),
            format!("{good}{good}"),
            good.replace("\"usage\":", "\"usage\""),
            good.replace(",\"response\"", "\"response\""),
            good.replace("5}", "5,}"),
            good.replace("5", "05"),
            good.replace("{\"usage", "[\"usage"),
            good.replace("\"type\"", "\"type"),
            good.replace("\"type\"", "\"ty\\qpe\""),
            good.replace("\"type\"", "\"ty\\u00Gpe\""),
            good.replace("\"type\"", "\"ty\u{1}pe\""),
            good.replace("{\"usage", "{\"x\":tru,\"usage"),
            good.replace("{\"usage", "{\"x\":nulll,\"usage"),
            good.replace("{\"usage", "{\"x\":1.,\"usage"),
            good.replace("{\"usage", "{\"x\":1e,\"usage"),
            good.replace("{\"usage", "{\"x\":-,\"usage"),
            good.replace("{\"usage", "{\"x\":+1,\"usage"),
            format!("{{\"a\":{deep},{}", &good[1..]),
        ];
        for data in &cases {
            assert!(serde_json::from_str::<Value>(data).is_err(), "the fixture is invalid JSON: {data:.80}");
            assert_eq!(result(&scan_str(&event(data))), (1, None), "{data:.80}");
        }
        // A string value split over two data lines holds a raw LF, which JSON forbids.
        let split = "data: {\"type\":\"response.completed\",\"response\":{\"instructions\":\"a\ndata: b\",\"usage\":{\"total_tokens\":5}}}\n\n";
        assert_eq!(result(&scan_str(split)), (1, None));
        // Structure split over two data lines is joined with LF, which is whitespace.
        let lines = "data: {\"type\":\"response.completed\",\ndata: \"response\":{\"usage\":{\"total_tokens\":5}}}\n\n";
        assert_eq!(result(&scan_str(lines)), (0, Some(5)));
        // 127 containers in all are still read.
        let nested = format!(
            "{{\"type\":\"response.completed\",\"a\":{}{},\"response\":{{\"usage\":{{\"total_tokens\":5}}}}}}",
            "[".repeat(126),
            "]".repeat(126)
        );
        assert!(serde_json::from_str::<Value>(&nested).is_ok());
        assert_eq!(scan_str(&event(&nested)).reported_total(), Some(5));
        // Arbitrary bytes do not panic.
        let mut noise = ResponsesUsageScanner::new();
        let bytes: Vec<u8> = (0..=255u8).cycle().take(64 * 1024).collect();
        noise.feed(b"data: ");
        noise.feed(&bytes);
        noise.finish();
        assert_eq!(noise.reported_total(), None);
    }

    #[test]
    fn a_long_type_is_not_terminal() {
        let long = format!("response.completed{}", " ".repeat(MAX_TYPE_BYTES));
        let data = format!("{{\"type\":\"{long}\",\"response\":{{\"usage\":{{\"total_tokens\":5}}}}}}");
        let scanner = scan_str(&event(&data));
        assert_eq!(result(&scanner), (1, None));
        assert!(scanner.held_bytes() < 1024);
        // An escaped type is not read either.
        let escaped = "{\"type\":\"response.complet\\u0065d\",\"response\":{\"usage\":{\"total_tokens\":5}}}";
        assert_eq!(result(&scan_str(&event(escaped))), (1, None));
    }

    #[test]
    fn stream_usage_reads_a_responses_stream() {
        let mut usage = StreamUsage::Responses(ResponsesUsageScanner::new());
        usage.feed(delta("a").as_bytes());
        usage.feed(terminal("response.completed", 11, 0).trim_end_matches('\n').as_bytes());
        usage.finish();
        assert_eq!((usage.completion_records(), usage.reported_total()), (1, Some(11)));
    }

    #[test]
    fn the_body_reports_usage_or_estimates() {
        assert_eq!(non_stream_usage(br#"{"object":"response","output":[],"usage":{"total_tokens":12}}"#), BodyUsage::Reported(12));
        // 8 text bytes + 7 argument bytes + 5 input bytes → ceil(20 / 4) = 5.
        let body = json!({
            "object": "response",
            "output": [
                {"type": "message", "content": [{"type": "output_text", "text": "abcdefgh"}, {"type": "refusal", "refusal": "no"}]},
                {"type": "function_call", "name": "f", "arguments": "{\"a\":1}"},
                {"type": "custom_tool_call", "name": "g", "input": "hello"},
                {"type": "reasoning", "summary": []},
                "not an item"
            ],
            "usage": null
        });
        assert_eq!(non_stream_usage(body.to_string().as_bytes()), BodyUsage::Completion(5));
        assert_eq!(non_stream_usage(b"not json"), BodyUsage::Completion(0));
        assert_eq!(non_stream_usage(br#"{"usage":{"total_tokens":-3},"output":[]}"#), BodyUsage::Completion(0));
        assert_eq!(non_stream_usage(br#"{"usage":{"total_tokens":1.5},"output":[]}"#), BodyUsage::Completion(0));
    }

    #[test]
    fn the_request_estimate_follows_the_chat_rule() {
        let est = |v: Value, body_len: usize| request_tokens_estimate(&v, body_len);
        // `instructions` and a string `input`: 5 + 3 bytes → 2.
        assert_eq!(est(json!({"model": "m", "instructions": "hello", "input": "abc"}), 999), 2);
        assert_eq!(est(json!({"model": "m"}), 999), 0);
        // Items: string content (4), `input_text` parts (4) and an image part (85), `arguments`
        // (3), `input` (2) and `output` (3) → ceil(16 / 4) + 85 = 89.
        let body = json!({"input": [
            {"role": "user", "content": "abcd"},
            {"role": "user", "content": [{"type": "input_text", "text": "efgh"}, {"type": "input_image", "image_url": "data:image/png;base64,AAAA"}]},
            {"type": "function_call", "call_id": "c1", "name": "f", "arguments": "{} "},
            {"type": "custom_tool_call", "call_id": "c2", "name": "g", "input": "hi"},
            {"type": "function_call_output", "call_id": "c1", "output": "ok!"},
            {"type": "reasoning", "summary": [], "content": null}
        ]});
        assert_eq!(est(body, 999), 89);
        // Any other shape falls back to ceil(body_len / 4), capped at 32768.
        for body in [
            json!({"input": ["not an object"]}),
            json!({"input": 42}),
            json!({"instructions": ["x"], "input": "abc"}),
            json!({"input": [{"role": "user", "content": 42}]}),
            json!({"input": [{"role": "user", "content": [{"text": "no type"}]}]}),
            json!({"input": [{"type": "function_call", "arguments": {"a": 1}}]}),
        ] {
            assert_eq!(est(body.clone(), 401), 101, "{body}");
        }
        assert_eq!(est(json!({"input": 42}), 1_000_000), 32_768);
    }

    /// Fix I1 (coordinator ruling): the chat rule counts the text of every role, so a replayed
    /// history is charged by its size, also when the client drops the stream.
    #[test]
    fn the_request_estimate_counts_every_replayed_text() {
        let est = |v: Value| request_tokens_estimate(&v, 999);
        const MIB: usize = 1024 * 1024;
        let big = |n: usize| "x".repeat(n);
        // A 4 MiB `output_text` part of an assistant message.
        let assistant = |n: usize| json!({"input": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": big(n), "annotations": []}]}]});
        assert_eq!(est(assistant(4 * MIB)), (4 * MIB / 4) as u64);
        // A reasoning item: every `summary[].text` and every `content[].text`.
        let reasoning = |n: usize| json!({"input": [{"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": big(n)}, {"type": "summary_text", "text": big(n)}], "content": [{"type": "reasoning_text", "text": big(n)}]}]});
        assert_eq!(est(reasoning(MIB)), (3 * MIB / 4) as u64);
        // A `summary` that is not an array counts its JSON length.
        assert_eq!(est(json!({"input": [{"type": "reasoning", "summary": {"text": "abcd"}}]})), 4);
        // A refusal part counts its `refusal` string.
        let refusal = |n: usize| json!({"input": [{"type": "message", "role": "assistant", "content": [{"type": "refusal", "refusal": big(n)}]}]});
        assert_eq!(est(refusal(MIB)), (MIB / 4) as u64);
        assert_eq!(est(refusal(9)), 3);
        // A non-string `output` counts its serialized JSON length.
        let output = |n: usize| json!({"input": [{"type": "function_call_output", "call_id": "c1", "output": [{"type": "input_text", "text": big(n)}]}]});
        let small = r#"[{"type":"input_text","text":"x"}]"#;
        assert_eq!(est(output(1)), small.len().div_ceil(4) as u64);
        assert_eq!(est(output(MIB)), (small.len() - 1 + MIB).div_ceil(4) as u64);
        // An item with no known field counts its serialized JSON length.
        let unknown = |n: usize| json!({"type": "item_reference", "id": big(n)});
        let unknown_len = |n: usize| serde_json::to_vec(&unknown(n)).unwrap().len();
        assert_eq!(est(json!({"input": [unknown(MIB)]})), unknown_len(MIB).div_ceil(4) as u64);
        let null_fields = json!({"type": "x", "content": null, "output": null, "data": big(MIB)});
        let null_len = serde_json::to_vec(&null_fields).unwrap().len();
        assert_eq!(est(json!({"input": [null_fields]})), null_len.div_ceil(4) as u64, "null fields are not known fields");
        // Every one of them scales with its size.
        let shapes: [&dyn Fn(usize) -> Value; 4] = [&assistant, &reasoning, &refusal, &output];
        for shape in shapes {
            assert!(est(shape(2 * MIB)) > est(shape(MIB)) + (MIB / 8) as u64);
        }
        // A non-text part keeps the fixed count; text parts of any type add their bytes.
        let mixed = json!({"input": [{"role": "user", "content": [{"type": "input_file", "file_id": "f"}, {"type": "summary_text", "text": "abcd"}]}]});
        assert_eq!(est(mixed), 1 + NON_TEXT_PART_TOKENS);
    }
}
