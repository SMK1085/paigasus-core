// SPDX-License-Identifier: Apache-2.0

//! The pure part of `paigasus-gateway decisions export` (SMA-755 § 5.11): the arguments, the org check, the
//! time filter, the principal and key filters, the removal of duplicates, the order, the gap detection, the
//! summary text and the exit codes. It reads no stream and no clock. The NATS reader is
//! `adapters::decision_log::reader`.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use paigasus_gateway_core::decision_log::trace::{Encoded, SkipReason, encode, order_key};
use paigasus_kernel::tool_policy::ActionGroup;
use paigasus_proto::paigasus::gateway::v1 as pb;
use prost::Message;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::adapters::decision_log::proto::from_event;
use crate::domain::parse_org_uuid;

/// The default of `--max-lag`: 1 hour.
pub const DEFAULT_MAX_LAG: Duration = Duration::from_secs(3600);
/// The default of `--max-events`.
pub const DEFAULT_MAX_EVENTS: usize = 5_000_000;
/// The store delay that the summary counts, "10 min".
const LATE_AFTER: Duration = Duration::from_secs(600);
/// The most missing sequence numbers that the summary names for one instance.
const MAX_LISTED_GAPS: usize = 20;

/// The usage text.
pub const USAGE: &str = "usage: paigasus-gateway decisions export --org <uuid> --since <rfc3339> --until <rfc3339>\n    \
[--principal <prn>] [--api-key <id>] [--max-lag <n>s|m|h] [--max-events <n>]\n    \
[--out <file>] [--report <file>] [--nats-url-file <file>] [--creds <file>] [--ca <file>]\n\
The NATS URL comes from --nats-url-file or from PAIGASUS_NATS_URL. It is never an argument.";

/// The exit codes of the command (§ 5.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Done = 0,
    BadArgs = 1,
    Nats = 2,
    WrongOrg = 3,
    TooMany = 4,
}

impl Exit {
    /// The process exit code.
    #[must_use]
    pub fn code(self) -> u8 {
        self as u8
    }
}

/// The arguments of `decisions export`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportArgs {
    pub org: Uuid,
    pub since: SystemTime,
    pub until: SystemTime,
    pub principal: Option<String>,
    pub api_key: Option<String>,
    pub max_lag: Duration,
    pub max_events: usize,
    pub out: Option<PathBuf>,
    pub report: Option<PathBuf>,
    pub nats_url_file: Option<PathBuf>,
    pub creds: Option<PathBuf>,
    pub ca: Option<PathBuf>,
}

fn parse_time(flag: &str, value: &str) -> Result<SystemTime, String> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map(SystemTime::from)
        .map_err(|_| format!("{flag} must be an RFC 3339 time with an offset, for example 2026-11-01T00:00:00Z"))
}

fn parse_lag(value: &str) -> Result<Duration, String> {
    let bad = || "--max-lag must be <n>s, <n>m or <n>h".to_owned();
    let unit = value.chars().last().ok_or_else(bad)?;
    let scale: u64 = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        _ => return Err(bad()),
    };
    let n: u64 = value[..value.len() - 1].parse().map_err(|_| bad())?;
    n.checked_mul(scale).map(Duration::from_secs).ok_or_else(bad)
}

