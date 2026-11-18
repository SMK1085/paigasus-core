// SPDX-License-Identifier: Apache-2.0

//! The decision log port (SMA-755 § 5.5, P-8 to P-10). The recorder builds a record and hands it to
//! a [`DecisionLog`]. The port never fails: a log that cannot keep a record counts the loss itself.

use std::sync::Mutex;

use paigasus_gateway_core::decision_log::{AppliedEffect, DecisionRecord, RecordBasis, Side};
use paigasus_gateway_core::sanitize_name;
use paigasus_kernel::tool_policy::{PolicyOrigin, PolicyRef};

/// Where the decision records go.
pub trait DecisionLog: Send + Sync {
    /// Whether the log keeps the keyed digest of the arguments. A log that does not keep it makes
    /// the recorder skip the HMAC.
    fn wants_digest(&self) -> bool;

    /// Take one record. It never blocks and never fails.
    fn record(&self, record: DecisionRecord);
}

/// The entropy of a decision id (the 80 random bits of a UUIDv7).
pub trait Entropy: Send + Sync {
    fn bytes10(&self) -> [u8; 10];
}

/// The production entropy: the operating system generator through `rand`.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsEntropy;

impl Entropy for OsEntropy {
    fn bytes10(&self) -> [u8; 10] {
        rand::random()
    }
}

fn refs(list: &[PolicyRef]) -> String {
    list.iter()
        .map(|r| {
            let set = match r.set {
                PolicyOrigin::Floor => "floor",
                _ => "tenant",
            };
            format!("{set}:{}", sanitize_name(&r.id))
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// The per-decision log line. It has the fields of the line that `decide_now` writes. It writes
/// nothing for a request-side record and nothing for an `OnFailure` record, so the set of lines
/// stays the same as before the decision log (P-10). It never holds the arguments (A1).
pub fn log_line(r: &DecisionRecord) {
    if r.side != Side::Response || r.basis == RecordBasis::OnFailure {
        return;
    }
    let verdict = match r.applied {
        AppliedEffect::Allow => "allow",
        AppliedEffect::Observe => "observe",
        _ => "block",
    };
    let (determining, errored) = r.cedar.as_ref().map_or_else(|| (String::new(), String::new()), |c| (refs(&c.determining), refs(&c.errored)));
    tracing::info!(
        request_id = r.request_id.as_deref().unwrap_or(""),
        tool = %r.tool.name,
        verdict,
        basis = r.basis.as_str(),
        mode = r.mode.map_or("", |m| m.as_str()),
        determining = %determining,
        errored = %errored,
        "tool-policy decision"
    );
}

/// The log of the default configuration: the tracing line only. It keeps no digest.
#[derive(Debug, Clone, Copy, Default)]
pub struct TracingDecisionLog;

impl DecisionLog for TracingDecisionLog {
    fn wants_digest(&self) -> bool {
        false
    }

    fn record(&self, record: DecisionRecord) {
        log_line(&record);
    }
}

/// A log that keeps the records in memory, for tests. The runtime never builds it (P-9).
#[derive(Debug, Default)]
pub struct MemoryDecisionLog {
    wants_digest: bool,
    records: Mutex<Vec<DecisionRecord>>,
}

impl MemoryDecisionLog {
    #[must_use]
    pub fn new(wants_digest: bool) -> Self {
        Self {
            wants_digest,
            records: Mutex::new(Vec::new()),
        }
    }

    /// The records kept so far, in the order of `record`.
    #[must_use]
    pub fn records(&self) -> Vec<DecisionRecord> {
        self.records.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

impl DecisionLog for MemoryDecisionLog {
    fn wants_digest(&self) -> bool {
        self.wants_digest
    }

    fn record(&self, record: DecisionRecord) {
        self.records.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(record);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_gateway_core::decision_log::{RecordHeader, ReportedCall, build_refusal, build_response};
    use paigasus_gateway_core::{Basis, BlockCode, DecisionContext, ReportDecision, ReportEntry};
    use paigasus_logging::test_support::capture_logs_at;
    use std::time::SystemTime;
    use uuid::Uuid;

    fn header() -> RecordHeader {
        RecordHeader::new(Uuid::nil(), SystemTime::UNIX_EPOCH, Uuid::nil(), 1, None)
    }

    fn entry(basis: Option<Basis>) -> ReportEntry {
        ReportEntry {
            choice: 0,
            position: 0,
            name: "Get_Weather".into(),
            decision: ReportDecision::Block,
            basis,
            code: Some(BlockCode::Policy),
            enforced: Some(true),
            reason: None,
        }
    }

    fn ctx() -> DecisionContext {
        DecisionContext::new("prn:pgs:iam:::user/u", "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1", Some("req-1".into()))
    }

    #[test]
    fn the_tracing_log_line_has_no_arguments() {
        let (logs, _guard) = capture_logs_at(tracing::Level::INFO);
        let rec = build_response(header(), &ctx(), &ReportedCall::new(entry(Some(Basis::Decider)), None, None));
        TracingDecisionLog.record(rec);
        let text = logs.text();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("tool-policy decision"), "{text}");
        assert!(text.contains("request_id=\"req-1\"") || text.contains("request_id=req-1"), "{text}");
        assert!(text.contains("tool=Get_Weather"), "{text}");
        assert!(text.contains("verdict=\"block\"") || text.contains("verdict=block"), "{text}");
        assert!(text.contains("basis=\"policy\"") || text.contains("basis=policy"), "{text}");
        for field in ["mode=", "determining=", "errored="] {
            assert!(text.contains(field), "{field} in {text}");
        }
        assert!(!text.contains("s3cr3t-canary"), "{text}");
    }

    #[test]
    fn an_on_failure_or_request_record_writes_no_line() {
        let (logs, _guard) = capture_logs_at(tracing::Level::INFO);
        let on_failure = build_response(header(), &ctx(), &ReportedCall::new(entry(Some(Basis::OnFailure)), None, None));
        assert_eq!(on_failure.basis, RecordBasis::OnFailure);
        TracingDecisionLog.record(on_failure);
        let request = build_refusal(header(), &ctx(), None);
        assert_eq!(request.side, Side::Request);
        TracingDecisionLog.record(request);
        assert_eq!(logs.text(), "");
    }

    #[test]
    fn policy_ids_are_sanitized_in_logs() {
        let r = [PolicyRef::new(PolicyOrigin::Floor, "f 1"), PolicyRef::new(PolicyOrigin::Tenant, "t\n2")];
        assert_eq!(refs(&r), "floor:f_1,tenant:t_2");
    }

    #[test]
    fn the_memory_log_keeps_records_in_order() {
        let log = MemoryDecisionLog::new(true);
        assert!(log.wants_digest());
        assert!(!TracingDecisionLog.wants_digest());
        log.record(build_refusal(header(), &ctx(), None));
        assert_eq!(log.records().len(), 1);
    }
}
