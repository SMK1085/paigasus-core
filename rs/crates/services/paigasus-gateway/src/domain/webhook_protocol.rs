// SPDX-License-Identifier: Apache-2.0
//! The wire protocol version 1 of a webhook detector (SMA-856 § 6.3). Pure: no I/O, no clock, no
//! random numbers. The HTTP adapter calls these functions.

use std::collections::BTreeSet;

use bytes::Bytes;
use hmac::{Hmac, KeyInit, Mac};
use paigasus_kernel::detect::{Finding, Location, Severity};
use paigasus_kernel::tool_policy::{ToolInput, parses_strictly};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::Sha256;

use crate::domain::detectors::{DetectBatch, Subject, WebhookFailure, valid_version};

/// The protocol version in the `Paigasus-Webhook-Version` header and the `version` body field.
pub const PROTOCOL_VERSION: &str = "1";
/// The largest answer body in bytes. A body of exactly this size passes.
pub const MAX_RESPONSE_BYTES: usize = 65_536;
/// The most findings in one answer. An answer with exactly this many passes.
pub const MAX_FINDINGS: usize = 16;
/// The name of the version header (lower case, as `http` wants it).
pub const HEADER_VERSION: &str = "paigasus-webhook-version";
/// The name of the nonce header.
pub const HEADER_NONCE: &str = "paigasus-nonce";
/// The name of the signature header.
pub const HEADER_SIGNATURE: &str = "paigasus-signature";

type HmacSha256 = Hmac<Sha256>;

#[derive(Serialize)]
struct CallBody<'a> {
    version: u32,
    request_id: Option<&'a str>,
    subject: &'a str,
    org: Option<String>,
    session: Option<String>,
    tool: ToolBody<'a>,
    args: Option<Box<RawValue>>,
    args_text: Option<&'a str>,
    findings: Vec<FindingBody<'a>>,
}

#[derive(Serialize)]
struct ToolBody<'a> {
    name: &'a str,
    declared: &'static str,
}

#[derive(Serialize)]
struct FindingBody<'a> {
    detector: &'a str,
    kind: &'a str,
    severity: &'static str,
    version: &'a str,
    evidence: Vec<EvidenceBody<'a>>,
}

#[derive(Serialize)]
struct EvidenceBody<'a> {
    rule: &'static str,
    location: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pointer: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    host: Option<&'a str>,
}

fn finding_body(f: &Finding) -> FindingBody<'_> {
    let evidence = f
        .evidence
        .iter()
        .filter_map(|e| {
            let (location, pointer) = match &e.location {
                Location::Argument { pointer } => ("argument", Some(pointer.as_str())),
                Location::TextInput => ("text_input", None),
                // A result never occurs on a call, and no other location exists today.
                _ => return None,
            };
            Some(EvidenceBody {
                rule: e.rule,
                location,
                pointer,
                host: e.host.as_deref(),
            })
        })
        .collect();
    FindingBody {
        detector: &f.detector,
        kind: &f.kind,
        severity: f.severity.as_str(),
        version: &f.version,
        evidence,
    }
}

/// The request body of one tool call, shared by every provider (§ 6.3). `None` for a result subject.
///
/// `args` holds the argument text when it passes the kernel strict parse. Surrounding whitespace of
/// the text is removed (`RawValue` trims it). The rest of the text is not changed. In every
/// other case `args` is `null` and `args_text` holds the raw text. `built` is the built-in findings
/// of this call.
#[must_use]
pub fn call_body(batch: &DetectBatch, subject: &Subject, built: &[Finding]) -> Option<Bytes> {
    let Subject::Call { invocation, .. } = subject else {
        return None;
    };
    let (args, args_text) = match &invocation.input {
        ToolInput::Json(text) if parses_strictly(&invocation.input) => match RawValue::from_string(text.clone()) {
            Ok(raw) => (Some(raw), None),
            Err(_) => (None, Some(text.as_str())),
        },
        ToolInput::Json(text) | ToolInput::Text(text) => (None, Some(text.as_str())),
        // `ToolInput` is non-exhaustive. A future variant has no known text: send nothing.
        _ => (None, None),
    };
    let body = CallBody {
        version: 1,
        request_id: batch.request_id.as_deref(),
        subject: &batch.caller.principal,
        org: batch.org.map(|o| o.to_string()),
        session: batch.caller.session.map(|s| s.to_string()),
        tool: ToolBody {
            name: &invocation.name,
            declared: invocation.declared.as_str(),
        },
        args,
        args_text,
        findings: built.iter().map(finding_body).collect(),
    };
    serde_json::to_vec(&body).ok().map(Bytes::from)
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Lower-case hex of the bytes.
#[must_use]
pub fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 0x0f)]));
    }
    out
}

