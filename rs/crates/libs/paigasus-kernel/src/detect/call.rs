// SPDX-License-Identifier: Apache-2.0
//! The call built-ins `undeclared_tool` and `unseen_host`, and `detect_call` (§ 6.3).

use std::collections::BTreeMap;
use std::fmt;

use super::hosts::scan_hosts;
use super::schema::invalid_args;
use super::secrets::{safe_pointer, secret_in_args};
use super::types::prefix_on_boundary;
use super::{CallSubject, Declared, Evidence, Finding, HOSTS_OVERFLOW, HostSet, Severity, UNDECLARED_TOOL, UNSEEN_HOST};

/// The version of the `unseen_host` rule.
const UNSEEN_HOST_VERSION: &str = "v1";
/// The most evidence entries of one finding.
const MAX_EVIDENCE: usize = 16;
/// A pointer is cut to this many bytes before it is copied. `Evidence` cuts it to the same size.
const MAX_POINTER_COPY: usize = 256;

/// The result of the call detectors for one call.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CallDetection {
    /// The findings, in the order `undeclared_tool`, `invalid_args`, `unseen_host`, `secret_in_args`.
    pub findings: Vec<Finding>,
    /// Every host of the call.
    pub hosts: HostSet,
}

impl fmt::Debug for CallDetection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CallDetection {{ findings: {}, hosts: {} }}", self.findings.len(), self.hosts.len())
    }
}

/// `undeclared_tool`: the request does not declare the tool (`Declared::No` only, D17). Medium.
#[must_use]
pub fn undeclared_tool(subject: &CallSubject<'_>) -> Option<Finding> {
    (subject.declared == Declared::No).then(|| Finding::new(UNDECLARED_TOOL, UNDECLARED_TOOL, Severity::Medium, "v1"))
}

/// The unseen hosts of a call: the smallest 16, each with the pointer of its first leaf.
/// This is the one builder of the `unseen_host` evidence.
struct Unseen {
    found: BTreeMap<String, Option<String>>,
}

impl Unseen {
    fn new() -> Self {
        Self { found: BTreeMap::new() }
    }

    /// Offer one host. The sentinel is not a host name. A host that is not among the 16 smallest
    /// is not copied, and a pointer is cut before it is copied.
    fn offer(&mut self, seen: &HostSet, host: &str, pointer: Option<&str>) {
        if host == HOSTS_OVERFLOW || seen.contains(host) || self.found.contains_key(host) {
            return;
        }
        if self.found.len() >= MAX_EVIDENCE && self.found.last_key_value().is_some_and(|(last, _)| host > last.as_str()) {
            return;
        }
        self.found.insert(host.to_owned(), pointer.map(|p| prefix_on_boundary(p, MAX_POINTER_COPY).to_owned()));
        if self.found.len() > MAX_EVIDENCE {
            self.found.pop_last();
        }
    }

    fn finish(self) -> Option<Finding> {
        if self.found.is_empty() {
            return None;
        }
        let mut finding = Finding::new(UNSEEN_HOST, UNSEEN_HOST, Severity::Medium, UNSEEN_HOST_VERSION);
        for (host, pointer) in self.found {
            finding = finding.with_evidence(Evidence::host(UNSEEN_HOST, pointer.as_deref(), host));
        }
        Some(finding)
    }
}

/// `unseen_host`: a host of the call that is not in the seen set. Medium, at most 16 evidence
/// entries, no pointer. The sentinel `<overflow>` is not a host name: it never gives a finding or
/// an evidence entry by itself. The kernel reports an overflow as a `detector_error` (D36).
#[must_use]
pub fn unseen_host(seen: &HostSet, hosts: &HostSet) -> Option<Finding> {
    let mut unseen = Unseen::new();
    for host in hosts.iter() {
        unseen.offer(seen, host, None);
    }
    unseen.finish()
}

