// SPDX-License-Identifier: Apache-2.0
//! SSE framing (spec D2–D4).
//!
//! Input follows WHATWG HTML § 9.2.6 line splitting, with one difference: the last record is
//! dispatched at the end of the stream even without a blank line (D4), as `UsageScanner::finish`
//! does. Output is ALWAYS re-framed by [`frame`]: no upstream byte reaches the client unless the
//! record that carried it was classified (D3). So a BOM, a lax line or an unknown field cannot
//! smuggle a record past the classifier into a client that reads SSE differently.
//!
//! Unlike `UsageScanner`, nothing is ever skipped silently. A record over the cap becomes
//! [`SseItem::Oversized`], which the interceptor treats as a stream-level failure (D8).

use bytes::Bytes;

/// One dispatched SSE record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Record {
    pub event: Option<String>,
    /// The joined `data` lines, without the final `\n`. Never empty (P-2).
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SseItem {
    Record(Record),
    /// A record passed `max_record_bytes`. Its bytes were discarded (P-3).
    Oversized,
}

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The SSE comment sent while anything is held (D15).
pub(crate) const KEEP_ALIVE: &[u8] = b": keep-alive\n\n";

enum BomState {
    /// Bytes seen so far that could still be a BOM.
    Pending(Vec<u8>),
    Done,
}

pub(crate) struct SseSplitter {
    max_record_bytes: usize,
    bom: BomState,
    line: Vec<u8>,
    /// Bytes in the current line, counted even while skipping.
    line_bytes: usize,
    after_cr: bool,
    record_bytes: usize,
    data: Vec<u8>,
    has_data: bool,
    event: Option<String>,
    skipping: bool,
}