/// The bytes of a lower-case hex string. `None` for an upper-case digit, a non-hex character or an
/// odd length.
#[must_use]
pub fn hex_decode_lower(s: &str) -> Option<Vec<u8>> {
    fn nibble(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes.chunks(2).map(|p| Some((nibble(p[0])? << 4) | nibble(p[1])?)).collect()
}

fn mac_for(secret: &[u8], parts: &[&[u8]]) -> HmacSha256 {
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(secret).expect("HMAC accepts a key of any length");
    for part in parts {
        mac.update(part);
    }
    mac
}

fn response_parts<'a>(name: &'a str, nonce: &'a str, body: &'a [u8]) -> [&'a [u8]; 6] {
    [b"paigasus.response.", name.as_bytes(), b".", nonce.as_bytes(), b".", body]
}

/// The request MAC as 64 lower-case hex characters.
#[must_use]
pub fn request_mac(secret: &[u8], name: &str, t: u64, nonce: &str, body: &[u8]) -> String {
    let t = t.to_string();
    let parts: [&[u8]; 8] = [b"paigasus.request.", name.as_bytes(), b".", t.as_bytes(), b".", nonce.as_bytes(), b".", body];
    hex_lower(&mac_for(secret, &parts).finalize().into_bytes())
}

/// The value of the `Paigasus-Signature` request header.
#[must_use]
pub fn request_signature_header(t: u64, mac_hex: &str) -> String {
    format!("t={t},v1={mac_hex}")
}

/// The response MAC as 64 lower-case hex characters. The gateway only verifies it. The test mock
/// and the protocol doc make one with it.
#[must_use]
pub fn response_mac(secret: &[u8], name: &str, nonce: &str, body: &[u8]) -> String {
    hex_lower(&mac_for(secret, &response_parts(name, nonce, body)).finalize().into_bytes())
}

/// The MAC values of a response signature header: `v1=<64 hex>[,v1=<64 hex>]`, strict.
fn parse_signature_header(header: &str) -> Option<Vec<Vec<u8>>> {
    let values: Vec<&str> = header.split(',').collect();
    if values.len() > 2 {
        return None;
    }
    values
        .into_iter()
        .map(|v| {
            let hex = v.strip_prefix("v1=")?;
            if hex.len() != 64 {
                return None;
            }
            hex_decode_lower(hex)
        })
        .collect()
}

/// Check the response signature. It accepts a value that verifies with any of the secrets. Every
/// failure is [`WebhookFailure::Signature`]. The comparison is constant time (`verify_slice`).
///
/// # Errors
/// [`WebhookFailure::Signature`] when the header is not well-formed or no value verifies.
pub fn verify_response(secrets: &[&[u8]], name: &str, nonce: &str, body: &[u8], header: &str) -> Result<(), WebhookFailure> {
    let values = parse_signature_header(header).ok_or(WebhookFailure::Signature)?;
    let parts = response_parts(name, nonce, body);
    let mut ok = false;
    for secret in secrets {
        for value in &values {
            ok |= mac_for(secret, &parts).verify_slice(value).is_ok();
        }
    }
    if ok { Ok(()) } else { Err(WebhookFailure::Signature) }
}

