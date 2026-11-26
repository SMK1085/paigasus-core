// SPDX-License-Identifier: Apache-2.0
//! The request screen (SMA-855 § 6.8.1): the text of each tool result and the hosts of the seen
//! message sources, read by the one format walker of `turn.rs` (S13). No I/O.
//!
//! The screen gate (§ 6.8.0, D27) and the seen hosts (D14) live here too. The result texts are
//! dropped when the request side is done; only the seen-host set goes on, in the snapshot.

use std::fmt;
use std::sync::Arc;

use paigasus_kernel::detect::{Finding, HostSet, Severity, hosts_in_text};

use crate::policy::{PolicySet, PolicySnapshot, TenantMode, TenantPart};
use crate::turn::{TextSink, TurnFacts, parse_value, walk};
use crate::types::Format;

/// What the request screen read. `Debug` prints counts only (§ 6.2.4).
#[non_exhaustive]
pub struct RequestScreen {
    /// One text per entry of `TurnFacts.tool_results`, in the same order. `None` when the result
    /// has no text (a screenshot, an approval), or when it did not fit in the scan budget.
    pub result_texts: Vec<Option<Arc<str>>>,
    /// The hosts of the seen message sources (§ 6.3.3).
    pub message_hosts: HostSet,
    /// The indexes of the results that did not fit in the scan budget, in scan order (P-3). Each
    /// one gets a `detector_error` finding (rule `scan_limit`, § 6.10).
    pub limited: Vec<usize>,
    /// The indexes of the results that hold content the screen cannot read as text (an object
    /// `content`, an Anthropic `document` or `search_result` block, a Responses `input_file`), in
    /// result order. Each one gets a `detector_error` finding: it must not pass unscanned and
    /// unreported. The text parts of such a result that the screen can read are still scanned.
    pub unscanned: Vec<usize>,
}

impl fmt::Debug for RequestScreen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RequestScreen {{ results: {}, texts: {}, message_hosts: {}, limited: {}, unscanned: {} }}",
            self.result_texts.len(),
            self.result_texts.iter().filter(|t| t.is_some()).count(),
            self.message_hosts.len(),
            self.limited.len(),
            self.unscanned.len()
        )
    }
}

/// Why the request screen failed. While `parse_request` gave `ToolTurn::Parsed`, either one gives
/// every result a `detector_error` finding with detector `built_in` (S13).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenError {
    /// The body did not parse.
    Malformed,
    /// The results that the screen saw differ from `TurnFacts.tool_results`.
    KeyMismatch,
}

/// Screen a request body. `turn` must be the `ToolTurn::Parsed` facts of the same body.
///
/// The budget (§ 6.10, S12): the result texts first, newest first, then the message texts in
/// order. A result that does not fit whole goes to `limited`; a message that does not fit whole is
/// not scanned for hosts. The check is a byte count, so no slice is made.
///
/// # Errors
///
/// `Malformed` when the body does not parse, `KeyMismatch` when the walker's results differ from
/// `turn.tool_results`.
pub fn screen_request(format: Format, body: &[u8], turn: &TurnFacts, max_scan_bytes: usize) -> Result<RequestScreen, ScreenError> {
    let value = parse_value(format, body).map_err(|_| ScreenError::Malformed)?;
    let mut sink = Collect::default();
    let facts = walk(format, &value, Some(&mut sink)).map_err(|_| ScreenError::Malformed)?;
    if facts.tool_results != turn.tool_results {
        return Err(ScreenError::KeyMismatch);
    }
    let mut parts: Vec<Vec<&str>> = vec![Vec::new(); facts.tool_results.len()];
    let mut unscanned = Vec::new();
    for (index, p, complete) in sink.results {
        if let Some(slot) = parts.get_mut(index) {
            *slot = p;
            if !complete {
                unscanned.push(index);
            }
        }
    }
    let mut left = max_scan_bytes;
    let mut result_texts: Vec<Option<Arc<str>>> = vec![None; parts.len()];
    let mut limited = Vec::new();
    for (index, p) in parts.iter().enumerate().rev() {
        if p.is_empty() {
            continue;
        }
        // The joined length: every part and one "\n" between two parts.
        let size = p.iter().fold(p.len() - 1, |n, t| n.saturating_add(t.len()));
        if size <= left {
            left -= size;
            result_texts[index] = Some(Arc::from(p.join("\n")));
        } else {
            limited.push(index);
        }
    }
    let mut message_hosts = HostSet::new();
    for text in sink.messages {
        if text.len() <= left {
            left -= text.len();
            hosts_in_text(text, &mut message_hosts);
        }
    }
    Ok(RequestScreen {
        result_texts,
        message_hosts,
        limited,
        unscanned,
    })
}