/// Read the arguments after `decisions export`. There is no `--nats-url`: a URL can hold credentials, and
/// argv is visible to every process of the host.
///
/// # Errors
///
/// A text for the operator when an argument is unknown, missing or bad. The exit code is 1.
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<ExportArgs, String> {
    let mut org = None;
    let mut since = None;
    let mut until = None;
    let mut principal = None;
    let mut api_key = None;
    let mut max_lag = DEFAULT_MAX_LAG;
    let mut max_events = DEFAULT_MAX_EVENTS;
    let (mut out, mut report, mut nats_url_file, mut creds, mut ca) = (None, None, None, None, None);
    let mut it = args.into_iter();
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("missing value for {flag}"));
        match flag.as_str() {
            "--org" => {
                let v = value()?;
                org = Some(parse_org_uuid(&v).ok_or_else(|| "--org must be a UUID in the form 8-4-4-4-12".to_owned())?);
            }
            "--since" => since = Some(parse_time("--since", &value()?)?),
            "--until" => until = Some(parse_time("--until", &value()?)?),
            "--principal" => principal = Some(value()?),
            "--api-key" => api_key = Some(value()?),
            "--max-lag" => max_lag = parse_lag(&value()?)?,
            "--max-events" => {
                max_events = value()?.parse().ok().filter(|n| *n > 0).ok_or_else(|| "--max-events must be a positive integer".to_owned())?;
            }
            "--out" => out = Some(PathBuf::from(value()?)),
            "--report" => report = Some(PathBuf::from(value()?)),
            "--nats-url-file" => nats_url_file = Some(PathBuf::from(value()?)),
            "--creds" => creds = Some(PathBuf::from(value()?)),
            "--ca" => ca = Some(PathBuf::from(value()?)),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let org = org.ok_or("--org is required")?;
    let since = since.ok_or("--since is required")?;
    let until = until.ok_or("--until is required")?;
    if since >= until {
        return Err("--since must be before --until".to_owned());
    }
    Ok(ExportArgs {
        org,
        since,
        until,
        principal,
        api_key,
        max_lag,
        max_events,
        out,
        report,
        nats_url_file,
        creds,
        ca,
    })
}

/// One message of the stream: its payload and the time that JetStream stored it.
#[derive(Debug, Clone)]
pub struct Exported {
    pub stored_at: SystemTime,
    pub bytes: Bytes,
}

/// Why the export stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    /// An event of another org. Exit 3 (A3).
    WrongOrg(String),
    /// More events in the range than `--max-events`. Exit 4.
    TooMany,
}

impl ExportError {
    /// The exit code of the error.
    #[must_use]
    pub fn exit(&self) -> Exit {
        match self {
            ExportError::WrongOrg(_) => Exit::WrongOrg,
            ExportError::TooMany => Exit::TooMany,
        }
    }
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportError::WrongOrg(what) => write!(f, "an event of another org is in the stream for this org: {what}"),
            ExportError::TooMany => f.write_str("more events in the range than --max-events: use a smaller range"),
        }
    }
}

/// The missing sequence numbers of one instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceGaps {
    pub instance: Uuid,
    pub missing: u64,
    /// The first missing numbers, at most `MAX_LISTED_GAPS`.
    pub listed: Vec<u64>,
}

/// The facts of one export. `Display` is the § 5.11 text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub org: Uuid,
    pub since: SystemTime,
    pub until: SystemTime,
    pub exported: usize,
    /// The skip counts, the largest first.
    pub skipped: Vec<(SkipReason, usize)>,
    pub duplicates: usize,
    pub late: usize,
    pub shared_second: usize,
    pub gaps: Vec<InstanceGaps>,
    pub unregistered: usize,
    /// Events that could not be decoded or mapped: a newer gateway or a forged entry. They are not in the trace.
    pub unmapped: usize,
}

/// The result of an export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    /// The trace lines, in order, with no newline.
    pub lines: Vec<String>,
    pub summary: Summary,
}

