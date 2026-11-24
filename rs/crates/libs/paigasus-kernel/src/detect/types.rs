// SPDX-License-Identifier: Apache-2.0
//! The finding types of the tool call detectors (SMA-855). Every public type is
//! `#[non_exhaustive]`, so a caller makes a value with its constructor.
//!
//! Evidence says where a finding came from, never what the content was. The `Debug` output of
//! these types prints counts and fixed names only.

use std::fmt;

use super::hosts::HostSet;
use super::secrets::{is_secret_token, safe_pointer};

/// The finding that two tool names have the same normal form in one request.
pub const NAME_COLLISION: &str = "name_collision";
/// The finding that the arguments of a call are not one strict JSON object.
pub const UNPARSED_ARGUMENTS: &str = "unparsed_arguments";
/// The finding that a call names a tool that the request does not declare.
pub const UNDECLARED_TOOL: &str = "undeclared_tool";
/// The finding that the arguments do not match the schema of the tool.
pub const INVALID_ARGS: &str = "invalid_args";
/// The finding that the arguments hold a host that the session has not seen.
pub const UNSEEN_HOST: &str = "unseen_host";
/// The finding that the arguments hold a secret.
pub const SECRET_IN_ARGS: &str = "secret_in_args";
/// The finding that the arguments hold a prompt injection marker.
pub const INJECTION_MARKERS: &str = "injection_markers";
/// The finding that the calls of one history belong to more than one session (SMA-753).
pub const MIXED_SESSIONS: &str = "mixed_sessions";
/// The finding that the history of a request contains a call that the gateway did not see (SMA-753).
pub const FORGED_HISTORY: &str = "forged_history";
/// The finding that the history store cut the events of a history at a cap or at the retention
/// limit (SMA-753).
pub const HISTORY_TRUNCATED: &str = "history_truncated";
/// The finding that a detector failed.
pub const DETECTOR_ERROR: &str = "detector_error";
/// The detector name for a finding that the kernel itself makes.
pub const DETECTOR_KERNEL: &str = "kernel";
/// The detector name for the stage of the built-in detectors.
pub const DETECTOR_BUILT_IN: &str = "built_in";

/// The severity value when there are no findings.
pub const NO_SEVERITY: i64 = -1;

/// The most evidence entries that one finding keeps.
const MAX_EVIDENCE: usize = 16;
/// The longest JSON pointer, in bytes, that an evidence entry keeps.
const MAX_POINTER: usize = 256;

/// How serious a finding is. The order is the order of the levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Severity {
    /// For information only.
    Info,
    /// Low.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
}

impl Severity {
    /// The value in `context.max_severity`: 0 for `Info` to 3 for `High`.
    #[must_use]
    pub fn as_long(self) -> i64 {
        match self {
            Self::Info => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
        }
    }

    /// The fixed spelling of the level.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// Where in the input a finding came from.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Location {
    /// A value in the arguments, by JSON pointer.
    Argument {
        /// The JSON pointer of the value.
        pointer: String,
    },
    /// The free text of a call that has no JSON arguments.
    TextInput,
    /// A tool result, by its index.
    Result {
        /// The index of the result.
        index: usize,
    },
}

impl fmt::Debug for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Argument { .. } => f.write_str("Argument"),
            Self::TextInput => f.write_str("TextInput"),
            Self::Result { index } => write!(f, "Result {{ index: {index} }}"),
        }
    }
}

/// One place where a rule matched.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Evidence {
    /// The fixed identifier of the rule.
    pub rule: &'static str,
    /// Where the rule matched.
    pub location: Location,
    /// The host that the rule matched, if the rule is about a host.
    pub host: Option<String>,
}

impl Evidence {
    /// A match in the arguments. The pointer is cut before the first token that is a secret (a key
    /// can be one), then to 256 bytes on a character boundary.
    #[must_use]
    pub fn argument(rule: &'static str, pointer: &str) -> Self {
        Self {
            rule,
            location: Location::Argument { pointer: stored_pointer(pointer) },
            host: None,
        }
    }

    /// A match of a host. `pointer` is `None` when the host is in free text. A host that holds a
    /// known token form is not stored (`host` is `None`), because no evidence copy holds a secret
    /// (§ 6.2.1). The finding and the location stay.
    #[must_use]
    pub fn host(rule: &'static str, pointer: Option<&str>, host: String) -> Self {
        let location = match pointer {
            Some(pointer) => Location::Argument { pointer: stored_pointer(pointer) },
            None => Location::TextInput,
        };
        let host = (!is_secret_token(&host)).then_some(host);
        Self { rule, location, host }
    }

