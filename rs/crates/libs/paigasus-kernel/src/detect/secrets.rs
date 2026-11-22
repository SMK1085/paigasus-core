// SPDX-License-Identifier: Apache-2.0
//! `secret_in_args` (§ 6.3.4): known token forms in the string values and keys of the arguments.
//! There is no entropy heuristic. The evidence holds the rule id and the location, never a part
//! of the secret.

use std::sync::LazyLock;

use regex::RegexSet;

use super::walk::walk_strings;
use super::{CallSubject, Evidence, Finding, SECRET_IN_ARGS, Severity, ToolInput};

/// The version of the token forms.
pub const SECRET_PATTERNS_VERSION: &str = "v1";

/// The most evidence entries that this detector collects.
const MAX_EVIDENCE: usize = 16;

/// One token form per entry, as (rule id, pattern). `(?-u:\b)` is the ASCII word boundary. Every
/// token form is ASCII. A rule can have more than one form.
const PATTERNS: [(&str, &str); 12] = [
    // https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_identifiers.html#identifiers-unique-ids
    ("aws_access_key_id", r"(?-u:\b)(AKIA|ASIA|ABIA|ACCA)[0-9A-Z]{16}(?-u:\b)"),
    // https://github.blog/changelog/2021-04-05-behind-githubs-new-authentication-token-formats/
    ("github_token", r"(?-u:\b)gh[pousr]_[A-Za-z0-9]{36,255}(?-u:\b)"),
    // https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/about-authentication-to-github
    ("github_fine_grained_pat", r"(?-u:\b)github_pat_[A-Za-z0-9_]{22,255}(?-u:\b)"),
    // https://api.slack.com/authentication/token-types
    ("slack_token", r"(?-u:\b)xox[abposr]-[A-Za-z0-9-]{10,}"),
    // https://api.slack.com/messaging/webhooks
    ("slack_webhook", r"https://hooks\.slack\.com/services/T[A-Z0-9]+/B[A-Z0-9]+/[A-Za-z0-9]+"),
    // https://docs.anthropic.com/en/api/getting-started
    ("anthropic_api_key", r"(?-u:\b)sk-ant-(api|admin)[0-9]{2}-[A-Za-z0-9_-]{80,}"),
    // https://platform.openai.com/docs/api-reference/authentication
    ("openai_api_key", r"(?-u:\b)sk-(proj|svcacct|admin)-[A-Za-z0-9_-]{20,}"),
    // The legacy OpenAI form: 20 characters, the marker `T3BlbkFJ`, 20 characters.
    ("openai_api_key", r"(?-u:\b)sk-[A-Za-z0-9]{20}T3BlbkFJ[A-Za-z0-9]{20}(?-u:\b)"),
    // https://cloud.google.com/docs/authentication/api-keys
    // The key can end in `-`, so a word boundary cannot close it: the end class does.
    ("google_api_key", r"(?-u:\b)AIza[0-9A-Za-z_-]{35}(?:[^0-9A-Za-z_-]|$)"),
    // https://docs.stripe.com/keys
    ("stripe_secret_key", r"(?-u:\b)(sk|rk)_live_[0-9A-Za-z]{24,}(?-u:\b)"),
    // https://www.rfc-editor.org/rfc/rfc7468#section-11 (the label of a private key)
    ("pem_private_key", r"-----BEGIN ((RSA|EC|DSA|OPENSSH|ENCRYPTED|PGP) )?PRIVATE KEY( BLOCK)?-----"),
    // https://www.rfc-editor.org/rfc/rfc7519 (a JWS compact form: header.payload.signature)
    ("jwt", r"(?-u:\b)eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"),
];

static SET: LazyLock<Option<RegexSet>> = LazyLock::new(|| RegexSet::new(PATTERNS.iter().map(|(_, p)| *p)).ok());

/// The rule id of each pattern, in order.
#[cfg(test)]
fn rule_ids() -> Vec<&'static str> {
    PATTERNS.iter().map(|(rule, _)| *rule).collect()
}

/// Whether `token` (an unescaped object key) is a known token form.
pub(super) fn is_secret_token(token: &str) -> bool {
    SET.as_ref().is_some_and(|set| set.is_match(token))
}