/// The sink of the screen: borrowed texts, copied only when they fit in the budget.
#[derive(Default)]
struct Collect<'a> {
    results: Vec<(usize, Vec<&'a str>, bool)>,
    messages: Vec<&'a str>,
}

impl<'a> TextSink<'a> for Collect<'a> {
    fn result(&mut self, index: usize, parts: &[&'a str], complete: bool) {
        self.results.push((index, parts.to_vec(), complete));
    }
    fn message(&mut self, text: &'a str) {
        self.messages.push(text);
    }
}

/// Whether the request side screens (§ 6.8.0, D27).
#[must_use]
pub fn screens(snapshot: &PolicySnapshot, detectors_enabled: bool) -> bool {
    if !detectors_enabled {
        return false;
    }
    match &snapshot.set {
        PolicySet::Unavailable => false,
        PolicySet::Available { policies, tenant, .. } => {
            // The tenant part counts: every kind can reach a tenant policy, so screen.
            let tenant_counts = *tenant == TenantPart::Evaluated && matches!(snapshot.mode, TenantMode::LogOnly | TenantMode::Enforce);
            // Else the set is floor-only (P-4): screen when it reads a screen-dependent kind.
            tenant_counts || policies.reads_screen_dependent()
        }
    }
}

/// The seen hosts of a request (D14, P-15): the message hosts, and the hosts of each result text
/// that has no finding of `medium` or higher and that the plan does not replace with
/// `enforced = true`. A result with no entry in `result_findings` does not count.
#[must_use]
pub fn seen_hosts(screen: &RequestScreen, result_findings: &[Vec<Finding>], replaced_enforced: &dyn Fn(usize) -> bool) -> HostSet {
    let mut seen = screen.message_hosts.clone();
    for (index, text) in screen.result_texts.iter().enumerate() {
        let Some(text) = text else { continue };
        let Some(findings) = result_findings.get(index) else { continue };
        if findings.iter().any(|f| f.severity >= Severity::Medium) || replaced_enforced(index) {
            continue;
        }
        hosts_in_text(text, &mut seen);
    }
    seen
}

#[cfg(test)]
mod tests {
    use paigasus_kernel::detect::{DETECTOR_BUILT_IN, INJECTION_MARKERS, UNSEEN_HOST};
    use paigasus_kernel::tool_policy::{Facts, RequestInfo, SessionInfo, ToolPolicies};
    use paigasus_kernel::{Prn, to_cedar_uid};
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::*;
    use crate::policy::{OnUnavailable, SHIPPED_FLOOR, SeenHosts};
    use crate::turn::{ToolTurn, parse_request};

    const MAX: usize = 16 * 1024 * 1024;

    fn turn_of(format: Format, body: &Value) -> TurnFacts {
        match parse_request(format, body.to_string().as_bytes()) {
            ToolTurn::Parsed(f) => f,
            ToolTurn::Unavailable(r) => panic!("unavailable: {r}"),
        }
    }

    fn screen_max(format: Format, body: &Value, max: usize) -> RequestScreen {
        let turn = turn_of(format, body);
        screen_request(format, body.to_string().as_bytes(), &turn, max).expect("screen")
    }

    fn screen(format: Format, body: &Value) -> RequestScreen {
        screen_max(format, body, MAX)
    }

    fn texts(s: &RequestScreen) -> Vec<Option<&str>> {
        s.result_texts.iter().map(|t| t.as_deref()).collect()
    }