    /// A match in the free text of a call.
    #[must_use]
    pub fn text_input(rule: &'static str) -> Self {
        Self {
            rule,
            location: Location::TextInput,
            host: None,
        }
    }

    /// A match in the tool result with this index.
    #[must_use]
    pub fn result(rule: &'static str, index: usize) -> Self {
        Self {
            rule,
            location: Location::Result { index },
            host: None,
        }
    }
}

impl fmt::Debug for Evidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Evidence {{ rule: {}, location: {:?} }}", self.rule, self.location)
    }
}

/// The first `max` bytes of `s` or less, cut on a character boundary.
pub(super) fn prefix_on_boundary(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The pointer that an evidence entry stores. The secret cut runs first, on the whole pointer, so
/// that the length cut cannot split a secret key and leave a part of it. A length cut that ends
/// inside an escape (`~0` or `~1`) drops the lone `~`, so the stored pointer stays valid RFC 6901
/// syntax: every `~` of a pointer starts an escape.
fn stored_pointer(pointer: &str) -> String {
    let safe = safe_pointer(pointer);
    let cut = prefix_on_boundary(safe, MAX_POINTER);
    let cut = if cut.len() < safe.len() { cut.strip_suffix('~').unwrap_or(cut) } else { cut };
    cut.to_owned()
}

/// One finding of one detector. A finding never blocks a call by itself.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Finding {
    /// The name of the detector.
    pub detector: String,
    /// The kind of the finding.
    pub kind: String,
    /// How serious the finding is.
    pub severity: Severity,
    /// The version of the detector rules.
    pub version: String,
    /// Where the finding came from. It holds at most 16 entries.
    pub evidence: Vec<Evidence>,
}

impl Finding {
    /// A finding with no evidence.
    #[must_use]
    pub fn new(detector: impl Into<String>, kind: impl Into<String>, severity: Severity, version: impl Into<String>) -> Self {
        Self {
            detector: detector.into(),
            kind: kind.into(),
            severity,
            version: version.into(),
            evidence: Vec::new(),
        }
    }

    /// Add one evidence entry. An entry past the sixteenth is dropped.
    #[must_use]
    pub fn with_evidence(mut self, evidence: Evidence) -> Self {
        if self.evidence.len() < MAX_EVIDENCE {
            self.evidence.push(evidence);
        }
        self
    }
}

impl fmt::Debug for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Finding {{ detector: {}, kind: {}, severity: {}, version: {}, evidence: {} }}",
            self.detector,
            self.kind,
            self.severity.as_str(),
            self.version,
            self.evidence.len()
        )
    }
}

/// The finding for a detector that failed. It is `high` and has no version.
#[must_use]
pub fn detector_error(detector: impl Into<String>) -> Finding {
    Finding::new(detector, DETECTOR_ERROR, Severity::High, "")
}

/// The arguments of a tool call, as the model sent them.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolInput {
    /// The raw JSON text. The kernel parses it and reads each number from its original text.
    Json(String),
    /// Free text, for the tool kinds `custom`, `apply_patch` and `other`.
    Text(String),
}

/// The variant and the length only: the arguments can hold a secret (A2).
impl fmt::Debug for ToolInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolInput::Json(s) => write!(f, "Json {{ len: {} }}", s.len()),
            ToolInput::Text(s) => write!(f, "Text {{ len: {} }}", s.len()),
        }
    }
}

/// Whether the name of a call is among the tool definitions of the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Declared {
    /// The name is among the definitions.
    Yes,
    /// The definitions are complete and the name is not among them.
    No,
    /// The definitions are not complete, so the kernel cannot tell.
    Unknown,
}

impl Declared {
    /// The value in `context.call.declared`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
            Self::Unknown => "unknown",
        }
    }
}

/// What the call detectors read about one tool call.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct CallSubject<'a> {
    /// The raw name, as the model sent it.
    pub name: &'a str,
    /// The arguments of the call.
    pub input: &'a ToolInput,
    /// Whether the request declares the tool.
    pub declared: Declared,
    /// The JSON Schema of the matching definition.
    pub schema: Option<&'a serde_json::Value>,
    /// The hosts of the seen message sources. `None`: no screen ran, so `unseen_host` does not run.
    pub seen_hosts: Option<&'a HostSet>,
}

impl<'a> CallSubject<'a> {
    /// A subject with no schema and no seen hosts.
    #[must_use]
    pub fn new(name: &'a str, input: &'a ToolInput, declared: Declared) -> Self {
        Self {
            name,
            input,
            declared,
            schema: None,
            seen_hosts: None,
        }
    }