fn rfc3339(t: SystemTime) -> String {
    OffsetDateTime::from(t).format(&Rfc3339).unwrap_or_else(|_| "?".to_owned())
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "exported {} decisions for org {} from {} to {} (until exclusive)",
            self.exported,
            self.org,
            rfc3339(self.since),
            rfc3339(self.until)
        )?;
        let total: usize = self.skipped.iter().map(|(_, n)| n).sum();
        if total == 0 {
            writeln!(f, "skipped 0")?;
        } else {
            let parts: Vec<String> = self.skipped.iter().map(|(reason, n)| format!("{n} {}", reason.as_str())).collect();
            writeln!(f, "skipped {total}: {}", parts.join(", "))?;
        }
        if self.unmapped > 0 {
            writeln!(f, "{} events could not be mapped (a newer gateway or a forged entry); they are not in the trace", self.unmapped)?;
            writeln!(f, "WARNING: the trace misses {} events that this tool could not read.", self.unmapped)?;
        }
        writeln!(f, "removed {} duplicates; {} decisions arrived later than 10 min", self.duplicates, self.late)?;
        writeln!(f, "{} lines share a second with another line", self.shared_second)?;
        if self.gaps.is_empty() {
            writeln!(f, "sequence gaps: none (gaps inside the range only)")?;
        } else {
            let parts: Vec<String> = self
                .gaps
                .iter()
                .map(|g| {
                    let mut numbers: Vec<String> = g.listed.iter().map(u64::to_string).collect();
                    if usize::try_from(g.missing).map_or(true, |m| m > g.listed.len()) {
                        numbers.push("...".to_owned());
                    }
                    format!("instance {}: {} missing ({})", g.instance, g.missing, numbers.join(", "))
                })
                .collect();
            writeln!(f, "sequence gaps: {} (gaps inside the range only)", parts.join("; "))?;
        }
        writeln!(f, "WARNING: the trace holds no tool arguments and no tool results.")?;
        writeln!(f, "  A policy that reads context.input or context.input_text gets an evaluation error on replay.")?;
        writeln!(f, "  A policy that uses a `response` or `error` event in a temporal block never matches.")?;
        writeln!(f, "WARNING: Dogwood 1.2 checks each replayed call against its action schema. The trace has no tool")?;
        writeln!(f, "  arguments, so a tool with a required argument replays as DENY (\"nonconforming decision event\").")?;
        writeln!(f, "  Replay against a copy of your schema in which every context.input attribute is optional (`name?: Type`).")?;
        if self.unregistered > 0 {
            writeln!(f, "WARNING: {} lines are tools that the live gateway put under Unregistered. Replay takes the", self.unregistered)?;
            writeln!(f, "  action groups from your schema, so `action in Unregistered` can decide differently.")?;
        }
        writeln!(f, "WARNING: the trace is one history for the org. A policy with @scope(\"session\") or")?;
        writeln!(f, "  @scope(\"key\") replays against the wrong history unless you filter with --principal or")?;
        writeln!(f, "  --api-key. Session ids are empty until SMA-753.")
    }
}

/// The missing numbers of one sorted set: the count, and the first ones.
fn gaps_of(seqs: &BTreeSet<u64>) -> (u64, Vec<u64>) {
    let mut missing = 0u64;
    let mut listed = Vec::new();
    let mut prev: Option<u64> = None;
    for &s in seqs {
        if let Some(p) = prev {
            missing = missing.saturating_add(s - p - 1);
            let mut n = p + 1;
            while n < s && listed.len() < MAX_LISTED_GAPS {
                listed.push(n);
                n += 1;
            }
        }
        prev = Some(s);
    }
    (missing, listed)
}