    /// The `parse_request` fixtures of `turn.rs`, and each SMA-776 result shape.
    fn fixtures() -> Vec<(Format, Value)> {
        use Format::{AnthropicMessages as A, OpenAiChat as C, OpenAiResponses as R};
        vec![
            (
                C,
                json!({"model":"m","tools":[{"type":"function","function":{"name":"read"}}],"messages":[{"role":"user","content":"hi"}]}),
            ),
            (
                C,
                json!({"model":"m","tools":[{"type":"function","function":{"name":"read","parameters":{"type":"object"}}},{"type":"custom","custom":{"name":"sql"}}],
                   "functions":[{"name":"old","parameters":{}}],
                   "messages":[{"role":"user","content":"hi"},
                               {"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"read","arguments":"{}"}}]},
                               {"role":"tool","tool_call_id":"c1","content":"SECRET RESULT"},
                               {"role":"assistant","function_call":{"name":"old","arguments":"{}"}},
                               {"role":"function","name":"old","content":"SECRET"}]}),
            ),
            (
                C,
                json!({"messages":[{"role":"assistant","tool_calls":[
                {"id":"c1","type":"custom","custom":{"name":"sql","input":"SELECT 1"}},
                {"id":"c2","type":"function","function":{"name":"read","arguments":"{}"}}]}]}),
            ),
            (A, json!({"model":"m","tools":[{"name":"read","input_schema":{}}],"messages":[{"role":"user","content":"hi"}]})),
            (
                A,
                json!({"model":"c","tools":[{"name":"rm","input_schema":{"type":"object"}},{"type":"web_search_20250305","name":"web_search"}],
                   "messages":[{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"rm","input":{"p":"/"}}]},
                               {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"SECRET"}]}]}),
            ),
            (
                R,
                json!({"model":"m",
                "tools":[{"type":"function","name":"read","parameters":{"type":"object"}},{"type":"custom","name":"sql"},{"type":"web_search"}],
                "input":[{"role":"user","content":"hi"},
                         {"type":"function_call","id":"fc_1","call_id":"c1","name":"read","arguments":"{\"p\":1}","status":"completed"},
                         {"type":"function_call_output","call_id":"c1","output":"SECRET"},
                         {"type":"custom_tool_call","id":"ctc_1","call_id":"c2","name":"sql","input":"SELECT 1"},
                         {"type":"custom_tool_call_output","call_id":"c2","output":"SECRET"},
                         {"type":"shell_call","id":"sh_1","call_id":"c3","status":"completed","action":{"commands":["ls"]}},
                         {"type":"shell_call_output","call_id":"c3","output":[]},
                         {"type":"computer_call_output","call_id":"c4","output":{}},
                         {"type":"local_shell_call_output","call_id":"c5","output":"x"},
                         {"type":"apply_patch_call_output","call_id":"c6","status":"completed"},
                         {"type":"mcp_approval_request","id":"mcpr_1","name":"rm","server_label":"srv","arguments":"{}"},
                         {"type":"mcp_approval_response","approval_request_id":"mcpr_1","approve":true},
                         {"type":"reasoning","id":"rs_1","summary":[]}]}),
            ),
            (
                R,
                json!({"input":[
                {"type":"local_shell_call","id":"ls_1","call_id":"c1","action":{"type":"exec","command":["ls"],"env":{}},"status":"completed"},
                {"type":"local_shell_call_output","id":"ls_1","output":"x"},
                {"type":"function_call_output","id":"fco_1","call_id":"c2","output":"x"}]}),
            ),
            (R, json!({"model":"m","input":"hi"})),
            (R, json!({"tools":[{"type":"tool_search"}],"input":"hi"})),
            (
                R,
                json!({"model":"m","stream":true,
                "tools":[{"type":"function","name":"read","parameters":{"type":"object","properties":{"Id":{"type":"integer","maximum":1e300},"constructor":{}}}}],
                "metadata":{"Id":"x"},
                "input":[{"type":"function_call","id":"fc_1","call_id":"c1","name":"read","arguments":"{}","status":"completed"},
                         {"type":"function_call_output","call_id":"c1","output":"SECRET"}]}),
            ),
        ]
    }

    #[test]
    fn result_keys_equal_parse_request_for_every_fixture() {
        for (format, body) in fixtures() {
            let turn = turn_of(format, &body);
            let s = screen_request(format, body.to_string().as_bytes(), &turn, MAX).unwrap_or_else(|e| panic!("{e:?}: {body}"));
            assert_eq!(s.result_texts.len(), turn.tool_results.len(), "{body}");
            assert!(s.limited.is_empty(), "{body}");
        }
    }

    #[test]
    fn result_texts_per_shape() {
        let chat = screen(
            Format::OpenAiChat,
            &json!({"messages":[
                {"role":"tool","tool_call_id":"a","content":"plain"},
                {"role":"tool","tool_call_id":"b","content":[{"type":"text","text":"one"},{"type":"image_url","image_url":{"url":"x"}},{"type":"text","text":"two"}]},
                {"role":"function","name":"old","content":"legacy"}]}),
        );
        assert_eq!(texts(&chat), [Some("plain"), Some("one\ntwo"), Some("legacy")]);

        let anthropic = screen(
            Format::AnthropicMessages,
            &json!({"messages":[{"role":"user","content":[
                {"type":"tool_result","tool_use_id":"a","content":"plain"},
                {"type":"tool_result","tool_use_id":"b","content":[{"type":"text","text":"one"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA"}},{"type":"text","text":"two"}]},
                {"type":"tool_result","tool_use_id":"c","content":[{"type":"image","source":{}}]}]}]}),
        );
        assert_eq!(texts(&anthropic), [Some("plain"), Some("one\ntwo"), None]);

        let responses = screen(
            Format::OpenAiResponses,
            &json!({"input":[
                {"type":"function_call_output","call_id":"a","output":"plain"},
                {"type":"custom_tool_call_output","call_id":"b","output":[{"type":"input_text","text":"one"},{"type":"input_image","image_url":"x"},{"type":"input_text","text":"two"}]},
                {"type":"shell_call_output","call_id":"c","output":[{"stdout":"out1","stderr":"err1","outcome":{"type":"exit","exit_code":0}},{"stdout":"out2","stderr":"","outcome":{"type":"timeout"}}]},
                {"type":"computer_call_output","call_id":"d","output":{"type":"computer_screenshot","image_url":"x"}},
                {"type":"mcp_approval_response","approval_request_id":"e","approve":true},
                {"type":"local_shell_call_output","id":"f","output":"shell"}]}),
        );
        assert_eq!(texts(&responses), [Some("plain"), Some("one\ntwo"), Some("out1\nerr1\nout2\n"), None, None, Some("shell")]);
    }

    fn has(s: &RequestScreen, host: &str) -> bool {
        s.message_hosts.contains(host)
    }

    #[test]
    fn seen_sources_per_format() {
        let chat = screen(
            Format::OpenAiChat,
            &json!({"tools":[{"type":"function","function":{"name":"f","description":"see https://desc.example/"}}],"messages":[
                {"role":"system","content":"https://sys.example/"},
                {"role":"developer","content":[{"type":"text","text":"https://dev.example/"}]},
                {"role":"user","content":"https://user.example/"},
                {"role":"user","content":[{"type":"text","text":"https://parts.example/"},{"type":"image_url","image_url":{"url":"https://img.example/"}}]},
                {"role":"assistant","content":"https://assistant.example/","tool_calls":[{"id":"c","type":"function","function":{"name":"f","arguments":"{\"u\":\"https://args.example/\"}"}}]}]}),
        );
        for h in ["sys.example", "dev.example", "user.example", "parts.example"] {
            assert!(has(&chat, h), "chat {h}");
        }
        for h in ["desc.example", "img.example", "assistant.example", "args.example"] {
            assert!(!has(&chat, h), "chat {h}");
        }

        let anthropic = |system: Value| {
            screen(
                Format::AnthropicMessages,
                &json!({"system": system,"tools":[{"name":"f","description":"https://desc.example/","input_schema":{}}],"messages":[
                    {"role":"user","content":"https://user.example/"},
                    {"role":"user","content":[{"type":"text","text":"https://block.example/"},{"type":"tool_result","tool_use_id":"t","content":"https://result.example/"}]},
                    {"role":"assistant","content":[{"type":"text","text":"https://assistant.example/"},{"type":"tool_use","id":"t","name":"f","input":{"u":"https://args.example/"}}]}]}),
            )
        };
        for s in [anthropic(json!("https://sys.example/")), anthropic(json!([{"type":"text","text":"https://sys.example/"}]))] {
            for h in ["sys.example", "user.example", "block.example"] {
                assert!(has(&s, h), "anthropic {h}");
            }
            for h in ["desc.example", "result.example", "assistant.example", "args.example"] {
                assert!(!has(&s, h), "anthropic {h}");
            }
        }

        let string_input = screen(Format::OpenAiResponses, &json!({"instructions":"https://instr.example/","input":"https://input.example/"}));
        assert!(has(&string_input, "instr.example") && has(&string_input, "input.example"));
        let items = screen(
            Format::OpenAiResponses,
            &json!({"tools":[{"type":"function","name":"f","description":"https://desc.example/","parameters":{}}],"input":[
                {"type":"message","role":"system","content":"https://sys.example/"},
                {"type":"message","role":"developer","content":[{"type":"input_text","text":"https://dev.example/"}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"https://user.example/"},{"type":"input_image","image_url":"https://img.example/"}]},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"https://assistant.example/"}]},
                {"role":"assistant","content":"https://short-assistant.example/"},
                {"type":"function_call","call_id":"c","name":"f","arguments":"{\"u\":\"https://args.example/\"}"},
                {"type":"function_call_output","call_id":"c","output":"https://result.example/"}]}),
        );
        for h in ["sys.example", "dev.example", "user.example"] {
            assert!(has(&items, h), "responses {h}");
        }
        for h in ["desc.example", "img.example", "assistant.example", "short-assistant.example", "args.example", "result.example"] {
            assert!(!has(&items, h), "responses {h}");
        }
    }

    #[test]
    fn a_responses_short_form_user_message_is_a_seen_source() {
        let body = json!({"model":"m","input":[{"role":"user","content":"read https://short.example/"}]});
        assert!(screen(Format::OpenAiResponses, &body).message_hosts.contains("short.example"));
        let parts = json!({"model":"m","input":[{"role":"developer","content":[{"type":"input_text","text":"https://short-parts.example/"}]}]});
        assert!(screen(Format::OpenAiResponses, &parts).message_hosts.contains("short-parts.example"));
    }

    #[test]
    fn a_forced_key_mismatch_is_an_error() {
        let body = json!({"messages":[{"role":"tool","tool_call_id":"a","content":"x"},{"role":"tool","tool_call_id":"b","content":"y"}]});
        let bytes = body.to_string();
        let mut t = turn_of(Format::OpenAiChat, &body);
        t.tool_results.pop();
        assert_eq!(screen_request(Format::OpenAiChat, bytes.as_bytes(), &t, MAX).unwrap_err(), ScreenError::KeyMismatch);
        let mut t = turn_of(Format::OpenAiChat, &body);
        t.tool_results[1].call_id = Some("other".into());
        assert_eq!(screen_request(Format::OpenAiChat, bytes.as_bytes(), &t, MAX).unwrap_err(), ScreenError::KeyMismatch);
    }

    /// S13: a screen failure while `parse_request` gave `Parsed` (here: the facts of one body and
    /// the bytes of another, which no longer parse) is an error, never an empty screen.
    #[test]
    fn a_screen_failure_after_a_parsed_turn_is_an_error() {
        let body = json!({"messages":[{"role":"tool","tool_call_id":"a","content":"x"}]});
        let t = turn_of(Format::OpenAiChat, &body);
        for bad in [&b"{"[..], br#"{"a":1,"a":2}"#, br#"{"messages":"x"}"#] {
            assert_eq!(screen_request(Format::OpenAiChat, bad, &t, MAX).unwrap_err(), ScreenError::Malformed);
        }
    }

    /// RULING F10: 30 KiB results, so two fit in a 65 536-byte budget and the third does not.
    #[test]
    fn the_budget_scans_newest_results_first_then_messages() {
        let big = |c: char| c.to_string().repeat(30 * 1024);
        let message = format!("https://late.example/ {}", "m".repeat(5000));
        let body = json!({"messages":[
            {"role":"tool","tool_call_id":"a","content":big('a')},
            {"role":"tool","tool_call_id":"b","content":big('b')},
            {"role":"tool","tool_call_id":"c","content":big('c')},
            {"role":"user","content":message}]});
        let s = screen_max(Format::OpenAiChat, &body, 65536);
        assert!(s.result_texts[0].is_none());
        assert_eq!(s.result_texts[1].as_deref().map(str::len), Some(30 * 1024));
        assert_eq!(s.result_texts[2].as_deref().map(str::len), Some(30 * 1024));
        assert_eq!(s.limited, [0]);
        assert!(!s.message_hosts.contains("late.example"), "the message does not fit and is not scanned");
        // The control: with room for everything, the message is scanned and nothing is limited.
        let all = screen_max(Format::OpenAiChat, &body, 4 * 30 * 1024 + 6000);
        assert!(all.limited.is_empty());
        assert!(all.message_hosts.contains("late.example"));
    }

    #[test]
    fn a_budget_inside_a_multibyte_char_gives_scan_limit_and_no_panic() {
        // 3 bytes per char: a budget of 65536 falls inside a char of a 66 000-byte text.
        let text = "日本語".repeat(66_000 / 9);
        assert!(text.len() > 65536 && !text.is_char_boundary(65536));
        let body = json!({"messages":[{"role":"tool","tool_call_id":"a","content":text}]});
        let s = screen_max(Format::OpenAiChat, &body, 65536);
        assert_eq!(s.limited, [0]);
        assert!(s.result_texts[0].is_none());
    }

    /// Carried from the Task 14 review: a result shape that the screen cannot read must not pass
    /// unscanned and unreported. The readable text parts of such a result are still scanned.
    #[test]
    fn a_result_shape_that_the_screen_cannot_read_is_unscanned() {
        use Format::{AnthropicMessages as A, OpenAiChat as C, OpenAiResponses as R};
        let anthropic = |content: Value| json!({"model":"m","max_tokens":1,"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":content}]}]});
        let responses = |output: Value| json!({"model":"m","input":[{"type":"function_call_output","call_id":"c1","output":output}]});
        let chat = |content: Value| json!({"messages":[{"role":"tool","tool_call_id":"a","content":content}]});
        let cases: Vec<(Format, Value, bool, Option<&str>)> = vec![
            // An object content: nothing is read.
            (C, chat(json!({"text":"hidden"})), true, None),
            (R, responses(json!({"text":"hidden"})), true, None),
            (A, anthropic(json!({"text":"hidden"})), true, None),
            // An Anthropic document with a base64 (PDF), URL or file source: not read (fail closed).
            (
                A,
                anthropic(json!([{"type":"text","text":"seen"},{"type":"document","source":{"type":"base64","media_type":"application/pdf","data":"aGlkZGVu"}}])),
                true,
                Some("seen"),
            ),
            (A, anthropic(json!([{"type":"document","source":{"type":"url","url":"https://d.example/a.pdf"}}])), true, None),
            (A, anthropic(json!([{"type":"document","source":{"type":"file","file_id":"f1"}}])), true, None),
            // A search_result with a block that is not text in its content.
            (
                A,
                anthropic(json!([{"type":"search_result","source":"s","title":"t","content":[{"type":"document","source":{"type":"file","file_id":"f1"}}]}])),
                true,
                Some("t\ns"),
            ),
            // The same block types are not read in a chat result.
            (C, chat(json!([{"type":"tool_reference","tool_name":"x"}])), true, None),
            // A Responses file part.
            (R, responses(json!([{"type":"input_file","file_data":"hidden"}])), true, None),
            // A text part whose text is not a string.
            (C, chat(json!([{"type":"text","text":{"x":"hidden"}}])), true, None),
            // Controls: a string, the text parts and an image part are read in full.
            // Controller ruling (Task 22 review): the Anthropic RAG blocks are read. A search_result
            // gives its title, its source and its text blocks; a document with a text or content
            // source gives its title, its context and that text; a tool_reference holds no text.
            (
                A,
                anthropic(json!([{"type":"search_result","source":"https://s.example","title":"t","content":[{"type":"text","text":"r1"},{"type":"text","text":"r2"}]}])),
                false,
                Some("t\nhttps://s.example\nr1\nr2"),
            ),
            (
                A,
                anthropic(json!([{"type":"text","text":"seen"},{"type":"document","title":"d","context":"c","source":{"type":"text","media_type":"text/plain","data":"plain"}}])),
                false,
                Some("seen\nd\nc\nplain"),
            ),
            (
                A,
                anthropic(
                    json!([{"type":"document","source":{"type":"content","content":[{"type":"text","text":"n1"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"x"}}]}}]),
                ),
                false,
                Some("n1"),
            ),
            (A, anthropic(json!([{"type":"document","source":{"type":"content","content":"n2"}}])), false, Some("n2")),
            (A, anthropic(json!([{"type":"tool_reference","tool_name":"x"}])), false, None),
            (C, chat(json!("seen")), false, Some("seen")),
            (
                A,
                anthropic(json!([{"type":"text","text":"seen"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"x"}}])),
                false,
                Some("seen"),
            ),
            (
                R,
                responses(json!([{"type":"input_text","text":"seen"},{"type":"input_image","image_url":"data:x"}])),
                false,
                Some("seen"),
            ),
        ];
        for (format, body, unscanned, text) in cases {
            let s = screen(format, &body);
            assert_eq!(s.unscanned == [0], unscanned, "{format:?} {body}");
            assert_eq!(texts(&s), [text], "{format:?} {body}");
        }
    }

    #[test]
    fn a_large_adversarial_request_screens_in_linear_time() {
        let mut messages: Vec<Value> = (0..20_000)
            .map(|i| json!({"role":"tool","tool_call_id":format!("c{i}"),"content":"https://a.example/ ".repeat(20)}))
            .collect();
        messages.push(json!({"role":"user","content":"https://b.example/ ".repeat(200_000)}));
        let body = json!({ "messages": messages });
        let start = std::time::Instant::now();
        let s = screen(Format::OpenAiChat, &body);
        // A very wide margin: this is a guard against a quadratic walk, not a benchmark.
        assert!(start.elapsed() < std::time::Duration::from_secs(60), "{:?}", start.elapsed());
        assert_eq!(s.result_texts.len(), 20_000);
        assert!(s.limited.is_empty());
        assert!(s.message_hosts.contains("b.example"));
    }

    fn facts() -> Facts {
        let prn = Prn::parse("prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004").expect("prn");
        Facts::new(
            to_cedar_uid(&prn),
            Uuid::nil(),
            RequestInfo::new("prn:pgs:gateway:::scope/x", crate::policy::ingress(Format::OpenAiChat), "m"),
            SessionInfo::placeholder(),
        )
    }

    fn snapshot(floor: &str, tenant: TenantPart, mode: TenantMode) -> PolicySnapshot {
        let floor = Arc::new(ToolPolicies::compile_floor(floor).expect("floor"));
        let text = if tenant == TenantPart::Evaluated {
            r#"@id("all") permit(principal, action, resource);"#
        } else {
            ""
        };
        let set = PolicySet::Available {
            policies: Arc::new(ToolPolicies::compile(Arc::clone(&floor), text, &[]).expect("compile")),
            floor_only: Arc::new(ToolPolicies::compile(floor, "", &[]).expect("floor only")),
            tenant,
            facts: facts(),
        };
        PolicySnapshot::new(set, mode, OnUnavailable::Open)
    }

    #[test]
    fn the_screen_gate() {
        let severity_floor = r#"@id("f") forbid(principal, action, resource) when { context.max_severity >= 3 };"#;
        for floor in [SHIPPED_FLOOR, severity_floor] {
            for mode in [TenantMode::LogOnly, TenantMode::Enforce] {
                assert!(screens(&snapshot(floor, TenantPart::Evaluated, mode), true), "{floor} {mode:?}");
                assert!(!screens(&snapshot(floor, TenantPart::Evaluated, mode), false), "enabled = false");
            }
        }
        for (tenant, mode) in [
            (TenantPart::Off, TenantMode::Off),
            (TenantPart::Unavailable, TenantMode::Enforce),
            (TenantPart::Unavailable, TenantMode::LogOnly),
        ] {
            assert!(!screens(&snapshot(SHIPPED_FLOOR, tenant, mode), true), "shipped floor {tenant:?}");
            assert!(screens(&snapshot(severity_floor, tenant, mode), true), "severity floor {tenant:?}");
            assert!(!screens(&snapshot(severity_floor, tenant, mode), false), "enabled = false");
        }
        for mode in [TenantMode::Off, TenantMode::LogOnly, TenantMode::Enforce] {
            assert!(!screens(&PolicySnapshot::new(PolicySet::Unavailable, mode, OnUnavailable::Closed), true));
        }
    }

    #[test]
    fn seen_hosts_follow_d14() {
        let body = json!({"messages":[
            {"role":"tool","tool_call_id":"a","content":"https://r0.example/"},
            {"role":"tool","tool_call_id":"b","content":"https://r1.example/"},
            {"role":"tool","tool_call_id":"c","content":"https://r2.example/"},
            {"role":"tool","tool_call_id":"d","content":"https://r3.example/"},
            {"role":"tool","tool_call_id":"e","content":"https://r4.example/"},
            {"role":"user","content":"https://msg.example/"}]});
        let s = screen(Format::OpenAiChat, &body);
        let findings = vec![
            vec![Finding::new(DETECTOR_BUILT_IN, UNSEEN_HOST, Severity::Medium, "1")],
            vec![
                Finding::new(DETECTOR_BUILT_IN, "other", Severity::Low, "1"),
                Finding::new(DETECTOR_BUILT_IN, "other", Severity::Info, "1"),
            ],
            vec![],
            vec![],
            vec![Finding::new(DETECTOR_BUILT_IN, INJECTION_MARKERS, Severity::High, "1")],
        ];
        // Result 2 is replaced with `enforced = true`. Result 3 is replaced in `log_only`, so the
        // caller does not report it as enforced.
        let seen = seen_hosts(&s, &findings, &|i| i == 2);
        assert!(!seen.contains("r0.example"), "medium finding");
        assert!(seen.contains("r1.example"), "low and info only");
        assert!(!seen.contains("r2.example"), "replaced, enforced");
        assert!(seen.contains("r3.example"), "replaced, not enforced");
        assert!(!seen.contains("r4.example"), "high finding");
        assert!(seen.contains("msg.example"), "message hosts are always seen");
        // A result with no findings entry does not count (fail toward `unseen_host`).
        let short = seen_hosts(&s, &findings[..1], &|_| false);
        assert!(!short.contains("r1.example") && short.contains("msg.example"));
    }

    #[test]
    fn debug_prints_counts_only() {
        let body = json!({"messages":[
            {"role":"tool","tool_call_id":"a","content":"CANARY-TEXT https://canary-host.example/"},
            {"role":"user","content":"https://canary-msg.example/"}]});
        let s = screen(Format::OpenAiChat, &body);
        let debug = format!("{s:?}");
        assert_eq!(debug, "RequestScreen { results: 1, texts: 1, message_hosts: 1, limited: 0, unscanned: 0 }");
        let seen = SeenHosts(Arc::new(seen_hosts(&s, &[vec![]], &|_| false)));
        let debug = format!("{seen:?}");
        assert_eq!(debug, "SeenHosts { len: 2 }");
        for d in [format!("{s:?}"), debug] {
            assert!(!d.contains("CANARY") && !d.contains("canary"), "{d}");
        }
    }
}
