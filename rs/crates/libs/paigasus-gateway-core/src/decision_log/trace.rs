// SPDX-License-Identifier: Apache-2.0
//! The Dogwood trace encoder (SMA-755 § 5.11). Pure: no I/O and no clock. One response-side
//! record gives one trace line. A record that Dogwood cannot replay exactly is skipped. One
//! exception: the call hosts are not encoded. Every line writes `hosts: []` (SMA-855 D29), so a
//! policy that reads `context.call.hosts` can replay with another verdict than the live one. The
//! export CLI warns about this.

use std::fmt;
use std::time::SystemTime;

use paigasus_kernel::detect::{NO_SEVERITY, Severity};
use uuid::Uuid;

use super::record::{DecisionRecord, Side};

/// Why `encode` made no line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkipReason {
    /// Dogwood decides only `request` events, and the response side is the decided one.
    RequestSide,
    NoOrg,
    NoPrincipal,
    /// A cut shortened a field, so the action id is not exact.
    Truncated,
    /// The head parser of Dogwood cannot carry the action id (P-11).
    UnsafeActionId,
}

impl SkipReason {
    /// The stable label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SkipReason::RequestSide => "request side",
            SkipReason::NoOrg => "no org",
            SkipReason::NoPrincipal => "no principal",
            SkipReason::Truncated => "truncated",
            SkipReason::UnsafeActionId => "unsafe action id",
        }
    }
}

/// One trace line, without the newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceLine {
    ts: i64,
    text: String,
}

impl TraceLine {
    /// The timestamp of the line, in Unix seconds.
    #[must_use]
    pub fn ts(&self) -> i64 {
        self.ts
    }
}

impl fmt::Display for TraceLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// The result of `encode`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encoded {
    Line(TraceLine),
    Skipped(SkipReason),
}

/// The Cedar string form that Dogwood's `unescape` reads (F26).
#[must_use]
pub fn escape(s: &str) -> String {
    s.escape_debug().to_string()
}

/// Whether the head parser of Dogwood carries `id` (P-11). It reads the head with an unescaped
/// `find('(')`, and the spec keeps `)`, `"` and every control character out as a superset.
#[must_use]
pub fn is_safe_action_id(id: &str) -> bool {
    !id.chars().any(|c| matches!(c, '(' | ')' | '"') || c.is_control())
}

/// `decided_at` in Unix seconds, rounded down (F23).
fn unix_seconds(t: SystemTime) -> i64 {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => {
            let d = e.duration();
            let secs = i64::try_from(d.as_secs()).unwrap_or(i64::MAX);
            -secs - i64::from(d.subsec_nanos() > 0)
        }
    }
}

/// The export order of § 5.11: `(second, decided_at, instance_id, sequence)`.
#[must_use]
pub fn order_key(r: &DecisionRecord) -> (i64, SystemTime, Uuid, u64) {
    (unix_seconds(r.decided_at), r.decided_at, r.instance_id, r.sequence)
}

/// The trace line of a record, or the reason for no line.
#[must_use]
pub fn encode(record: &DecisionRecord) -> Encoded {
    if record.side != Side::Response {
        return Encoded::Skipped(SkipReason::RequestSide);
    }
    let Some(org) = record.org else {
        return Encoded::Skipped(SkipReason::NoOrg);
    };
    let Some(principal) = &record.principal else {
        return Encoded::Skipped(SkipReason::NoPrincipal);
    };
    if record.tool.truncated {
        return Encoded::Skipped(SkipReason::Truncated);
    }
    if !is_safe_action_id(&record.tool.action_id) {
        return Encoded::Skipped(SkipReason::UnsafeActionId);
    }
    let ts = unix_seconds(record.decided_at);
    let p = format!("{}::\"{}\"", principal.entity_type, escape(&principal.entity_id));
    let o = format!("Pgs::Iam::Organization::\"{}\"", org.as_hyphenated());
    let (findings, declared, max_severity) = match &record.cedar {
        Some(c) => {
            // `context.findings` is a set of kinds: one kind of two detectors is listed once.
            let kinds: std::collections::BTreeSet<&str> = c.findings.iter().map(|f| f.kind.as_str()).collect();
            (
                kinds.iter().map(|k| format!("\"{}\"", escape(k))).collect::<Vec<_>>().join(", "),
                c.declared.as_str(),
                c.max_severity.map_or(NO_SEVERITY, Severity::as_long),
            )
        }
        None => (String::new(), "unknown", NO_SEVERITY),
    };
    let session = escape(&record.session.id);
    let text = format!(
        "@{ts} scope(principal: {p}, resource: {o}) \
         request_context(input: {{}}, findings: [{findings}], max_severity: {max_severity}, call: {{ declared: \"{declared}\", hosts: [] }}, \
         request: {{ scope: \"{scope}\", ingress: \"{ingress}\", model: \"{model}\" }}, \
         session: {{ id: \"{session}\", turn: {turn} }}) \
         Pgs::Gateway::Action::\"{action}\"::request(input: {{}}, callerPrincipal: {p}, callerResource: {o}, requestId: \"{rid}\", sessionId: \"{session}\")",
        scope = escape(&record.scope_prn),
        ingress = escape(&record.ingress),
        model = escape(&record.model),
        turn = record.session.turn,
        action = escape(&record.tool.action_id),
        rid = escape(record.request_id.as_deref().unwrap_or("")),
    );
    Encoded::Line(TraceLine { ts, text })
}