    /// Set the JSON Schema of the matching definition.
    #[must_use]
    pub fn with_schema(mut self, schema: &'a serde_json::Value) -> Self {
        self.schema = Some(schema);
        self
    }

    /// Set the hosts of the seen message sources.
    #[must_use]
    pub fn with_seen_hosts(mut self, seen_hosts: &'a HostSet) -> Self {
        self.seen_hosts = Some(seen_hosts);
        self
    }
}

impl fmt::Debug for CallSubject<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CallSubject {{ declared: {}, schema: {}, seen_hosts: {} }}",
            self.declared.as_str(),
            self.schema.is_some(),
            self.seen_hosts.is_some()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_levels_and_spellings() {
        let all = [Severity::Info, Severity::Low, Severity::Medium, Severity::High];
        assert_eq!(all.map(Severity::as_long), [0, 1, 2, 3]);
        assert_eq!(all.map(Severity::as_str), ["info", "low", "medium", "high"]);
        assert!(Severity::High > Severity::Medium && NO_SEVERITY < Severity::Info.as_long());
    }

    #[test]
    fn evidence_is_capped_at_16() {
        let mut f = Finding::new("d", "k", Severity::Low, "v1");
        for i in 0..20 {
            f = f.with_evidence(Evidence::result("r", i));
        }
        assert_eq!(f.evidence.len(), 16);
        assert_eq!(f.evidence[15].location, Location::Result { index: 15 });
    }

    #[test]
    fn a_long_pointer_is_cut_on_a_char_boundary() {
        let e = Evidence::argument("r", &format!("/{}", "é".repeat(200)));
        let Location::Argument { pointer } = &e.location else { panic!("not an argument") };
        assert!(pointer.len() <= 256 && pointer.len() > 250);
        let h = Evidence::host("r", Some(&format!("/{}", "é".repeat(200))), "h".into());
        let Location::Argument { pointer } = &h.location else { panic!("not an argument") };
        assert!(pointer.len() <= 256);
    }

    #[test]
    fn a_cut_inside_an_escape_drops_the_lone_tilde() {
        // 254 bytes of key, then `~1`: the 256-byte cut ends after the `~`.
        let long = format!("/{}~1", "k".repeat(254));
        let Location::Argument { pointer } = Evidence::argument("r", &long).location else {
            panic!("not an argument")
        };
        assert_eq!(pointer, format!("/{}", "k".repeat(254)));
        // A pointer that fits keeps its escapes.
        let Location::Argument { pointer } = Evidence::argument("r", "/a~1b/c~0").location else {
            panic!("not an argument")
        };
        assert_eq!(pointer, "/a~1b/c~0");
    }

    #[test]
    fn a_host_that_holds_a_secret_is_not_stored() {
        let token = ["xoxb-", "123456789012-abcdefghij"].concat();
        let e = Evidence::host("url", Some("/u"), format!("{token}.evil.example"));
        assert_eq!((e.host, e.location), (None, Location::Argument { pointer: "/u".into() }));
    }

    #[test]
    fn a_host_without_a_pointer_is_in_the_text_input() {
        let e = Evidence::host("r", None, "h.example".into());
        assert_eq!(e.location, Location::TextInput);
        assert_eq!(e.host.as_deref(), Some("h.example"));
        assert_eq!(Evidence::text_input("r").location, Location::TextInput);
    }

    #[test]
    fn detector_error_is_high_with_no_version() {
        let f = detector_error("built_in");
        assert_eq!((f.kind.as_str(), f.severity, f.version.as_str()), ("detector_error", Severity::High, ""));
        assert_eq!(f.detector, "built_in");
    }

    #[test]
    fn debug_prints_no_content() {
        const CANARY: &str = "CANARY-7f3a";
        let f = Finding::new("unseen_host", "unseen_host", Severity::Medium, "v1")
            .with_evidence(Evidence::host("url", Some(&format!("/{CANARY}")), format!("{CANARY}.example")))
            .with_evidence(Evidence::argument("type", &format!("/{CANARY}")));
        for s in [format!("{f:?}"), format!("{:?}", f.evidence[0]), format!("{:?}", f.evidence[0].location)] {
            assert!(!s.contains(CANARY), "{s}");
        }
        assert!(format!("{f:?}").contains("unseen_host") && format!("{f:?}").contains("evidence: 2"));
        for input in [ToolInput::Json(format!(r#"{{"k":"{CANARY}"}}"#)), ToolInput::Text(CANARY.to_owned())] {
            let s = format!("{input:?}");
            assert!(!s.contains(CANARY) && s.contains("len: "), "{s}");
        }
    }
}