/// Run the four call detectors on one call. One walk extracts the hosts and the unseen hosts
/// with their pointers. `unseen_host` runs only when `seen_hosts` is `Some`.
#[must_use]
pub fn detect_call(subject: &CallSubject<'_>) -> CallDetection {
    let mut hosts = HostSet::new();
    let mut unseen = Unseen::new();
    scan_hosts(subject.input, &mut |leaf_hosts, pointer| {
        hosts.extend(leaf_hosts);
        if let Some(seen) = subject.seen_hosts {
            // The secret cut runs once per leaf with an unseen host, before any pointer is copied.
            let mut safe: Option<Option<&str>> = None;
            for host in leaf_hosts.iter().filter(|h| *h != HOSTS_OVERFLOW && !seen.contains(h)) {
                let pointer = *safe.get_or_insert_with(|| pointer.map(safe_pointer));
                unseen.offer(seen, host, pointer);
            }
        }
    });
    let unseen = subject.seen_hosts.and_then(|_| unseen.finish());
    let findings = [undeclared_tool(subject), invalid_args(subject), unseen, secret_in_args(subject)].into_iter().flatten().collect();
    CallDetection { findings, hosts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{INVALID_ARGS, Location, SECRET_IN_ARGS, Severity, ToolInput, UNDECLARED_TOOL, UNSEEN_HOST};
    use serde_json::json;

    fn aws() -> String {
        ["AKIA", "ABCDEFGHIJKLMNOP"].concat()
    }
    fn kinds(d: &CallDetection) -> Vec<&str> {
        d.findings.iter().map(|f| f.kind.as_str()).collect()
    }

    #[test]
    fn undeclared_tool_only_for_no() {
        let input = ToolInput::Json("{}".into());
        for (d, n) in [(Declared::Yes, 0), (Declared::No, 1), (Declared::Unknown, 0)] {
            let found = undeclared_tool(&CallSubject::new("t", &input, d));
            assert_eq!(found.iter().count(), n);
            if let Some(f) = found {
                assert_eq!((f.kind.as_str(), f.severity, f.evidence.len()), (UNDECLARED_TOOL, Severity::Medium, 0));
            }
        }
    }

    #[test]
    fn unseen_host_needs_a_seen_set() {
        let input = ToolInput::Json(r#"{"u":"https://new.example/","v":"https://old.example/"}"#.into());
        let none = detect_call(&CallSubject::new("t", &input, Declared::Yes));
        assert!(none.findings.iter().all(|f| f.kind != UNSEEN_HOST));
        assert_eq!(none.hosts.len(), 2);
        let mut seen = HostSet::new();
        seen.insert("old.example");
        let d = detect_call(&CallSubject::new("t", &input, Declared::Yes).with_seen_hosts(&seen));
        let f = d.findings.iter().find(|f| f.kind == UNSEEN_HOST).expect("finding");
        assert_eq!((f.severity, f.evidence.len(), f.evidence[0].host.as_deref()), (Severity::Medium, 1, Some("new.example")));
        assert_eq!(f.evidence[0].location, Location::Argument { pointer: "/u".to_owned() });
    }

    #[test]
    fn unseen_host_standalone_has_no_pointer_and_caps_evidence() {
        let mut hosts = HostSet::new();
        for i in 0..40 {
            hosts.insert(&format!("h{i:02}.example"));
        }
        let seen = HostSet::new();
        let f = unseen_host(&seen, &hosts).expect("finding");
        assert_eq!(f.evidence.len(), 16);
        assert!(f.evidence.iter().all(|e| e.location == Location::TextInput));
        assert_eq!(f.evidence[0].host.as_deref(), Some("h00.example"));
        assert!(unseen_host(&hosts, &hosts).is_none());
    }

    #[test]
    fn the_overflow_sentinel_is_never_an_unseen_host_name() {
        let mut hosts = HostSet::new();
        for i in 0..300 {
            hosts.insert(&format!("h{i:03}.example"));
        }
        assert!(hosts.overflowed());
        // The sentinel alone is not an unseen host: the kernel reports the overflow itself.
        assert!(unseen_host(&hosts, &hosts).is_none());
        let f = unseen_host(&HostSet::new(), &hosts).expect("finding");
        assert!(f.evidence.iter().all(|e| e.host.as_deref() != Some(crate::detect::HOSTS_OVERFLOW)));
    }

    fn pointers(f: &Finding) -> Vec<String> {
        f.evidence
            .iter()
            .map(|e| match &e.location {
                Location::Argument { pointer } => pointer.clone(),
                other => panic!("unexpected location {other:?}"),
            })
            .collect()
    }

    #[test]
    fn no_evidence_pointer_holds_a_secret_key() {
        let token = aws();
        let secret_part = &token[4..];
        let schema = json!({"type":"object","properties":{token.clone():{"type":"object","properties":{"v":{"type":"integer"}}}}});
        let input = ToolInput::Json(format!(r#"{{"{token}":{{"v":"x","u":"https://x.example"}}}}"#));
        let seen = HostSet::new();
        let subject = CallSubject::new("t", &input, Declared::Yes).with_schema(&schema).with_seen_hosts(&seen);
        let d = detect_call(&subject);
        assert_eq!(kinds(&d), [INVALID_ARGS, UNSEEN_HOST, SECRET_IN_ARGS]);
        for f in &d.findings {
            for p in pointers(f) {
                assert!(!p.contains(secret_part), "{} {p}", f.kind);
            }
        }
        // The pointer is cut before the key: the pointer of the object that holds it.
        assert_eq!(pointers(&d.findings[1]), [""]);
        // The same holds for the escaped form of the key.
        let escaped = token.replace('A', "\\u0041");
        let input = ToolInput::Json(format!(r#"{{"{escaped}":{{"u":"https://x.example"}}}}"#));
        let d = detect_call(&CallSubject::new("t", &input, Declared::Yes).with_seen_hosts(&seen));
        assert!(pointers(&d.findings[0]).iter().all(|p| !p.contains(secret_part)));
    }

    #[test]
    fn a_sentinel_only_call_gives_no_unseen_host_finding() {
        let input = ToolInput::Json(format!(r#"{{"u":"https://{}.example/"}}"#, "a".repeat(1100)));
        let seen = HostSet::new();
        let d = detect_call(&CallSubject::new("t", &input, Declared::Yes).with_seen_hosts(&seen));
        assert!(d.findings.iter().all(|f| f.kind != UNSEEN_HOST));
        assert_eq!(d.hosts.iter().collect::<Vec<_>>(), [crate::detect::HOSTS_OVERFLOW]);
    }

    #[test]
    fn a_seen_host_matches_after_normalising() {
        let input = ToolInput::Json(r#"{"u":"http://BÜCHER.example.:8443/x"}"#.into());
        let mut seen = HostSet::new();
        seen.insert("xn--bcher-kva.example");
        let d = detect_call(&CallSubject::new("t", &input, Declared::Yes).with_seen_hosts(&seen));
        assert!(d.findings.is_empty(), "{d:?}");
        assert_eq!(d.hosts.iter().collect::<Vec<_>>(), ["xn--bcher-kva.example"]);
    }

    #[test]
    fn detect_call_runs_the_four_in_order() {
        let schema = json!({"type":"object","properties":{"n":{"type":"integer"}}});
        let text = format!(r#"{{"n":"x","u":"https://new.example/","k":"{}"}}"#, aws());
        let input = ToolInput::Json(text);
        let seen = HostSet::new();
        let subject = CallSubject::new("t", &input, Declared::No).with_schema(&schema).with_seen_hosts(&seen);
        let d = detect_call(&subject);
        assert_eq!(kinds(&d), [UNDECLARED_TOOL, INVALID_ARGS, UNSEEN_HOST, SECRET_IN_ARGS]);
        assert_eq!(d.hosts.iter().collect::<Vec<_>>(), ["new.example"]);
    }

    #[test]
    fn detection_is_deterministic() {
        let schema = json!({"type":"object","properties":{"n":{"type":"integer"}}});
        let input = ToolInput::Json(format!(r#"{{"n":"x","a":"https://b.example/ https://a.example","k":"{}"}}"#, aws()));
        let seen = HostSet::new();
        let subject = CallSubject::new("t", &input, Declared::No).with_schema(&schema).with_seen_hosts(&seen);
        let (a, b) = (detect_call(&subject), detect_call(&subject));
        assert_eq!(a.findings, b.findings);
        assert_eq!(a.hosts, b.hosts);
    }

    #[test]
    fn debug_of_subject_and_detection_prints_no_content() {
        let canary = "CANARYvalue123";
        let name = format!("name-{canary}");
        let input = ToolInput::Json(format!(r#"{{"u":"https://{canary}.example/","k":"{}"}}"#, aws()));
        let seen = HostSet::new();
        let subject = CallSubject::new(&name, &input, Declared::No).with_seen_hosts(&seen);
        let d = detect_call(&subject);
        let text = format!("{subject:?} {d:?}");
        assert!(!text.to_lowercase().contains(&canary.to_lowercase()), "{text}");
        assert!(!text.contains("AKIA"), "{text}");
    }

    #[test]
    fn a_large_adversarial_call_is_linear_and_its_evidence_is_cut() {
        // About 4 MiB: one long key (a long pointer), many URLs, and near-miss tokens.
        let key = "k".repeat(1 << 20);
        let mut body = String::new();
        for i in 0..80_000 {
            body.push_str(&format!("https://h{i}.example/p AKIA123 ghp_short "));
        }
        let text = format!(r#"{{"{key}":{{"u":"{body}","t":"{}"}}}}"#, aws());
        assert!(text.len() > 1 << 22);
        let input = ToolInput::Json(text);
        let seen = HostSet::new();
        let subject = CallSubject::new("t", &input, Declared::Yes).with_seen_hosts(&seen);
        let start = std::time::Instant::now();
        let d = detect_call(&subject);
        assert!(start.elapsed() < std::time::Duration::from_secs(60), "{:?}", start.elapsed());
        assert_eq!(kinds(&d), [UNSEEN_HOST, SECRET_IN_ARGS]);
        assert!(d.hosts.overflowed());
        let f = &d.findings[0];
        assert_eq!(f.evidence.len(), 16);
        for e in &f.evidence {
            match &e.location {
                Location::Argument { pointer } => assert!(pointer.len() <= 256),
                other => panic!("unexpected location {other:?}"),
            }
            assert_ne!(e.host.as_deref(), Some(crate::detect::HOSTS_OVERFLOW));
        }
        assert_eq!(f.evidence[0].host.as_deref(), Some("h0.example"));
    }
}