/// Make the export from the messages of the stream, in stream order.
///
/// # Errors
///
/// [`ExportError`]: an event of another org, or too many events.
pub fn export(args: &ExportArgs, events: impl IntoIterator<Item = Exported>) -> Result<Export, ExportError> {
    let mut seen: HashSet<Uuid> = HashSet::new();
    let mut sequences: BTreeMap<Uuid, BTreeSet<u64>> = BTreeMap::new();
    let mut kept = Vec::new();
    let (mut duplicates, mut late, mut unmapped) = (0usize, 0usize, 0usize);
    for ev in events {
        // An event that cannot be decoded or mapped is skipped and counted: a newer gateway or a forged entry
        // must not stop the export of the other events. The org check reads the raw `org_id` first, so an event
        // of another org is exit 3 even when the rest of it cannot be mapped.
        let Ok(event) = pb::ToolDecisionEvent::decode(ev.bytes.as_ref()) else {
            unmapped += 1;
            continue;
        };
        if Uuid::try_parse(&event.org_id).is_ok_and(|o| o != args.org) {
            return Err(ExportError::WrongOrg(format!("decision {} has another org", event.decision_id)));
        }
        let Ok(record) = from_event(event) else {
            unmapped += 1;
            continue;
        };
        if record.org != Some(args.org) {
            return Err(ExportError::WrongOrg(format!("decision {} has org {:?}", record.decision_id, record.org)));
        }
        if record.decided_at < args.since || record.decided_at >= args.until {
            continue;
        }
        if !seen.insert(record.decision_id) {
            duplicates += 1;
            continue;
        }
        if seen.len() > args.max_events {
            return Err(ExportError::TooMany);
        }
        if ev.stored_at.duration_since(record.decided_at).is_ok_and(|d| d > LATE_AFTER) {
            late += 1;
        }
        sequences.entry(record.instance_id).or_default().insert(record.sequence);
        if args.principal.as_deref().is_some_and(|p| p != record.principal_prn) || args.api_key.as_deref().is_some_and(|k| record.api_key_id.as_deref() != Some(k)) {
            continue;
        }
        kept.push(record);
    }

    kept.sort_by_key(order_key);

    let mut lines = Vec::new();
    let mut seconds: Vec<i64> = Vec::new();
    let mut skips: BTreeMap<usize, (SkipReason, usize)> = BTreeMap::new();
    let mut unregistered = 0usize;
    for record in &kept {
        match encode(record) {
            Encoded::Line(line) => {
                if record.cedar.as_ref().is_some_and(|c| c.action_group == ActionGroup::Unregistered) {
                    unregistered += 1;
                }
                seconds.push(line.ts());
                lines.push(line.to_string());
            }
            Encoded::Skipped(reason) => {
                let rank = [SkipReason::RequestSide, SkipReason::NoOrg, SkipReason::NoPrincipal, SkipReason::Truncated, SkipReason::UnsafeActionId]
                    .iter()
                    .position(|r| *r == reason)
                    .unwrap_or(usize::MAX);
                skips.entry(rank).or_insert((reason, 0)).1 += 1;
            }
        }
    }
    // The lines are in order, so the lines of one second are next to each other.
    let mut shared_second = 0usize;
    let mut i = 0;
    while i < seconds.len() {
        let run = seconds[i..].iter().take_while(|s| **s == seconds[i]).count();
        if run > 1 {
            shared_second += run;
        }
        i += run;
    }
    let mut skipped: Vec<(SkipReason, usize)> = skips.into_values().collect();
    // A stable sort keeps the enum order for equal counts.
    skipped.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let gaps = sequences
        .iter()
        .filter_map(|(instance, seqs)| {
            let (missing, listed) = gaps_of(seqs);
            (missing > 0).then_some(InstanceGaps { instance: *instance, missing, listed })
        })
        .collect();
    Ok(Export {
        summary: Summary {
            org: args.org,
            since: args.since,
            until: args.until,
            exported: lines.len(),
            skipped,
            duplicates,
            late,
            shared_second,
            gaps,
            unregistered,
            unmapped,
        },
        lines,
    })
}

#[cfg(test)]
mod tests {
    use paigasus_gateway_core::decision_log::{CedarDetail, DecisionRecord, RecordBasis, RecordHeader, ReportedCall, Side, build_response};
    use paigasus_gateway_core::{Basis, BlockCode, DecisionContext, ReportDecision, ReportEntry};
    use paigasus_kernel::CedarUid;
    use paigasus_kernel::tool_policy::{Declared, Effect};

    use super::*;
    use crate::adapters::decision_log::proto::to_event;

    const ORG: &str = "0190a100-0000-7000-8000-0000000000a1";
    const OTHER_ORG: &str = "0190a100-0000-7000-8000-0000000000b2";
    const T0: u64 = 1_700_000_000;
    const RANGE: [&str; 4] = ["--since", "2023-11-14T22:13:20Z", "--until", "2023-11-14T22:13:30Z"];

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    // `T0` is 2023-11-14T22:13:20Z, so the range of `args_of` is [T0, T0 + 10).
    fn args_of(extra: &[&str]) -> ExportArgs {
        let mut v: Vec<String> = ["--org", ORG].iter().chain(RANGE.iter()).map(ToString::to_string).collect();
        v.extend(extra.iter().map(ToString::to_string));
        parse_args(v).expect("valid arguments")
    }