/// Whether a `Content-Type` value is `application/json`: the media type before `;`, trimmed,
/// compared with no regard to ASCII case. Parameters are allowed.
#[must_use]
pub fn content_type_ok(value: &str) -> bool {
    value.split(';').next().is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    findings: Vec<AnswerFinding>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerFinding {
    kind: String,
    severity: String,
    #[serde(default)]
    version: String,
}

/// Parse and check the answer of a provider. A duplicate key, an unknown or a missing field, a
/// bad severity, a kind outside `kinds`, a bad version or more than [`MAX_FINDINGS`] findings is
/// [`WebhookFailure::Malformed`]. The findings have no evidence.
///
/// # Errors
/// [`WebhookFailure::Malformed`] for each of the cases above.
pub fn parse_answer(name: &str, body: &[u8], kinds: &BTreeSet<String>) -> Result<Vec<Finding>, WebhookFailure> {
    let answer: Answer = serde_json::from_slice(body).map_err(|_| WebhookFailure::Malformed)?;
    if answer.findings.len() > MAX_FINDINGS {
        return Err(WebhookFailure::Malformed);
    }
    answer
        .findings
        .into_iter()
        .map(|f| {
            let severity = match f.severity.as_str() {
                "info" => Severity::Info,
                "low" => Severity::Low,
                "medium" => Severity::Medium,
                "high" => Severity::High,
                _ => return Err(WebhookFailure::Malformed),
            };
            if !kinds.contains(&f.kind) || !valid_version(&f.version) {
                return Err(WebhookFailure::Malformed);
            }
            Ok(Finding::new(name, f.kind, severity, f.version))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use paigasus_kernel::detect::{Evidence, Finding, Severity};
    use paigasus_kernel::tool_policy::{Declared, ToolInput, ToolInvocation};
    use serde_json::Value;
    use uuid::Uuid;

    use super::*;
    use crate::domain::detectors::{Caller, DetectBatch, Subject, WebhookFailure};

    const SECRET: &[u8] = b"paigasus-webhook-example-secret-0001";
    const NAME: &str = "acme_dlp";
    const T: u64 = 1_764_300_000;
    const NONCE: &str = "00112233445566778899aabbccddeeff";
    const BODY: &str = r#"{"version":1,"request_id":"req_example","subject":"prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004","org":"6f1c0000-0000-4000-8000-000000000001","session":"9a2e0000-0000-4000-8000-000000000002","tool":{"name":"send_email","declared":"yes"},"args":{"to":"x@y.example","body":"hi"},"args_text":null,"findings":[]}"#;
    const RESPONSE: &str = r#"{"findings":[{"kind":"pii_email","severity":"medium","version":"2026.11"}]}"#;
    const REQUEST_MAC: &str = "4929f346b96d703a5ab953c6e8981444573fa64e66795b8a5d61ba680c5ba878";
    const RESPONSE_MAC: &str = "cf6147bdf8be4b7c7b4619accf9b1f592a14d6d9209e112a1a86c7e00d6f5638";

    fn batch_with(input: ToolInput) -> DetectBatch {
        let org = Uuid::parse_str("6f1c0000-0000-4000-8000-000000000001").unwrap();
        let session = Uuid::parse_str("9a2e0000-0000-4000-8000-000000000002").unwrap();
        let caller = Caller {
            principal: Arc::from("prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004"),
            session: Some(session),
            opt_in: Arc::from(Vec::<String>::new()),
        };
        let subject = Subject::Call {
            index: 0,
            invocation: ToolInvocation::new("send_email", input, Declared::Yes),
            schema: None,
        };
        DetectBatch::for_calls(Some(org), Some("req_example".to_owned()), vec![subject], None, caller)
    }

    fn example_batch() -> DetectBatch {
        batch_with(ToolInput::Json(r#"{"to":"x@y.example","body":"hi"}"#.to_owned()))
    }

    #[test]
    fn the_worked_example_matches_byte_for_byte() {
        let batch = example_batch();
        assert_eq!(call_body(&batch, &batch.subjects[0], &[]).unwrap(), BODY.as_bytes());
        assert_eq!(request_mac(SECRET, NAME, T, NONCE, BODY.as_bytes()), REQUEST_MAC);
        assert_eq!(request_signature_header(T, REQUEST_MAC), format!("t={T},v1={REQUEST_MAC}"));
        assert_eq!(response_mac(SECRET, NAME, NONCE, RESPONSE.as_bytes()), RESPONSE_MAC);
        assert_eq!(verify_response(&[SECRET], NAME, NONCE, RESPONSE.as_bytes(), &format!("v1={RESPONSE_MAC}")), Ok(()));
        // The protocol doc holds the same values. A change of one of them reds this test.
        let doc = include_str!("../../../../../../docs/tool-policies/webhook-protocol.md");
        for value in [REQUEST_MAC, RESPONSE_MAC, NONCE, BODY, RESPONSE] {
            assert!(doc.contains(value), "the protocol doc lacks: {value}");
        }
    }

    #[test]
    fn each_bad_response_signature_is_signature() {
        let good = format!("v1={RESPONSE_MAC}");
        let other_nonce = response_mac(SECRET, NAME, "ffeeddccbbaa99887766554433221100", RESPONSE.as_bytes());
        let other_name = response_mac(SECRET, "other_dlp", NONCE, RESPONSE.as_bytes());
        let mut changed = RESPONSE.as_bytes().to_vec();
        changed[3] ^= 1;
        assert_eq!(verify_response(&[SECRET], NAME, NONCE, &changed, &good), Err(WebhookFailure::Signature));
        for h in [
            format!("v1={other_nonce}"),
            format!("v1={other_name}"),
            good.to_uppercase(),
            format!("v1={}", &RESPONSE_MAC[..63]),
            format!("v1={RESPONSE_MAC}, v1={RESPONSE_MAC}"),
            format!("v1={RESPONSE_MAC},v1={RESPONSE_MAC},v1={RESPONSE_MAC}"),
            format!("V1={RESPONSE_MAC}"),
            format!("t=1,v1={RESPONSE_MAC}"),
            format!("v1={RESPONSE_MAC},"),
            format!(" v1={RESPONSE_MAC}"),
            format!("v1={}", RESPONSE_MAC.to_uppercase()),
            String::new(),
        ] {
            assert_eq!(verify_response(&[SECRET], NAME, NONCE, RESPONSE.as_bytes(), &h), Err(WebhookFailure::Signature), "{h}");
        }
        assert_eq!(verify_response(&[], NAME, NONCE, RESPONSE.as_bytes(), &good), Err(WebhookFailure::Signature));
    }

    #[test]
    fn the_previous_secret_and_a_second_value_are_accepted() {
        let old = b"paigasus-webhook-example-secret-OLD1";
        let by_old = response_mac(old, NAME, NONCE, RESPONSE.as_bytes());
        assert_eq!(verify_response(&[SECRET, old], NAME, NONCE, RESPONSE.as_bytes(), &format!("v1={by_old}")), Ok(()));
        assert_eq!(verify_response(&[SECRET], NAME, NONCE, RESPONSE.as_bytes(), &format!("v1={by_old},v1={RESPONSE_MAC}")), Ok(()));
        // The old secret is not in the list: the old value alone fails.
        assert_eq!(verify_response(&[SECRET], NAME, NONCE, RESPONSE.as_bytes(), &format!("v1={by_old}")), Err(WebhookFailure::Signature));
    }

    #[test]
    fn the_content_type_check_reads_the_media_type_only() {
        for ok in [
            "application/json",
            "application/json; charset=utf-8",
            "Application/JSON;charset=UTF-8",
            "  APPLICATION/Json  ;  charset=utf-8",
            "application/json ;x=y",
        ] {
            assert!(content_type_ok(ok), "{ok}");
        }
        for bad in [
            "application/jsonp",
            "application/jsonx",
            "application/json-seq",
            "application/json5",
            "text/json",
            "application/problem+json",
            "xapplication/json",
            "application/json/x",
            "application/ json",
            "",
        ] {
            assert!(!content_type_ok(bad), "{bad}");
        }
    }

    #[test]
    fn each_bad_answer_is_malformed() {
        let kinds: BTreeSet<String> = ["pii_email".to_owned()].into();
        let seventeen = format!(r#"{{"findings":[{}]}}"#, vec![r#"{"kind":"pii_email","severity":"low"}"#; 17].join(","));
        for bad in [
            r#"{"findings":[],"extra":1}"#,
            r#"{}"#,
            r#"{"findings":[{"kind":"pii_email","severity":"urgent"}]}"#,
            r#"{"findings":[{"kind":"pii_iban","severity":"low"}]}"#,
            r#"{"findings":[{"kind":"pii_email","severity":"low","version":"Bad!"}]}"#,
            r#"{"findings":[{"kind":"pii_email","severity":"low","detector":"kernel"}]}"#,
            r#"{"findings":[{"kind":"pii_email"}]}"#,
            "not json",
            seventeen.as_str(),
        ] {
            assert_eq!(parse_answer(NAME, bad.as_bytes(), &kinds), Err(WebhookFailure::Malformed), "{bad}");
        }
        assert_eq!(parse_answer(NAME, br#"{"findings":[]}"#, &kinds), Ok(vec![]));
        let one = parse_answer(NAME, br#"{"findings":[{"kind":"pii_email","severity":"medium"}]}"#, &kinds).unwrap();
        assert_eq!((one[0].detector.as_str(), one[0].version.as_str(), one[0].severity), (NAME, "", Severity::Medium));
        assert!(one[0].evidence.is_empty());
        // Exactly 16 findings pass (the limit is inclusive).
        let sixteen = format!(r#"{{"findings":[{}]}}"#, vec![r#"{"kind":"pii_email","severity":"low"}"#; 16].join(","));
        assert_eq!(parse_answer(NAME, sixteen.as_bytes(), &kinds).unwrap().len(), 16);
    }

    #[test]
    fn a_duplicate_key_in_the_answer_is_malformed() {
        let kinds: BTreeSet<String> = ["pii_email".to_owned()].into();
        for bad in [
            r#"{"findings":[],"findings":[{"kind":"pii_email","severity":"low"}]}"#,
            r#"{"findings":[{"kind":"pii_email","kind":"pii_email","severity":"low"}]}"#,
        ] {
            assert_eq!(parse_answer(NAME, bad.as_bytes(), &kinds), Err(WebhookFailure::Malformed), "{bad}");
        }
    }

    #[test]
    fn the_arguments_go_verbatim_or_as_text() {
        let body = |input: ToolInput| -> (Vec<u8>, Value) {
            let b = batch_with(input);
            let raw = call_body(&b, &b.subjects[0], &[]).unwrap().to_vec();
            let v = serde_json::from_slice(&raw).unwrap();
            (raw, v)
        };
        let exactly_one = |v: &Value| assert_eq!(u8::from(v["args"].is_null()) + u8::from(v["args_text"].is_null()), 1, "{v}");
        // A duplicate key, a non-object and invalid JSON: args null, args_text the raw text.
        for text in [r#"{"a":1,"a":2}"#, "not json", "[1]"] {
            let (_, v) = body(ToolInput::Json(text.to_owned()));
            assert!(v["args"].is_null(), "{text}");
            assert_eq!(v["args_text"], text);
            exactly_one(&v);
        }
        let (_, v) = body(ToolInput::Text("free text".to_owned()));
        assert!(v["args"].is_null());
        assert_eq!(v["args_text"], "free text");
        exactly_one(&v);
        // A 25-digit and a 19-digit number pass the strict parse: the text goes byte for byte.
        for text in [r#"{"n":1234567890123456789012345}"#, r#"{"n":1234567890123456789}"#] {
            let (raw, v) = body(ToolInput::Json(text.to_owned()));
            assert!(v["args_text"].is_null(), "{text}");
            exactly_one(&v);
            let raw = String::from_utf8(raw).unwrap();
            assert!(raw.contains(&format!(r#""args":{text},"#)), "{raw}");
        }
    }

    #[test]
    fn the_built_in_findings_carry_their_evidence() {
        let built = vec![
            Finding::new("secret_in_args", "secret_in_args", Severity::High, "v1").with_evidence(Evidence::argument("github_pat", "/body")),
            Finding::new("unseen_host", "unseen_host", Severity::Medium, "v1").with_evidence(Evidence::host("url", Some("/u"), "a.example".into())),
            Finding::new("kernel", "unparsed_arguments", Severity::Info, "").with_evidence(Evidence::text_input("free_text")),
        ];
        let b = example_batch();
        let v: Value = serde_json::from_slice(&call_body(&b, &b.subjects[0], &built).unwrap()).unwrap();
        let f = &v["findings"];
        assert_eq!(f[0]["evidence"], serde_json::json!([{"rule":"github_pat","location":"argument","pointer":"/body"}]));
        assert_eq!(f[0]["detector"], "secret_in_args");
        assert_eq!(f[0]["severity"], "high");
        assert_eq!(f[0]["version"], "v1");
        assert_eq!(f[1]["evidence"][0]["host"], "a.example");
        assert_eq!(f[1]["evidence"][0]["pointer"], "/u");
        assert_eq!(f[2]["evidence"], serde_json::json!([{"rule":"free_text","location":"text_input"}]));
        // The request id, the session and the org are null when absent.
        let mut none = batch_with(ToolInput::Json("{}".to_owned()));
        none.request_id = None;
        none.caller.session = None;
        none.org = None;
        let v: Value = serde_json::from_slice(&call_body(&none, &none.subjects[0], &[]).unwrap()).unwrap();
        assert!(v["request_id"].is_null() && v["session"].is_null() && v["org"].is_null());
        for (d, want) in [(Declared::Yes, "yes"), (Declared::No, "no"), (Declared::Unknown, "unknown")] {
            let mut b = example_batch();
            b.subjects = vec![Subject::Call {
                index: 0,
                invocation: ToolInvocation::new("t", ToolInput::Json("{}".to_owned()), d),
                schema: None,
            }];
            let v: Value = serde_json::from_slice(&call_body(&b, &b.subjects[0], &[]).unwrap()).unwrap();
            assert_eq!(v["tool"]["declared"], want);
        }
    }

    #[test]
    fn a_result_subject_has_no_body() {
        let b = example_batch();
        let result = Subject::Result {
            index: 0,
            tool: None,
            text: Arc::from("x"),
        };
        assert!(call_body(&b, &result, &[]).is_none());
    }

    #[test]
    fn hex_round_trips_and_rejects_upper_case() {
        assert_eq!(hex_lower(&[0, 255, 16]), "00ff10");
        assert_eq!(hex_decode_lower("00ff10"), Some(vec![0, 255, 16]));
        assert_eq!(hex_decode_lower("00FF10"), None);
        assert_eq!(hex_decode_lower("abc"), None);
        assert_eq!(hex_decode_lower("zz"), None);
        assert_eq!(hex_decode_lower(""), Some(vec![]));
    }

    #[test]
    fn the_constants_have_the_protocol_values() {
        assert_eq!((PROTOCOL_VERSION, MAX_RESPONSE_BYTES, MAX_FINDINGS), ("1", 65_536, 16));
        assert_eq!((HEADER_VERSION, HEADER_NONCE, HEADER_SIGNATURE), ("paigasus-webhook-version", "paigasus-nonce", "paigasus-signature"));
    }
}