impl SseSplitter {
    pub(crate) fn new(max_record_bytes: usize) -> Self {
        SseSplitter {
            max_record_bytes,
            bom: BomState::Pending(Vec::with_capacity(BOM.len())),
            line: Vec::new(),
            line_bytes: 0,
            after_cr: false,
            record_bytes: 0,
            data: Vec::new(),
            has_data: false,
            event: None,
            skipping: false,
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<SseItem> {
        let mut out = Vec::new();
        if let BomState::Pending(buf) = &mut self.bom {
            buf.extend_from_slice(chunk);
            if buf.len() < BOM.len() && BOM.starts_with(buf) {
                return out;
            }
            let mut head = std::mem::take(buf);
            self.bom = BomState::Done;
            if head.starts_with(BOM) {
                head.drain(..BOM.len());
            }
            for &b in &head {
                self.byte(b, &mut out);
            }
            return out;
        }
        for &b in chunk {
            self.byte(b, &mut out);
        }
        out
    }

    /// The upstream ended: close the pending line, then dispatch the pending record (D4).
    pub(crate) fn finish(&mut self) -> Vec<SseItem> {
        let mut out = Vec::new();
        if let BomState::Pending(buf) = &mut self.bom {
            let head = std::mem::take(buf);
            self.bom = BomState::Done;
            for &b in &head {
                self.byte(b, &mut out);
            }
        }
        if self.line_bytes > 0 {
            self.end_line(&mut out);
        }
        self.after_cr = false;
        if self.skipping {
            self.skipping = false;
            self.reset_record();
        } else {
            self.dispatch(&mut out);
        }
        out
    }

    fn byte(&mut self, b: u8, out: &mut Vec<SseItem>) {
        if self.after_cr {
            self.after_cr = false;
            if b == b'\n' {
                return;
            }
        }
        match b {
            b'\r' => {
                self.after_cr = true;
                self.end_line(out);
            }
            b'\n' => self.end_line(out),
            _ => {
                self.line_bytes += 1;
                self.record_bytes += 1;
                if self.skipping {
                    return;
                }
                if self.record_bytes > self.max_record_bytes {
                    self.skipping = true;
                    self.line.clear();
                    self.reset_record();
                    out.push(SseItem::Oversized);
                    return;
                }
                self.line.push(b);
            }
        }
    }

    fn end_line(&mut self, out: &mut Vec<SseItem>) {
        let blank = self.line_bytes == 0;
        self.line_bytes = 0;
        let line = std::mem::take(&mut self.line);
        if blank {
            if self.skipping {
                self.skipping = false;
                self.reset_record();
            } else {
                self.dispatch(out);
            }
            return;
        }
        if !self.skipping {
            self.field(&line);
        }
    }

    fn field(&mut self, line: &[u8]) {
        if line.first() == Some(&b':') {
            return; // a comment (dropped, D3)
        }
        let (name, value) = match line.iter().position(|&b| b == b':') {
            Some(i) => {
                let v = &line[i + 1..];
                (&line[..i], v.strip_prefix(b" ").unwrap_or(v))
            }
            None => (line, &b""[..]),
        };
        match name {
            b"data" => {
                self.data.extend_from_slice(value);
                self.data.push(b'\n');
                self.has_data = true;
            }
            b"event" => self.event = Some(String::from_utf8_lossy(value).into_owned()),
            _ => {} // `id`, `retry` and unknown fields are dropped (D3)
        }
    }

    fn dispatch(&mut self, out: &mut Vec<SseItem>) {
        if self.has_data {
            let mut data = std::mem::take(&mut self.data);
            data.pop();
            if !data.is_empty() {
                out.push(SseItem::Record(Record { event: self.event.take(), data }));
            }
        }
        self.reset_record();
    }

    fn reset_record(&mut self) {
        self.data.clear();
        self.has_data = false;
        self.event = None;
        self.record_bytes = 0;
    }
}

/// Re-frame one record (D3): an optional `event:` line, one `data:` line per payload line, and the
/// blank line. The payload cannot hold a CR: the splitter ended a line at every CR.
pub(crate) fn frame(event: Option<&str>, payload: &[u8]) -> Bytes {
    let mut out = Vec::with_capacity(payload.len() + 24);
    if let Some(e) = event {
        out.extend_from_slice(b"event: ");
        out.extend_from_slice(e.as_bytes());
        out.push(b'\n');
    }
    for line in payload.split(|&b| b == b'\n') {
        out.extend_from_slice(b"data: ");
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    out.push(b'\n');
    Bytes::from(out)
}

/// One SSE comment. A CR or LF in `text` would end the comment early, so each becomes a space.
pub(crate) fn comment(text: &str) -> Bytes {
    let clean: String = text.chars().map(|c| if c == '\r' || c == '\n' { ' ' } else { c }).collect();
    Bytes::from(format!(": {clean}\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(items: Vec<SseItem>) -> Vec<(Option<String>, String)> {
        items
            .into_iter()
            .map(|i| match i {
                SseItem::Record(r) => (r.event, String::from_utf8(r.data).unwrap()),
                SseItem::Oversized => (None, "<oversized>".to_owned()),
            })
            .collect()
    }

    fn split_all(input: &[u8], max: usize) -> Vec<(Option<String>, String)> {
        let mut s = SseSplitter::new(max);
        let mut out = s.push(input);
        out.extend(s.finish());
        records(out)
    }

    #[test]
    fn all_three_line_endings_end_a_record() {
        for input in [&b"data: a\n\ndata: b\n\n"[..], b"data: a\r\n\r\ndata: b\r\n\r\n", b"data: a\r\rdata: b\r\r"] {
            assert_eq!(split_all(input, 1024), vec![(None, "a".into()), (None, "b".into())], "{input:?}");
        }
    }

    #[test]
    fn every_split_point_gives_the_same_records() {
        let input = b"\xEF\xBB\xBFevent: e\r\ndata: {\"x\":1}\r\n\r\n: c\ndata: l1\ndata: l2\n\ndata: tail";
        let whole = split_all(input, 1024);
        assert_eq!(whole, vec![(Some("e".into()), "{\"x\":1}".into()), (None, "l1\nl2".into()), (None, "tail".into())]);
        for i in 0..=input.len() {
            for j in i..=input.len() {
                let mut s = SseSplitter::new(1024);
                let mut out = s.push(&input[..i]);
                out.extend(s.push(&input[i..j]));
                out.extend(s.push(&input[j..]));
                out.extend(s.finish());
                assert_eq!(records(out), whole, "split at {i},{j}");
            }
        }
    }

    #[test]
    fn a_bom_is_stripped_once_and_only_at_the_start() {
        assert_eq!(split_all(b"\xEF\xBB\xBFdata: a\n\n", 64), vec![(None, "a".into())]);
        // A second BOM is part of the field name, so the line is an unknown field and is dropped.
        assert_eq!(split_all(b"\xEF\xBB\xBF\xEF\xBB\xBFdata: a\n\n", 64), vec![]);
        // A BOM-like prefix that is not a BOM is kept.
        assert_eq!(split_all(b"\xEF\xBBdata: a\n\n", 64), vec![]);
    }

    #[test]
    fn comments_ids_retries_and_unknown_fields_are_dropped() {
        assert_eq!(split_all(b": hi\nid: 7\nretry: 10\nfoo: bar\ndata: a\n\n", 64), vec![(None, "a".into())]);
    }

    #[test]
    fn a_lax_line_is_an_unknown_field_not_data() {
        // `  data: x` and `Data: x` are what a trimming or case-blind client would read as data.
        assert_eq!(split_all(b"  data: x\n\nData: y\n\n", 64), vec![]);
    }

    #[test]
    fn data_without_a_space_and_with_two_spaces() {
        assert_eq!(split_all(b"data:a\n\ndata:  b\n\n", 64), vec![(None, "a".into()), (None, " b".into())]);
    }

    #[test]
    fn an_empty_data_buffer_dispatches_nothing() {
        assert_eq!(split_all(b"data:\n\nevent: x\n\n", 64), vec![]);
    }

    #[test]
    fn the_unterminated_tail_is_dispatched_by_finish() {
        let mut s = SseSplitter::new(64);
        assert!(s.push(b"data: a").is_empty());
        assert_eq!(records(s.finish()), vec![(None, "a".into())]);
    }

    #[test]
    fn an_oversized_record_is_reported_once_and_the_next_record_survives() {
        let mut input = b"data: ".to_vec();
        input.extend(std::iter::repeat_n(b'x', 100));
        input.extend_from_slice(b"\ndata: more\n\ndata: ok\n\n");
        assert_eq!(split_all(&input, 32), vec![(None, "<oversized>".into()), (None, "ok".into())]);
    }

    #[test]
    fn frame_writes_one_data_line_per_payload_line() {
        assert_eq!(&frame(None, b"{}")[..], b"data: {}\n\n");
        assert_eq!(&frame(Some("ping"), b"a\nb")[..], b"event: ping\ndata: a\ndata: b\n\n");
    }

    #[test]
    fn comment_never_breaks_the_frame() {
        assert_eq!(&comment("x")[..], b": x\n\n");
        assert_eq!(&comment("a\r\nb")[..], b": a  b\n\n");
        assert_eq!(KEEP_ALIVE, b": keep-alive\n\n");
    }
}