    fn record(org: &str, decided: u64, instance: u128, seq: u64, id: u128) -> DecisionRecord {
        let entry = ReportEntry {
            choice: 0,
            position: 0,
            name: "Get_Weather".into(),
            decision: ReportDecision::Block,
            basis: Some(Basis::Decider),
            code: Some(BlockCode::Policy),
            enforced: Some(true),
            reason: None,
        };
        let ctx = DecisionContext::new("prn:pgs:iam:::user/u", format!("prn:pgs:iam:::organization/{org}"), Some("req-1".into()));
        let header = RecordHeader::new(Uuid::from_u128(id), at(decided), Uuid::from_u128(instance), seq, Some(Uuid::try_parse(org).unwrap()));
        let mut r = build_response(header, &ctx, &ReportedCall::new(entry, None, None));
        r.principal = Some(CedarUid {
            entity_type: "Pgs::Iam::User".into(),
            entity_id: "u".into(),
        });
        r
    }

    fn exported(r: &DecisionRecord, stored_after: u64) -> Exported {
        Exported {
            stored_at: r.decided_at + Duration::from_secs(stored_after),
            bytes: Bytes::from(to_event(r).encode_to_vec()),
        }
    }

    fn plain(decided: u64, seq: u64, id: u128) -> Exported {
        exported(&record(ORG, decided, 1, seq, id), 1)
    }

    #[test]
    fn lines_are_in_order_and_until_is_exclusive() {
        let args = args_of(&[]);
        let events = vec![plain(T0 + 7, 3, 3), plain(T0 + 10, 4, 4), plain(T0 + 3, 2, 2), plain(T0 - 1, 1, 1), plain(T0, 5, 5)];
        let out = export(&args, events).expect("export");
        assert_eq!(out.lines.len(), 3, "since is inclusive, until is exclusive");
        assert!(out.lines[0].starts_with(&format!("@{T0} ")), "{}", out.lines[0]);
        assert!(out.lines[1].starts_with(&format!("@{} ", T0 + 3)));
        assert!(out.lines[2].starts_with(&format!("@{} ", T0 + 7)));
        assert!(out.lines.iter().all(|l| l.starts_with('@')));
    }

    #[test]
    fn an_event_of_another_org_is_exit_3() {
        let args = args_of(&[]);
        let foreign = exported(&record(OTHER_ORG, T0 + 1, 1, 1, 1), 1);
        let err = export(&args, vec![plain(T0 + 2, 2, 2), foreign]).unwrap_err();
        assert!(matches!(err, ExportError::WrongOrg(_)), "{err:?}");
        assert_eq!(err.exit(), Exit::WrongOrg);
        assert_eq!(err.exit().code(), 3);
        // An event outside the time range still stops the export: the org check comes first.
        let outside = exported(&record(OTHER_ORG, T0 + 500, 1, 1, 9), 1);
        assert!(matches!(export(&args, vec![outside]), Err(ExportError::WrongOrg(_))));
    }

    #[test]
    fn late_events_inside_max_lag_are_kept_and_counted() {
        let args = args_of(&[]);
        let late = exported(&record(ORG, T0 + 1, 1, 1, 1), 11 * 60);
        let on_time = exported(&record(ORG, T0 + 2, 1, 2, 2), 10 * 60);
        let out = export(&args, vec![late, on_time]).expect("export");
        assert_eq!(out.lines.len(), 2, "a late event is kept");
        assert_eq!(out.summary.late, 1, "exactly 10 min is not later than 10 min");
        assert!(out.summary.to_string().contains("1 decisions arrived later than 10 min"));
    }

    #[test]
    fn duplicates_are_removed_and_counted() {
        let args = args_of(&[]);
        let a = plain(T0 + 1, 1, 1);
        let out = export(&args, vec![a.clone(), plain(T0 + 2, 2, 2), a]).expect("export");
        assert_eq!(out.lines.len(), 2);
        assert_eq!(out.summary.duplicates, 1);
        assert!(out.summary.to_string().contains("removed 1 duplicates;"));
    }