/// The pointer cut before the first reference token that is itself a secret (RFC 6901: `~1` is
/// decoded before `~0`). A key that is a secret would else appear in the pointer of the key and
/// of every value below it. The cut pointer is the pointer of the object that holds that key.
/// `Evidence` calls it for every argument pointer it stores, at most 16 per finding, so the cost
/// stays linear.
pub(super) fn safe_pointer(pointer: &str) -> &str {
    let Some(set) = SET.as_ref() else { return pointer };
    let mut start = 0;
    while start < pointer.len() {
        let rest = &pointer[start + 1..];
        let end = rest.find('/').map_or(pointer.len(), |i| start + 1 + i);
        let token = pointer[start + 1..end].replace("~1", "/").replace("~0", "~");
        if set.is_match(&token) {
            return &pointer[..start];
        }
        start = end;
    }
    pointer
}

/// `secret_in_args`: a known token form in a string value or an object key of the arguments, or in
/// the raw text when the arguments are not valid JSON. One finding per call, with one evidence
/// entry per (rule, string), at most 16.
///
/// The evidence is the rule id and the location only. The location of a key is the JSON pointer
/// of the parent object, not of the key: the pointer of a key holds the key, and so the secret.
#[must_use]
pub fn secret_in_args(subject: &CallSubject<'_>) -> Option<Finding> {
    let set = SET.as_ref()?;
    let is_text = matches!(subject.input, ToolInput::Text(_));
    let mut evidence: Vec<Evidence> = Vec::new();
    walk_strings(subject.input, &mut |leaf| {
        if evidence.len() >= MAX_EVIDENCE {
            return;
        }
        let matched = set.matches(leaf.text);
        if !matched.matched_any() {
            return;
        }
        let mut last = None;
        for index in matched.iter() {
            let rule = PATTERNS[index].0;
            if last == Some(rule) || evidence.len() >= MAX_EVIDENCE {
                continue;
            }
            last = Some(rule);
            evidence.push(if is_text { Evidence::text_input(rule) } else { Evidence::argument(rule, leaf.pointer) });
        }
    });
    if evidence.is_empty() {
        return None;
    }
    let mut finding = Finding::new(SECRET_IN_ARGS, SECRET_IN_ARGS, Severity::High, SECRET_PATTERNS_VERSION);
    for e in evidence {
        finding = finding.with_evidence(e);
    }
    Some(finding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{Declared, Location, Severity, ToolInput};

    const RULES: [&str; 11] = [
        "aws_access_key_id",
        "github_token",
        "github_fine_grained_pat",
        "slack_token",
        "slack_webhook",
        "anthropic_api_key",
        "openai_api_key",
        "google_api_key",
        "stripe_secret_key",
        "pem_private_key",
        "jwt",
    ];

    fn n(c: char, count: usize) -> String {
        c.to_string().repeat(count)
    }

    /// (rule, positive, near miss). Each token is built from parts, so no scanner flags this file.
    fn cases() -> Vec<(&'static str, String, String)> {
        vec![
            ("aws_access_key_id", aws_token(), format!("AK{}{}", "IA", n('7', 15))),
            ("github_token", format!("gh{}_{}", "p", n('a', 36)), format!("gh{}_{}", "p", n('a', 35))),
            ("github_fine_grained_pat", format!("github{}pat_{}", "_", n('b', 22)), format!("github{}pat_{}", "_", n('b', 21))),
            ("slack_token", format!("xo{}b-{}", "x", n('1', 10)), format!("xo{}b-{}", "x", n('1', 9))),
            (
                "slack_webhook",
                format!("https://hooks.{}.com/services/T0A/B0B/{}", "slack", n('c', 8)),
                format!("https://hooks.{}.com/services/t0A/B0B/{}", "slack", n('c', 8)),
            ),
            ("anthropic_api_key", format!("sk-{}-api03-{}", "ant", n('d', 80)), format!("sk-{}-api03-{}", "ant", n('d', 79))),
            ("openai_api_key", format!("sk-{}-{}", "proj", n('e', 20)), format!("sk-{}-{}", "proj", n('e', 19))),
            ("google_api_key", format!("AI{}{}", "za", n('f', 35)), format!("AI{}{}", "za", n('f', 34))),
            ("stripe_secret_key", format!("sk_{}_{}", "live", n('g', 24)), format!("sk_{}_{}", "live", n('g', 23))),
            ("pem_private_key", format!("-----BEGIN RSA {}-----", "PRIVATE KEY"), format!("-----BEGIN RSA {}-----", "PRIVATE KE")),
            (
                "jwt",
                format!("ey{}{}.eyJ{}.{}", "J", n('h', 10), n('i', 10), n('j', 10)),
                format!("eyJ{}.eyJ{}.{}", n('h', 10), n('i', 9), n('j', 10)),
            ),
        ]
    }

    fn json_with(token: &str) -> ToolInput {
        ToolInput::Json(format!(r#"{{"note":"see {token} ok"}}"#))
    }

    fn run(input: &ToolInput) -> Option<Finding> {
        secret_in_args(&CallSubject::new("t", input, Declared::Yes))
    }

    fn aws_token() -> String {
        format!("AK{}{}", "IA", n('7', 16))
    }

    fn escaped_aws() -> String {
        format!("\\u0041K{}{}", "IA", n('7', 16))
    }

    #[test]
    fn the_rule_table_matches_the_rule_list() {
        let mut ids = rule_ids();
        ids.dedup();
        assert_eq!(ids, RULES);
    }

    #[test]
    fn one_positive_and_one_near_miss_per_rule() {
        for (rule, pos, miss) in cases() {
            let f = run(&json_with(&pos)).unwrap_or_else(|| panic!("{rule}"));
            assert_eq!(f.evidence[0].rule, rule);
            assert_eq!((f.kind.as_str(), f.severity, f.version.as_str()), ("secret_in_args", Severity::High, "v1"));
            assert_eq!(f.detector, "secret_in_args");
            assert!(run(&json_with(&miss)).is_none(), "{rule} near miss");
            assert!(run(&ToolInput::Text(pos)).is_some(), "{rule} text");
        }
    }

    #[test]
    fn a_google_key_that_ends_in_a_dash_is_found() {
        let key = format!("AI{}{}-", "za", n('f', 34));
        for input in [
            json_with(&key),
            ToolInput::Json(format!(r#"["{key}"]"#)),
            ToolInput::Text(key.clone()),
            ToolInput::Text(format!("'{key}'")),
        ] {
            assert_eq!(run(&input).expect("finding").evidence[0].rule, "google_api_key");
        }
        let too_long = format!("{key}x");
        assert!(run(&json_with(&too_long)).is_none());
    }

    #[test]
    fn a_secret_ancestor_key_never_reaches_a_pointer() {
        let aws = aws_token();
        let ghp = format!("gh{}_{}", "p", n('a', 36));
        let inputs = [
            format!(r#"{{"{aws}":"{aws}"}}"#),
            format!(r#"{{"{aws}":{{"note":"{ghp}"}}}}"#),
            format!(r#"{{"{aws}":{{"{ghp}":1}}}}"#),
            format!(r#"{{"{aws}":[{{"k":"{aws}"}}]}}"#),
            format!(r#"{{"ok":{{"{aws}":{{"x":"{ghp}"}}}}}}"#),
        ];
        for text in inputs {
            let f = run(&ToolInput::Json(text.clone())).expect("finding");
            for e in &f.evidence {
                let Location::Argument { pointer } = &e.location else { panic!("not an argument") };
                assert!(!pointer.contains(&aws[4..]) && !pointer.contains(&ghp[4..]), "{text}: {pointer}");
            }
        }
        let f = run(&ToolInput::Json(format!(r#"{{"ok":{{"{aws}":{{"x":"{ghp}"}}}}}}"#))).expect("finding");
        assert!(f.evidence.iter().any(|e| e.location == Location::Argument { pointer: "/ok".into() }));
    }

    #[test]
    fn a_prefix_inside_a_word_is_a_near_miss() {
        assert!(run(&json_with(&format!("x{}", aws_token()))).is_none());
        assert!(run(&json_with(&format!("{}A", aws_token()))).is_none());
    }

    #[test]
    fn the_openai_legacy_form_is_found() {
        let t = format!("sk-{}T3Blbk{}{}", n('k', 20), "FJ", n('l', 20));
        assert_eq!(run(&json_with(&t)).expect("finding").evidence[0].rule, "openai_api_key");
    }

    #[test]
    fn a_secret_behind_a_u_escape_in_a_key_and_deep() {
        let value = ToolInput::Json(format!(r#"{{"a":{{"b":["x",{{"c":"{}"}}]}}}}"#, escaped_aws()));
        let f = run(&value).expect("value");
        assert_eq!(f.evidence[0].location, Location::Argument { pointer: "/a/b/1/c".into() });
        let key = ToolInput::Json(format!(r#"{{"a":{{"{}":1}}}}"#, escaped_aws()));
        let f = run(&key).expect("key");
        assert_eq!(f.evidence[0].location, Location::Argument { pointer: "/a".into() });
        let deep = format!("{}\"{}\"{}", r#"{"k":"#.repeat(20), escaped_aws(), "}".repeat(20));
        assert!(run(&ToolInput::Json(deep)).is_some());
        let in_array = ToolInput::Json(format!(r#"["{}"]"#, aws_token()));
        let f = run(&in_array).expect("array");
        assert_eq!(f.evidence[0].location, Location::Argument { pointer: "/0".into() });
    }

    #[test]
    fn a_secret_under_a_duplicate_key_behind_an_escape() {
        let t = ToolInput::Json(format!(r#"{{"a":1,"a":"{}"}}"#, escaped_aws()));
        assert!(run(&t).is_some());
        let t = ToolInput::Json(format!(r#"{{"k":"x","k":"{}"}}"#, escaped_aws()));
        assert!(run(&t).is_some());
    }

    #[test]
    fn a_secret_in_double_encoded_arguments_is_found() {
        let inner = format!(r#"{{"k":"{}"}}"#, aws_token()).replace('"', "\\\"");
        let t = ToolInput::Json(format!(r#"{{"arguments":"{inner}"}}"#));
        assert!(run(&t).is_some());
    }

    #[test]
    fn invalid_json_is_scanned_raw_with_escapes_decoded() {
        let t = ToolInput::Json(format!(r#"{{"a":"{}"#, escaped_aws()));
        let f = run(&t).expect("finding");
        assert_eq!(f.evidence[0].location, Location::Argument { pointer: String::new() });
        let text = ToolInput::Text(aws_token());
        assert_eq!(run(&text).expect("finding").evidence[0].location, Location::TextInput);
    }

    #[test]
    fn one_finding_and_one_evidence_per_rule_and_leaf_up_to_16() {
        let tok = aws_token();
        let many: Vec<String> = (0..20).map(|i| format!(r#""k{i}":"{tok} {tok}""#)).collect();
        let f = run(&ToolInput::Json(format!("{{{}}}", many.join(",")))).expect("finding");
        assert_eq!(f.evidence.len(), 16);
        let two = ToolInput::Text(format!("{tok} gh{}_{}", "p", n('a', 36)));
        assert_eq!(run(&two).expect("finding").evidence.len(), 2);
    }

    #[test]
    fn evidence_holds_no_secret_byte() {
        let token = aws_token();
        let f = run(&json_with(&token)).expect("finding");
        let dump = format!("{f:?} {:?}", f.evidence.iter().map(|e| (e.rule, &e.location, &e.host)).collect::<Vec<_>>());
        assert!(!dump.contains(&token[4..]));
        assert!(f.evidence.iter().all(|e| match &e.location {
            Location::Argument { pointer } => !pointer.contains(&token[4..]),
            _ => true,
        }));
        let key = ToolInput::Json(format!(r#"{{"{token}":1}}"#));
        let f = run(&key).expect("finding");
        assert!(matches!(&f.evidence[0].location, Location::Argument { pointer } if pointer.is_empty()));
    }

    #[test]
    fn a_large_adversarial_input_is_linear() {
        let unit = ["AKIA", "ghp_", "sk-ant-api03-", "eyJaaaaaaaaaa.eyJ", "github_pat_", "-----BEGIN RSA "].concat();
        let big = unit.repeat(4 * 1024 * 1024 / unit.len());
        let start = std::time::Instant::now();
        assert!(run(&ToolInput::Text(big.clone())).is_none());
        assert!(run(&ToolInput::Json(format!(r#"{{"a":"{big}"}}"#))).is_none());
        assert!(start.elapsed().as_secs() < 30, "{:?}", start.elapsed());
    }
}
