// SPDX-License-Identifier: Apache-2.0

//! The decision log port (SMA-755 § 5.5, P-8 to P-10). The recorder builds a record and hands it to
//! a [`DecisionLog`]. The port never fails: a log that cannot keep a record counts the loss itself.

#[cfg(test)]
use std::sync::Mutex;

use paigasus_gateway_core::decision_log::{AppliedEffect, DecisionRecord, FindingRecord};
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

/// The findings of a line: `kind:severity` per finding, in record order, `unknown` for an unknown
/// severity. Never the evidence (SMA-855 D3): a record has none.
fn findings_field(list: &[FindingRecord]) -> String {
    list.iter()
        .map(|f| format!("{}:{}", f.kind, f.severity.map_or("unknown", |s| s.as_str())))
        .collect::<Vec<_>>()
        .join(",")
}

/// The per-decision log line. It has the fields of the line that `decide_now` wrote before the decision log.
/// The recorder writes it exactly when the decider ran for the report entry (`ReportedCall.detail` is set), so
/// the set of lines stays the same as before the decision log (P-10, R11). A request-side record, an
/// unenforceable entry and a `block_all` entry have no line. `verdict` is the applied effect: an observed hold
/// writes `observe`. `findings` lists the kinds and severities (SMA-855). It never holds the arguments
/// (A1) or the evidence of a finding (SMA-855 D3).
pub fn log_line(r: &DecisionRecord) {
    let verdict = match r.applied {
        AppliedEffect::Allow => "allow",
        AppliedEffect::Observe => "observe",
        _ => "block",
    };
    let (determining, errored, findings) = r.cedar.as_ref().map_or_else(
        || (String::new(), String::new(), String::new()),
        |c| (refs(&c.determining), refs(&c.errored), findings_field(&c.findings)),
    );
    tracing::info!(
        request_id = r.request_id.as_deref().unwrap_or(""),
        tool = %r.tool.name,
        verdict,
        basis = r.basis.as_str(),
        mode = r.mode.map_or("", |m| m.as_str()),
        determining = %determining,
        errored = %errored,
        findings = %findings,
        "tool-policy decision"
    );
}

/// The log of the default configuration: it keeps nothing (the recorder writes the tracing line). It keeps no digest.
#[derive(Debug, Clone, Copy, Default)]
pub struct TracingDecisionLog;

impl DecisionLog for TracingDecisionLog {
    fn wants_digest(&self) -> bool {
        false
    }

    fn record(&self, _record: DecisionRecord) {}
}

/// A log that keeps the records in memory, for tests. Only test builds have it: the runtime never builds it
/// (P-9), and no integration test or other crate uses it.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct MemoryDecisionLog {
    wants_digest: bool,
    records: Mutex<Vec<DecisionRecord>>,
}

#[cfg(test)]
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

#[cfg(test)]
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
    use paigasus_gateway_core::DecisionContext;
    use paigasus_gateway_core::decision_log::{RecordHeader, build_refusal};
    use std::time::SystemTime;
    use uuid::Uuid;

    fn header() -> RecordHeader {
        RecordHeader::new(Uuid::nil(), SystemTime::UNIX_EPOCH, Uuid::nil(), 1, None)
    }

    fn ctx() -> DecisionContext {
        DecisionContext::new("prn:pgs:iam:::user/u", "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1", Some("req-1".into()))
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