    #[test]
    fn sequence_gaps_per_instance() {
        let args = args_of(&[]);
        let a = |seq, id| exported(&record(ORG, T0 + 1, 1, seq, id), 1);
        let b = |seq, id| exported(&record(ORG, T0 + 1, 2, seq, id), 1);
        let out = export(&args, vec![a(1, 1), a(2, 2), a(5, 5), b(1, 11), b(2, 12)]).expect("export");
        assert_eq!(out.summary.gaps.len(), 1, "only instance 1 has a gap");
        let text = out.summary.to_string();
        assert!(text.contains(&format!("sequence gaps: instance {}: 2 missing (3, 4)", Uuid::from_u128(1))), "{text}");
        assert!(text.contains("inside the range only"));
    }

    #[test]
    fn a_huge_gap_is_counted_without_a_loop_and_listed_in_part() {
        let args = args_of(&[]);
        let out = export(&args, vec![plain(T0 + 1, 1, 1), plain(T0 + 2, 1_000_000_000_000, 2)]).expect("export");
        assert_eq!(out.summary.gaps[0].missing, 999_999_999_998);
        assert_eq!(out.summary.gaps[0].listed.len(), MAX_LISTED_GAPS);
        assert!(out.summary.to_string().contains(", ...)"));
    }

    #[test]
    fn more_than_max_events_is_exit_4() {
        let args = args_of(&["--max-events", "2"]);
        assert!(export(&args, vec![plain(T0 + 1, 1, 1), plain(T0 + 2, 2, 2)]).is_ok(), "exactly the limit is allowed");
        let err = export(&args, vec![plain(T0 + 1, 1, 1), plain(T0 + 2, 2, 2), plain(T0 + 3, 3, 3)]).unwrap_err();
        assert_eq!(err, ExportError::TooMany);
        assert_eq!(err.exit().code(), 4);
    }

    #[test]
    fn the_summary_has_every_warning() {
        let args = args_of(&[]);
        let mut unregistered = record(ORG, T0 + 1, 1, 1, 1);
        unregistered.cedar = Some(CedarDetail::new(
            Effect::Deny,
            vec![],
            vec![],
            false,
            BTreeSet::new(),
            Declared::Unknown,
            ActionGroup::Unregistered,
            Duration::ZERO,
        ));
        unregistered.basis = RecordBasis::Policy;
        let shared = record(ORG, T0 + 1, 1, 2, 2);
        let later = record(ORG, T0 + 4, 1, 3, 3);
        let mut request_a = record(ORG, T0 + 2, 1, 4, 4);
        request_a.side = Side::Request;
        let mut request_b = record(ORG, T0 + 2, 1, 5, 5);
        request_b.side = Side::Request;
        let mut unsafe_id = record(ORG, T0 + 3, 1, 6, 6);
        unsafe_id.tool.action_id = "a(b)".into();
        let mut truncated = record(ORG, T0 + 3, 1, 7, 7);
        truncated.tool.truncated = true;
        let events: Vec<Exported> = [&unregistered, &shared, &later, &request_a, &request_b, &unsafe_id, &truncated]
            .into_iter()
            .map(|r| exported(r, 1))
            .collect();
        let out = export(&args, events).expect("export");
        let expected = format!(
            "exported 3 decisions for org {ORG} from 2023-11-14T22:13:20Z to 2023-11-14T22:13:30Z (until exclusive)\n\
skipped 4: 2 request side, 1 truncated, 1 unsafe action id\n\
removed 0 duplicates; 0 decisions arrived later than 10 min\n\
2 lines share a second with another line\n\
sequence gaps: none (gaps inside the range only)\n\
WARNING: the trace holds no tool arguments and no tool results.\n  \
A policy that reads context.input or context.input_text gets an evaluation error on replay.\n  \
A policy that uses a `response` or `error` event in a temporal block never matches.\n\
WARNING: Dogwood 1.2 checks each replayed call against its action schema. The trace has no tool\n  \
arguments, so a tool with a required argument replays as DENY (\"nonconforming decision event\").\n  \
Replay against a copy of your schema in which every context.input attribute is optional (`name?: Type`).\n\
WARNING: 1 lines are tools that the live gateway put under Unregistered. Replay takes the\n  \
action groups from your schema, so `action in Unregistered` can decide differently.\n\
WARNING: the trace is one history for the org. A policy with @scope(\"session\") or\n  \
@scope(\"key\") replays against the wrong history unless you filter with --principal or\n  \
--api-key. Session ids are empty until SMA-753.\n"
        );
        assert_eq!(out.summary.to_string(), expected);
    }

    #[test]
    fn principal_and_api_key_filters_select_one_history() {
        let mk = |principal: &str, key: Option<&str>, seq: u64, id: u128| {
            let mut r = record(ORG, T0 + seq, 1, seq, id);
            r.principal_prn = principal.to_owned();
            r.api_key_id = key.map(ToOwned::to_owned);
            exported(&r, 1)
        };
        let all = || vec![mk("prn:a", Some("k1"), 1, 1), mk("prn:b", Some("k2"), 2, 2), mk("prn:a", Some("k2"), 3, 3), mk("prn:b", None, 4, 4)];
        assert_eq!(export(&args_of(&[]), all()).unwrap().lines.len(), 4);
        assert_eq!(export(&args_of(&["--principal", "prn:a"]), all()).unwrap().lines.len(), 2);
        assert_eq!(export(&args_of(&["--api-key", "k2"]), all()).unwrap().lines.len(), 2);
        let both = export(&args_of(&["--principal", "prn:a", "--api-key", "k2"]), all()).unwrap();
        assert_eq!(both.lines.len(), 1);
        assert!(both.lines[0].starts_with(&format!("@{} ", T0 + 3)));
        // A filter does not make a sequence gap: the gaps are those of the whole org in the range.
        assert!(both.summary.gaps.is_empty(), "sequences 1 to 4 are all seen, so the filter makes no gap");
    }

    #[test]
    fn the_export_ignores_an_unknown_field() {
        let mut ev = plain(T0 + 1, 1, 1);
        let mut raw = ev.bytes.to_vec();
        // Field 999, varint 1: the tag (999 << 3) is 7992, as a varint 0xB8 0x3E.
        raw.extend_from_slice(&[0xB8, 0x3E, 0x01]);
        ev.bytes = Bytes::from(raw);
        let out = export(&args_of(&[]), vec![ev]).expect("an unknown field is not an error");
        assert_eq!(out.lines.len(), 1);
    }

    #[test]
    fn a_garbage_event_is_skipped_and_counted() {
        let bad = Exported {
            stored_at: at(T0),
            bytes: Bytes::from_static(b"\xff\xff\xff"),
        };
        let out = export(&args_of(&[]), vec![bad, plain(T0 + 1, 1, 1)]).expect("a bad event does not stop the export");
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.summary.unmapped, 1);
    }

    /// An event with an enum value that this tool does not know.
    fn unknown_enum(org: &str, id: u128) -> Exported {
        let mut event = to_event(&record(org, T0 + 2, 1, 9, id));
        event.side = 99;
        Exported {
            stored_at: at(T0 + 3),
            bytes: Bytes::from(event.encode_to_vec()),
        }
    }

    #[test]
    fn an_unmappable_event_is_skipped_and_counted_with_a_warning() {
        let out = export(&args_of(&[]), vec![plain(T0 + 1, 1, 1), unknown_enum(ORG, 2), plain(T0 + 3, 3, 3)]).expect("export");
        assert_eq!(out.lines.len(), 2, "the other events are exported");
        assert_eq!(out.summary.unmapped, 1);
        let text = out.summary.to_string();
        assert!(text.contains("1 events could not be mapped (a newer gateway or a forged entry); they are not in the trace\n"), "{text}");
        assert!(text.contains("WARNING: the trace misses 1 events"), "{text}");
        let none = export(&args_of(&[]), vec![plain(T0 + 1, 1, 1)]).expect("export");
        assert!(!none.summary.to_string().contains("could not be mapped"));
    }

    #[test]
    fn an_unmappable_event_of_another_org_is_still_exit_3() {
        let err = export(&args_of(&[]), vec![unknown_enum(OTHER_ORG, 2)]).unwrap_err();
        assert_eq!(err.exit(), Exit::WrongOrg);
    }

    fn base(extra: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = ["--org", ORG].iter().map(ToString::to_string).collect();
        v.extend(extra.iter().map(ToString::to_string));
        v
    }

    #[test]
    fn since_not_before_until_is_exit_1() {
        let same = base(&["--since", "2026-01-01T00:00:00Z", "--until", "2026-01-01T00:00:00Z"]);
        assert!(parse_args(same).unwrap_err().contains("before"));
        let reversed = base(&["--since", "2026-01-02T00:00:00Z", "--until", "2026-01-01T00:00:00Z"]);
        assert!(parse_args(reversed).is_err());
        // The same instant in two offsets is equal, so it is refused too.
        let offsets = base(&["--since", "2026-01-01T02:00:00+02:00", "--until", "2026-01-01T00:00:00Z"]);
        assert!(parse_args(offsets).is_err());
        assert_eq!(Exit::BadArgs.code(), 1);
    }

    #[test]
    fn a_time_without_an_offset_is_exit_1() {
        let v = base(&["--since", "2026-01-01T00:00:00", "--until", "2026-01-02T00:00:00Z"]);
        let err = parse_args(v).unwrap_err();
        assert!(err.contains("--since") && err.contains("offset"), "{err}");
        let ok = parse_args(base(&["--since", "2026-01-01T00:00:00+02:00", "--until", "2026-01-02T00:00:00Z"])).unwrap();
        // 2026-01-01T00:00:00Z is 1767225600, and +02:00 is two hours earlier.
        assert_eq!(ok.since, at(1_767_225_600 - 7200));
    }

    #[test]
    fn an_upper_case_org_is_canonical() {
        let v: Vec<String> = ["--org", "0190A100-0000-7000-8000-0000000000A1", "--since", "2026-01-01T00:00:00Z", "--until", "2026-01-02T00:00:00Z"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let a = parse_args(v).unwrap();
        assert_eq!(a.org.to_string(), ORG);
        assert_eq!(a.max_lag, Duration::from_secs(3600));
        assert_eq!(a.max_events, 5_000_000);
        // A brace form is refused by `parse_org_uuid`.
        let braces: Vec<String> = ["--org", "{0190a100-0000-7000-8000-0000000000a1}", "--since", "2026-01-01T00:00:00Z", "--until", "2026-01-02T00:00:00Z"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(parse_args(braces).is_err());
    }

    #[test]
    fn the_url_is_never_read_from_argv() {
        let v = base(&["--since", "2026-01-01T00:00:00Z", "--until", "2026-01-02T00:00:00Z", "--nats-url", "nats://user:pw@host:4222"]);
        let err = parse_args(v).unwrap_err();
        assert!(err.contains("unknown argument"), "{err}");
        assert!(!err.contains("pw"), "the value must not be echoed: {err}");
    }

    #[test]
    fn max_lag_units_and_other_flags() {
        let range = ["--since", "2026-01-01T00:00:00Z", "--until", "2026-01-02T00:00:00Z"];
        let mut flags: Vec<&str> = range.to_vec();
        flags.extend(["--max-lag", "90m", "--out", "t.log", "--report", "r.txt", "--creds", "c", "--ca", "ca", "--nats-url-file", "u"]);
        let a = parse_args(base(&flags)).unwrap();
        assert_eq!(a.max_lag, Duration::from_secs(5400));
        assert_eq!(a.out.as_deref(), Some(std::path::Path::new("t.log")));
        assert_eq!(a.report.as_deref(), Some(std::path::Path::new("r.txt")));
        assert!(a.creds.is_some() && a.ca.is_some() && a.nats_url_file.is_some());
        for bad in ["5", "5d", "m", "-1h", ""] {
            let mut v: Vec<&str> = range.to_vec();
            v.extend(["--max-lag", bad]);
            assert!(parse_args(base(&v)).is_err(), "{bad:?}");
        }
        assert!(parse_args(base(&["--since", "2026-01-01T00:00:00Z"])).is_err(), "--until is required");
        assert!(parse_args(vec!["--since".to_owned()]).unwrap_err().contains("missing value"));
    }
}
