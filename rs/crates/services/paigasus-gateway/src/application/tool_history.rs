// SPDX-License-Identifier: Apache-2.0

//! SMA-753: the `ToolHistory` service. It wraps the `SessionStore` port with the store budget of one
//! request (spec D10), the error counting and the history metrics (D13).
//!
//! The deadline is one cumulative store-time budget per request. Each store call on the latency path
//! (`lookup`, `write`, `reserve`) runs under `timeout(budget.remaining())` and charges its elapsed
//! time. After the first store error, every later store call of the request is skipped. In PR 1 a
//! store error has no policy effect: it is counted and the request continues. No client id (call id,
//! result id, `ref`) becomes a metric label or a log field.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use metrics::counter;
use paigasus_gateway_core::decision_log::ReportedCall;
use paigasus_gateway_core::{DecisionContext, LatestTurn, ReportDecision, SessionContext, StoreBudget, ToolCall, ToolTurn};
use paigasus_kernel::mint_uuid7;
use paigasus_kernel::tool_policy::{FORGED_HISTORY, Findings, HISTORY_TRUNCATED, MIXED_SESSIONS, normalise};
use paigasus_observability::names;
use tokio::time::Instant;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

use crate::domain::decision_log::Entropy;
use crate::domain::limits::{Clock, UnavailableKind};
use crate::domain::tool_history::{
    AppendReply, AppendRequest, CallEvent, EventVerdict, ForwardedCall, Histories, HistoryStoreError, LookupReply, LookupRequest, ReserveRequest, ResultEvent, SessionStore, SettleItem, SettleRequest,
    WriteRequest, history_tool, link_session,
};

const OPS: [&str; 5] = ["lookup", "write", "reserve", "settle", "append"];
const LISTS: [&str; 3] = ["calls", "results", "refs"];
const REASONS: [&str; 2] = ["retention", "cap"];

/// What `ToolHistory::link` gives the request side.
pub struct LinkedRequest {
    pub session: Arc<SessionContext>,
    /// The write that runs after the admission. `None` when there is nothing to write.
    pub pending: Option<PendingWrite>,
}

/// The write that runs after the admission (spec D4).
#[derive(Debug)]
pub struct PendingWrite {
    session: Arc<SessionContext>,
    request: WriteRequest,
}

/// The store calls that a report needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlePlan {
    pub settle: Vec<SettleItem>,
    pub append: Vec<ForwardedCall>,
}

/// Split the report entries into the settles and the appends. Pure.
///
/// An entry with a decision id that `reserved` holds is a settle: `Allow` when the entry is allowed,
/// else `Deny`. Only an `Allow` carries the call id and the item id.
///
/// An entry with a decision id that `reserved` does not hold has no event to settle: the reserve
/// failed, or the budget skipped it. An allowed one with call facts is an append under its decision
/// id, so its result links on the next request. A denied one gives nothing: it was not forwarded,
/// so it needs no link, and it is not counted (final review, finding 1).
///
/// An allowed entry with call facts and no decision id was forwarded with no decision (spec D8): an
/// append under a new event id. Everything else gives nothing.
pub fn settle_plan(reported: &[ReportedCall], reserved: &dyn Fn(Uuid) -> bool, mint: &mut dyn FnMut() -> Uuid) -> SettlePlan {
    let mut plan = SettlePlan {
        settle: Vec::new(),
        append: Vec::new(),
    };
    for r in reported {
        let allow = matches!(r.entry.decision, ReportDecision::Allow);
        // `normalise(&r.entry.name)` equals `history_tool(call)` only because `report.rs` sets `name`
        // from `ToolCall::display_name()`. Keep the two in step.
        match (r.decision_id, &r.call) {
            (Some(event_id), call) if reserved(event_id) => {
                let (call_id, item_id) = match call {
                    Some(c) if allow => (c.id.clone(), c.item_id.clone()),
                    _ => (None, None),
                };
                plan.settle.push(SettleItem {
                    event_id,
                    verdict: if allow { EventVerdict::Allow } else { EventVerdict::Deny },
                    tool: normalise(&r.entry.name),
                    call_id,
                    item_id,
                });
            }
            (Some(event_id), Some(c)) if allow => plan.append.push(ForwardedCall {
                id: event_id,
                tool: normalise(&r.entry.name),
                call_id: c.id.clone(),
                item_id: c.item_id.clone(),
            }),
            (None, Some(c)) if allow => plan.append.push(ForwardedCall {
                id: mint(),
                tool: normalise(&r.entry.name),
                call_id: c.id.clone(),
                item_id: c.item_id.clone(),
            }),
            _ => {}
        }
    }
    plan
}

/// Prime every series of the three history metrics at 0.
pub fn prime_tool_history_metrics() {
    for op in OPS {
        for kind in UnavailableKind::ALL.map(UnavailableKind::as_label).into_iter().chain(["deadline"]) {
            counter!(names::GATEWAY_TOOL_HISTORY_STORE_ERRORS_TOTAL, "op" => op, "kind" => kind).absolute(0);
        }
    }
    for list in LISTS {
        counter!(names::GATEWAY_TOOL_HISTORY_IDS_CAPPED_TOTAL, "list" => list).absolute(0);
    }
    for reason in REASONS {
        counter!(names::GATEWAY_TOOL_HISTORY_EVENTS_TRIMMED_TOTAL, "reason" => reason).absolute(0);
    }
}

fn count_error(op: &'static str, kind: &'static str) {
    tracing::warn!(op, kind, "tool history store call failed");
    counter!(names::GATEWAY_TOOL_HISTORY_STORE_ERRORS_TOTAL, "op" => op, "kind" => kind).increment(1);
}

fn count_trimmed(reply: &AppendReply) {
    if reply.trimmed_retention > 0 {
        counter!(names::GATEWAY_TOOL_HISTORY_EVENTS_TRIMMED_TOTAL, "reason" => "retention").increment(reply.trimmed_retention);
    }
    if reply.trimmed_cap > 0 {
        counter!(names::GATEWAY_TOOL_HISTORY_EVENTS_TRIMMED_TOTAL, "reason" => "cap").increment(reply.trimmed_cap);
    }
}

fn error_label(e: &HistoryStoreError) -> &'static str {
    match e {
        HistoryStoreError::Unavailable { kind, .. } => kind.as_label(),
    }
}

/// The tool history of the gateway: the port, the clock, and the budget rules around it.
#[derive(Clone)]
pub struct ToolHistory {
    store: Arc<dyn SessionStore>,
    clock: Arc<dyn Clock>,
    entropy: Arc<dyn Entropy>,
    deadline: Duration,
    tasks: TaskTracker,
}

impl ToolHistory {
    #[must_use]
    pub fn new(store: Arc<dyn SessionStore>, clock: Arc<dyn Clock>, entropy: Arc<dyn Entropy>, deadline: Duration) -> Self {
        Self {
            store,
            clock,
            entropy,
            deadline,
            tasks: TaskTracker::new(),
        }
    }

    /// The settle and append tasks, for the shutdown drain.
    #[must_use]
    pub fn tasks(&self) -> TaskTracker {
        self.tasks.clone()
    }

    fn now_ms(&self) -> u64 {
        self.clock.now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }

    /// A fresh UUIDv7 from the clock and the entropy, as the decision recorder mints its ids.
    fn mint(&self) -> Uuid {
        mint_uuid7(self.now_ms(), self.entropy.bytes10())
    }

    /// One store call on the latency path. `None` when no time is left (no call is made), on an
    /// error and on a timeout. An error or a timeout fails the budget and is counted. A call that
    /// a spent budget skips, with no earlier failure, counts as a `deadline` for its op and fails
    /// the budget, so only the first skip counts.
    async fn timed<T>(&self, budget: &StoreBudget, op: &'static str, call: impl Future<Output = Result<T, HistoryStoreError>>) -> Option<T> {
        let Some(remaining) = budget.remaining() else {
            if !budget.is_failed() {
                budget.fail();
                count_error(op, "deadline");
            }
            return None;
        };
        let start = Instant::now();
        let out = tokio::time::timeout(remaining, call).await;
        budget.charge(start.elapsed());
        self.finish(budget, op, out)
    }

    /// One store call off the latency path (ruling T10-a). It always runs, under its own deadline,
    /// whatever the budget says. It does not charge the budget and an error does not fail it. The
    /// error is counted.
    async fn off_path<T>(&self, op: &'static str, call: impl Future<Output = Result<T, HistoryStoreError>>) -> Option<T> {
        match tokio::time::timeout(self.deadline, call).await {
            Ok(Ok(v)) => Some(v),
            Ok(Err(e)) => {
                count_error(op, error_label(&e));
                None
            }
            Err(_) => {
                count_error(op, "deadline");
                None
            }
        }
    }

    fn finish<T>(&self, budget: &StoreBudget, op: &'static str, out: Result<Result<T, HistoryStoreError>, tokio::time::error::Elapsed>) -> Option<T> {
        match out {
            Ok(Ok(v)) => Some(v),
            Ok(Err(e)) => {
                budget.fail();
                count_error(op, error_label(&e));
                None
            }
            Err(_) => {
                budget.fail();
                count_error(op, "deadline");
                None
            }
        }
    }

    /// Read only. `None` after a lookup error (counted, op `lookup`): the request has no session.
    pub async fn link(&self, principal: &str, org: Uuid, api_key_id: Option<&str>, turn: &ToolTurn) -> Option<LinkedRequest> {
        let budget = StoreBudget::new(self.deadline);
        let none = LatestTurn::default();
        let latest = match turn {
            ToolTurn::Parsed(f) => &f.latest,
            _ => &none,
        };
        for (list, n) in LISTS.into_iter().zip([latest.dropped_calls, latest.dropped_results, latest.dropped_refs]) {
            if n > 0 {
                counter!(names::GATEWAY_TOOL_HISTORY_IDS_CAPPED_TOTAL, "list" => list).increment(n as u64);
            }
        }
        let req = LookupRequest {
            org,
            principal: principal.to_owned(),
            call_ids: latest.call_ids.clone(),
            result_call_ids: latest.result_call_ids.clone(),
            refs: latest.refs.clone(),
            now_ms: self.now_ms(),
        };
        let new_session = self.mint();
        let link = self
            .timed(&budget, "lookup", async {
                let reply: LookupReply = self.store.lookup(&req).await?;
                link_session(&req, &reply, new_session)
            })
            .await?;

        let mut findings = Findings::new();
        if link.mixed_sessions {
            findings.insert(MIXED_SESSIONS);
            counter!(names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL, "side" => "request", "kind" => MIXED_SESSIONS).increment(1);
        }
        if link.forged_results > 0 {
            findings.insert(FORGED_HISTORY);
            counter!(names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL, "side" => "request", "kind" => FORGED_HISTORY).increment(link.forged_results as u64);
        }

        let turn_no = link.turn;
        let results: Vec<ResultEvent> = link
            .results
            .into_iter()
            .map(|r| ResultEvent {
                id: self.mint(),
                call_id: r.call_id,
                tool: r.tool,
                event_id: r.event_id,
            })
            .collect();
        let touch = link.joined.then_some(turn_no);
        let histories = Histories {
            org,
            principal: principal.to_owned(),
            key: api_key_id.map(str::to_owned),
            session: link.session,
        };
        let session = Arc::new(SessionContext::new(org, link.session, turn_no, findings, budget));
        let pending = (touch.is_some() || !results.is_empty()).then(|| PendingWrite {
            session: Arc::clone(&session),
            request: WriteRequest {
                histories,
                turn: touch,
                results,
                now_ms: 0,
            },
        });
        Some(LinkedRequest { session, pending })
    }

    /// After the admission. A store error is counted (op `write`); the request continues.
    pub async fn commit(&self, pending: PendingWrite) {
        let PendingWrite { session, mut request } = pending;
        request.now_ms = self.now_ms();
        if let Some(reply) = self.timed(&session.budget, "write", self.store.write(&request)).await {
            count_trimmed(&reply);
        }
    }

    /// Inside the decide future. Returns the findings for the hold: `history_truncated` when the
    /// reply flags a set. A store error is counted (op `reserve`) and gives no finding.
    pub async fn reserve(&self, ctx: &DecisionContext, session: &SessionContext, calls: &[ToolCall], ids: &[Uuid]) -> Findings {
        let mut findings = Findings::new();
        if calls.is_empty() {
            return findings;
        }
        debug_assert_eq!(calls.len(), ids.len(), "one id per call");
        let req = ReserveRequest {
            histories: Histories {
                org: session.org,
                principal: ctx.principal.clone(),
                key: ctx.api_key_id.clone(),
                session: session.session_id,
            },
            turn: session.info.turn,
            calls: calls.iter().zip(ids).map(|(c, id)| CallEvent { id: *id, tool: history_tool(c) }).collect(),
            now_ms: self.now_ms(),
            // PR 1 reads no window (spec D15).
            window_from_ms: None,
        };
        if let Some(reply) = self.timed(&session.budget, "reserve", self.store.reserve(&req)).await {
            // Only a reserve that returned wrote its events. The report site appends the others.
            session.mark_reserved(ids);
            count_trimmed(&reply);
            if !reply.truncated.is_empty() {
                findings.insert(HISTORY_TRUNCATED);
            }
        }
        findings
    }

    /// At the report site. Spawns one task on the tracker for the settle and, when needed, the append.
    pub fn settle(&self, ctx: &DecisionContext, reported: &[ReportedCall]) {
        let Some(session) = ctx.session.clone() else { return };
        // Settle and append are off the latency path: a failed budget does not skip them (T10-a, T10-b).
        if self.tasks.is_closed() {
            return;
        }
        let plan = settle_plan(reported, &|id| session.is_reserved(id), &mut || self.mint());
        if plan.settle.is_empty() && plan.append.is_empty() {
            return;
        }
        let this = self.clone();
        let principal = ctx.principal.clone();
        let key = ctx.api_key_id.clone();
        self.tasks.spawn(async move { this.run_settle(session, principal, key, plan).await });
    }

    async fn run_settle(&self, session: Arc<SessionContext>, principal: String, key: Option<String>, plan: SettlePlan) {
        let now_ms = self.now_ms();
        if !plan.settle.is_empty() {
            let req = SettleRequest {
                org: session.org,
                principal: principal.clone(),
                session: session.session_id,
                items: plan.settle,
                now_ms,
            };
            // An error is counted. It does not stop the append: the two are independent (spec D8).
            self.off_path("settle", self.store.settle(&req)).await;
        }
        if !plan.append.is_empty() {
            let req = AppendRequest {
                histories: Histories {
                    org: session.org,
                    principal,
                    key,
                    session: session.session_id,
                },
                turn: session.info.turn,
                calls: plan.append,
                now_ms,
            };
            if let Some(reply) = self.off_path("append", self.store.append(&req)).await {
                count_trimmed(&reply);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use metrics_util::debugging::{DebuggingRecorder, Snapshotter};
    use paigasus_gateway_core::decision_log::ReportedCall;
    use paigasus_gateway_core::{Basis, BlockCode, DecidedCall, Decision, DecisionContext, Format, ReportDecision, ReportEntry, ToolTurn, TurnFacts, parse_body};
    use paigasus_kernel::tool_policy::Findings;
    use uuid::Uuid;

    use super::*;
    use crate::adapters::history::memory::MemorySessionStore;
    use crate::domain::tool_history::{EventKind, EventVerdict, Scope};
    use crate::test_support::{FixedClock, ORG_A, ORG_A_PRN, at, counter};

    const PRINCIPAL: &str = "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004";
    const KEY: &str = "key-1";

    /// Distinct ids from a counter, so two ids minted at the same fixed time differ.
    struct CountingEntropy(AtomicU64);

    impl Entropy for CountingEntropy {
        fn bytes10(&self) -> [u8; 10] {
            let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
            let mut b = [0u8; 10];
            b[2..].copy_from_slice(&n.to_be_bytes());
            b
        }
    }

    #[derive(Clone, Copy)]
    enum Mode {
        Pass,
        Fail,
        /// Only `settle` fails.
        FailSettle,
        /// Only `reserve` fails.
        FailReserve,
        Sleep(Duration),
    }

    /// A store that wraps the memory store, counts every call and can fail, sleep or flag a truncation.
    struct Fake {
        inner: MemorySessionStore,
        mode: Mode,
        calls: AtomicUsize,
        reserve_reply: Mutex<VecDeque<AppendReply>>,
    }

    impl Fake {
        fn new(mode: Mode) -> Arc<Self> {
            Arc::new(Fake {
                inner: MemorySessionStore::new(),
                mode,
                calls: AtomicUsize::new(0),
                reserve_reply: Mutex::new(VecDeque::new()),
            })
        }

        fn count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        async fn gate(&self) -> Result<(), HistoryStoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.mode {
                Mode::Pass | Mode::FailSettle | Mode::FailReserve => Ok(()),
                Mode::Fail => Err(HistoryStoreError::Unavailable {
                    kind: UnavailableKind::Io,
                    detail: "down".into(),
                }),
                Mode::Sleep(d) => {
                    tokio::time::sleep(d).await;
                    Ok(())
                }
            }
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for Fake {
        async fn lookup(&self, req: &LookupRequest) -> Result<LookupReply, HistoryStoreError> {
            self.gate().await?;
            self.inner.lookup(req).await
        }
        async fn write(&self, req: &WriteRequest) -> Result<AppendReply, HistoryStoreError> {
            self.gate().await?;
            self.inner.write(req).await
        }
        async fn reserve(&self, req: &ReserveRequest) -> Result<AppendReply, HistoryStoreError> {
            self.gate().await?;
            if matches!(self.mode, Mode::FailReserve) {
                return Err(HistoryStoreError::Unavailable {
                    kind: UnavailableKind::Io,
                    detail: "down".into(),
                });
            }
            if let Some(r) = self.reserve_reply.lock().unwrap().pop_front() {
                return Ok(r);
            }
            self.inner.reserve(req).await
        }
        async fn settle(&self, req: &SettleRequest) -> Result<(), HistoryStoreError> {
            self.gate().await?;
            if matches!(self.mode, Mode::FailSettle) {
                return Err(HistoryStoreError::Unavailable {
                    kind: UnavailableKind::Io,
                    detail: "down".into(),
                });
            }
            self.inner.settle(req).await
        }
        async fn append(&self, req: &AppendRequest) -> Result<AppendReply, HistoryStoreError> {
            self.gate().await?;
            self.inner.append(req).await
        }
    }

    fn history(store: &Arc<Fake>, deadline: Duration) -> ToolHistory {
        let store: Arc<dyn SessionStore> = store.clone();
        ToolHistory::new(store, Arc::new(FixedClock(at("2026-11-22T10:00:00Z"))), Arc::new(CountingEntropy(AtomicU64::new(0))), deadline)
    }

    fn setup() -> (Arc<Fake>, ToolHistory) {
        let store = Fake::new(Mode::Pass);
        let h = history(&store, Duration::from_secs(1));
        (store, h)
    }

    fn metrics() -> (DebuggingRecorder, Snapshotter) {
        let r = DebuggingRecorder::new();
        let s = r.snapshotter();
        (r, s)
    }

    fn errors(s: &Snapshotter, op: &str, kind: &str) -> Option<u64> {
        counter(s, names::GATEWAY_TOOL_HISTORY_STORE_ERRORS_TOTAL, &[("op", op), ("kind", kind)])
    }

    fn finding(s: &Snapshotter, kind: &str) -> Option<u64> {
        counter(s, names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL, &[("side", "request"), ("kind", kind)])
    }

    fn turn(calls: &[&str], results: &[&str]) -> ToolTurn {
        let mut f = TurnFacts::default();
        f.latest.call_ids = calls.iter().map(|s| (*s).to_owned()).collect();
        f.latest.result_call_ids = results.iter().map(|s| (*s).to_owned()).collect();
        ToolTurn::Parsed(f)
    }

    fn ctx() -> DecisionContext {
        DecisionContext::new(PRINCIPAL, ORG_A_PRN, None).with_request(Some(KEY.into()), "chat", "m")
    }

    async fn link(h: &ToolHistory, t: &ToolTurn) -> Option<LinkedRequest> {
        h.link(PRINCIPAL, ORG_A, Some(KEY), t).await
    }

    fn chat_body(items: &[(&str, &str)]) -> paigasus_gateway_core::BodyCalls {
        let tool_calls: Vec<_> = items
            .iter()
            .map(|(id, name)| serde_json::json!({"id": id, "type": "function", "function": {"name": name, "arguments": "{}"}}))
            .collect();
        let body = serde_json::json!({"object": "chat.completion", "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": tool_calls}, "finish_reason": "tool_calls"}]});
        parse_body(Format::OpenAiChat, body.to_string().as_bytes(), 1 << 20).expect("body")
    }

    /// Calls from a chat body: `(call id, tool name)` pairs.
    fn calls(items: &[(&str, &str)]) -> Vec<ToolCall> {
        chat_body(items).calls
    }

    /// The report entries of `items`, with one `(decision, decision id)` per call.
    fn reported_of(items: &[(&str, &str)], decisions: &[(Decision, Option<Uuid>)]) -> Vec<ReportedCall> {
        let decided: Vec<DecidedCall> = decisions
            .iter()
            .map(|(d, id)| {
                let c = DecidedCall::new(*d, None);
                match id {
                    Some(id) => c.with_decision_id(*id),
                    None => c,
                }
            })
            .collect();
        chat_body(items).reported(&decided, Basis::Decider)
    }

    fn entry(name: &str, decision: ReportDecision) -> ReportEntry {
        ReportEntry {
            choice: 0,
            position: 0,
            name: name.into(),
            decision,
            basis: Some(Basis::Decider),
            code: None,
            enforced: None,
            reason: None,
        }
    }

    fn counting_mint() -> impl FnMut() -> Uuid {
        let mut n = 100u128;
        move || {
            n += 1;
            Uuid::from_u128(n)
        }
    }

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    const BLOCK: Decision = Decision::Block { code: BlockCode::Policy };

    #[tokio::test]
    async fn a_first_request_gets_a_new_session_and_writes_nothing() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        let (store, h) = setup();
        let linked = link(&h, &turn(&["c-9"], &[])).await.expect("a session");
        assert_eq!(linked.session.info.turn, 1);
        assert_eq!(linked.session.session_id.get_version_num(), 7);
        assert!(linked.pending.is_none());
        assert_eq!(linked.session.findings, Findings::new());
        assert_eq!(store.count(), 1, "one lookup");
        assert_eq!(finding(&snap, "mixed_sessions").unwrap_or(0), 0);
    }

    #[tokio::test]
    async fn a_follow_up_with_a_linked_result_joins_and_writes_one_result_event() {
        let (store, h) = setup();
        // Request 1: a session, a reserved call with a mixed-case tool name, then a settle that links it.
        let first = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let items = [("c-1", "Read_File")];
        let decision_id = u(0xd1);
        h.reserve(&ctx(), &first, &calls(&items), &[decision_id]).await;
        let reported = reported_of(&items, &[(Decision::Allow, Some(decision_id))]);
        h.settle(&ctx().with_session(first.clone()), &reported);
        h.tasks().close();
        h.tasks().wait().await;

        // Request 2 sends the result of `c-1`.
        let second = link(&h, &turn(&[], &["c-1"])).await.expect("a session");
        assert_eq!((second.session.session_id, second.session.info.turn), (first.session_id, 2));
        let pending = second.pending.expect("one write");
        assert_eq!(pending.request.results.len(), 1);
        h.commit(pending).await;

        let events = store.inner.events(ORG_A, Scope::Session, &first.session_id.to_string(), "read_file");
        assert_eq!(events.len(), 2, "the call and its result");
        assert_eq!((events[0].kind, events[0].verdict, events[0].tool.as_str()), (EventKind::Call, EventVerdict::Allow, "read_file"));
        assert_eq!((events[1].kind, events[1].tool.as_str()), (EventKind::Result, "read_file"));
    }

    #[tokio::test]
    async fn a_forged_result_counts_a_request_finding_and_writes_no_event() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        let (_store, h) = setup();
        let linked = link(&h, &turn(&[], &["nope-1", "nope-2"])).await.expect("a session");
        assert!(linked.session.findings.contains("forged_history"));
        assert!(!linked.session.findings.contains("mixed_sessions"));
        assert!(linked.pending.is_none(), "no touch and no result");
        let m = crate::application::tool_policy::testing::Snap::take(&snap);
        let f = |k: &str| m.counter(names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL, &[("side", "request"), ("kind", k)]);
        assert_eq!((f("forged_history"), f("mixed_sessions")), (2, 0));
    }

    #[tokio::test]
    async fn two_sessions_in_one_request_count_mixed_sessions_once() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        let (_store, h) = setup();
        for n in 0..2u128 {
            // Each first request starts its own session. Its call is linked by the settle.
            let s = link(&h, &turn(&[], &[])).await.expect("session").session;
            let id = format!("c-{n}");
            let items = [(id.as_str(), "t")];
            h.reserve(&ctx(), &s, &calls(&items), &[u(0xa0 + n)]).await;
            h.settle(&ctx().with_session(s.clone()), &reported_of(&items, &[(Decision::Allow, Some(u(0xa0 + n)))]));
        }
        h.tasks().close();
        h.tasks().wait().await;
        let linked = link(&h, &turn(&["c-0", "c-1"], &[])).await.expect("a session");
        assert!(linked.session.findings.contains("mixed_sessions"));
        assert_eq!(finding(&snap, "mixed_sessions"), Some(1));
    }

    #[tokio::test]
    async fn a_lookup_error_gives_no_session_and_counts_lookup_io() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        let store = Fake::new(Mode::Fail);
        let h = history(&store, Duration::from_secs(1));
        assert!(link(&h, &turn(&["c-1"], &[])).await.is_none());
        assert_eq!(errors(&snap, "lookup", "io"), Some(1));
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_lookup_spends_the_budget_and_skips_the_rest() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        let store = Fake::new(Mode::Sleep(Duration::from_secs(2)));
        let h = history(&store, Duration::from_secs(1));
        assert!(link(&h, &turn(&["c-1"], &[])).await.is_none());
        assert_eq!(errors(&snap, "lookup", "deadline"), Some(1));
        assert_eq!(store.count(), 1);

        // A session whose budget failed: reserve makes no store call.
        let budget = StoreBudget::new(Duration::from_secs(1));
        budget.fail();
        let session = Arc::new(SessionContext::new(ORG_A, u(5), 1, Findings::new(), budget));
        let findings = h.reserve(&ctx(), &session, &calls(&[("c-1", "t")]), &[u(1)]).await;
        assert_eq!(findings, Findings::new());
        assert_eq!(store.count(), 1, "no second call");
        assert_eq!(errors(&snap, "reserve", "deadline").unwrap_or(0), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn the_budget_is_cumulative_across_calls() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        let store = Fake::new(Mode::Sleep(Duration::from_millis(600)));
        let h = history(&store, Duration::from_secs(1));
        let session = link(&h, &turn(&[], &[])).await.expect("600 ms of 1 s").session;
        assert_eq!(session.budget.remaining(), Some(Duration::from_millis(400)));
        // The reserve gets the 400 ms that are left, not a new second.
        h.reserve(&ctx(), &session, &calls(&[("c-1", "t")]), &[u(1)]).await;
        assert_eq!(errors(&snap, "reserve", "deadline"), Some(1));
        assert!(session.budget.is_failed());
    }

    #[tokio::test]
    async fn a_reserve_writes_one_pending_event_per_call_in_every_history() {
        let (store, h) = setup();
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let findings = h.reserve(&ctx(), &session, &calls(&[("c-1", "Read_File"), ("c-2", "write")]), &[u(1), u(2)]).await;
        assert_eq!(findings, Findings::new());
        for (scope, id) in [(Scope::Session, session.session_id.to_string()), (Scope::Principal, PRINCIPAL.to_owned()), (Scope::Key, KEY.to_owned())] {
            let a = store.inner.events(ORG_A, scope, &id, "read_file");
            let b = store.inner.events(ORG_A, scope, &id, "write");
            assert_eq!((a.len(), b.len()), (1, 1), "{scope:?}");
            assert_eq!((a[0].id, a[0].verdict, a[0].kind), (u(1), EventVerdict::Pending, EventKind::Call));
            assert_eq!(b[0].id, u(2));
        }
    }

    #[tokio::test]
    async fn a_truncated_reply_gives_the_history_truncated_finding() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        let (store, h) = setup();
        store.reserve_reply.lock().unwrap().push_back(AppendReply {
            written: 1,
            trimmed_retention: 1,
            trimmed_cap: 2,
            truncated: vec![(Scope::Session, "t".into())],
        });
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let findings = h.reserve(&ctx(), &session, &calls(&[("c-1", "t")]), &[u(1)]).await;
        assert!(findings.contains("history_truncated"));
        let m = crate::application::tool_policy::testing::Snap::take(&snap);
        let trimmed = |r: &str| m.counter(names::GATEWAY_TOOL_HISTORY_EVENTS_TRIMMED_TOTAL, &[("reason", r)]);
        assert_eq!((trimmed("retention"), trimmed("cap")), (1, 2));
    }

    #[tokio::test]
    async fn a_reserve_error_is_counted_and_gives_no_finding() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        let store = Fake::new(Mode::Fail);
        let h = history(&store, Duration::from_secs(1));
        let session = Arc::new(SessionContext::new(ORG_A, u(5), 1, Findings::new(), StoreBudget::new(Duration::from_secs(1))));
        let findings = h.reserve(&ctx(), &session, &calls(&[("c-1", "t")]), &[u(1)]).await;
        assert_eq!(findings, Findings::new());
        assert_eq!(errors(&snap, "reserve", "io"), Some(1));
        assert!(session.budget.is_failed());
        // The next store call of the request is skipped.
        h.reserve(&ctx(), &session, &calls(&[("c-2", "t")]), &[u(2)]).await;
        assert_eq!(store.count(), 1);
    }

    #[test]
    fn settle_plan_marks_allow_and_deny_and_links_only_forwarded_calls() {
        let reported = reported_of(&[("c-1", "Read_File"), ("c-2", "rm")], &[(Decision::Allow, Some(u(1))), (BLOCK, Some(u(2)))]);
        let plan = settle_plan(&reported, &|_| true, &mut counting_mint());
        assert!(plan.append.is_empty());
        assert_eq!(
            plan.settle,
            vec![
                SettleItem {
                    event_id: u(1),
                    verdict: EventVerdict::Allow,
                    tool: "read_file".into(),
                    call_id: Some("c-1".into()),
                    item_id: None
                },
                SettleItem {
                    event_id: u(2),
                    verdict: EventVerdict::Deny,
                    tool: "rm".into(),
                    call_id: None,
                    item_id: None
                },
            ]
        );
    }

    #[test]
    fn settle_plan_appends_a_forwarded_call_without_a_decision() {
        let reported = reported_of(&[("c-1", "Read_File")], &[(Decision::Allow, None)]);
        let plan = settle_plan(&reported, &|_| true, &mut counting_mint());
        assert!(plan.settle.is_empty());
        assert_eq!(
            plan.append,
            vec![ForwardedCall {
                id: u(101),
                tool: "read_file".into(),
                call_id: Some("c-1".into()),
                item_id: None
            }]
        );
    }

    #[test]
    fn settle_plan_carries_the_item_id_of_a_forwarded_local_shell_call() {
        let body = serde_json::json!({"id": "resp_1", "object": "response", "status": "completed", "output": [
            {"type": "local_shell_call", "id": "lsh_1", "call_id": "call_1", "action": {"type": "exec", "command": ["ls"]}, "status": "completed"}
        ]});
        let parsed = parse_body(Format::OpenAiResponses, body.to_string().as_bytes(), 1 << 20).expect("body");
        let allow = parsed.reported(&[DecidedCall::new(Decision::Allow, None).with_decision_id(u(1))], Basis::Decider);
        let item = settle_plan(&allow, &|_| true, &mut counting_mint()).settle.remove(0);
        assert_eq!((item.verdict, item.call_id.as_deref(), item.item_id.as_deref()), (EventVerdict::Allow, Some("call_1"), Some("lsh_1")));
        let deny = parsed.reported(&[DecidedCall::new(BLOCK, None).with_decision_id(u(1))], Basis::Decider);
        let item = settle_plan(&deny, &|_| true, &mut counting_mint()).settle.remove(0);
        assert_eq!((item.verdict, item.call_id, item.item_id), (EventVerdict::Deny, None, None));
        // No decision: the append carries both links.
        let none = parsed.reported(&[DecidedCall::new(Decision::Allow, None)], Basis::Decider);
        let call = settle_plan(&none, &|_| true, &mut counting_mint()).append.remove(0);
        assert_eq!((call.call_id.as_deref(), call.item_id.as_deref()), (Some("call_1"), Some("lsh_1")));
    }

    #[test]
    fn settle_plan_ignores_a_block_without_a_decision_and_an_unenforceable_entry() {
        let mut reported = reported_of(&[("c-1", "rm")], &[(BLOCK, None)]);
        reported.push(ReportedCall::new(entry("web_search", ReportDecision::Unenforceable), None, None));
        // An allow entry with no call facts and no decision id.
        reported.push(ReportedCall::new(entry("x", ReportDecision::Allow), None, None));
        let plan = settle_plan(&reported, &|_| true, &mut counting_mint());
        assert!(plan.settle.is_empty() && plan.append.is_empty());
    }

    #[tokio::test]
    async fn settle_runs_on_the_tracker_and_sets_the_verdict() {
        let (store, h) = setup();
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let items = [("c-1", "a"), ("c-2", "b"), ("c-3", "c")];
        h.reserve(&ctx(), &session, &calls(&items[..2]), &[u(1), u(2)]).await;
        let reported = reported_of(&items, &[(Decision::Allow, Some(u(1))), (BLOCK, Some(u(2))), (Decision::Allow, None)]);
        h.settle(&ctx().with_session(session.clone()), &reported);
        h.tasks().close();
        h.tasks().wait().await;
        let sid = session.session_id.to_string();
        let verdict = |tool: &str| store.inner.events(ORG_A, Scope::Session, &sid, tool).iter().map(|e| e.verdict).collect::<Vec<_>>();
        assert_eq!(
            (verdict("a"), verdict("b"), verdict("c")),
            (vec![EventVerdict::Allow], vec![EventVerdict::Deny], vec![EventVerdict::Allow])
        );
    }

    #[tokio::test]
    async fn a_closed_tracker_makes_no_settle_call() {
        let reported = reported_of(&[("c-1", "a")], &[(Decision::Allow, Some(u(1)))]);
        let (store, h) = setup();
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let before = store.count();
        h.tasks().close();
        h.settle(&ctx().with_session(session), &reported);
        h.tasks().wait().await;
        assert_eq!(store.count(), before, "the tracker is closed");
    }

    /// T10-a and T10-b: a request whose `reserve` failed has a failed budget, and settle and append
    /// still run. The latency-path calls stay skipped.
    #[tokio::test]
    async fn settle_and_append_run_after_a_failed_reserve() {
        let (store, h) = setup();
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        session.budget.fail();
        let reported = reported_of(&[("c-1", "a"), ("c-2", "b")], &[(Decision::Allow, Some(u(1))), (Decision::Allow, None)]);
        h.reserve(&ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)]).await;
        let before = store.count();
        h.settle(&ctx().with_session(session.clone()), &reported);
        h.tasks().close();
        h.tasks().wait().await;
        assert_eq!(store.count(), before + 1, "one append: the skipped reserve left no event to settle");
        let sid = session.session_id.to_string();
        let a = store.inner.events(ORG_A, Scope::Session, &sid, "a");
        assert_eq!((a.len(), a[0].id, a[0].verdict), (1, u(1), EventVerdict::Allow), "the allowed call is appended under its decision id");
        let b = store.inner.events(ORG_A, Scope::Session, &sid, "b");
        assert_eq!(b.len(), 1, "the forwarded call without a decision is written");
        assert_eq!(b[0].verdict, EventVerdict::Allow);
        assert!(session.budget.is_failed());
    }

    #[tokio::test]
    async fn a_real_reserve_error_does_not_stop_settle_and_append() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        let store = Fake::new(Mode::Fail);
        let h = history(&store, Duration::from_secs(1));
        let session = Arc::new(SessionContext::new(ORG_A, u(5), 1, Findings::new(), StoreBudget::new(Duration::from_secs(1))));
        h.reserve(&ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)]).await;
        assert!(session.budget.is_failed());
        let reported = reported_of(&[("c-1", "a"), ("c-2", "b")], &[(Decision::Allow, Some(u(1))), (Decision::Allow, None)]);
        h.settle(&ctx().with_session(session.clone()), &reported);
        h.tasks().close();
        h.tasks().wait().await;
        assert_eq!(store.count(), 2, "reserve, then one append: no reserved event to settle");
        let m = crate::application::tool_policy::testing::Snap::take(&snap);
        let e = |op: &str| m.counter(names::GATEWAY_TOOL_HISTORY_STORE_ERRORS_TOTAL, &[("op", op), ("kind", "io")]);
        assert_eq!((e("reserve"), e("settle"), e("append")), (1, 0, 1));
    }

    #[tokio::test]
    async fn a_settle_error_is_counted_and_the_append_is_still_written() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        let store = Fake::new(Mode::FailSettle);
        let h = history(&store, Duration::from_secs(1));
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        h.reserve(&ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)]).await;
        let reported = reported_of(&[("c-1", "a"), ("c-2", "b")], &[(Decision::Allow, Some(u(1))), (Decision::Allow, None)]);
        h.settle(&ctx().with_session(session.clone()), &reported);
        h.tasks().close();
        h.tasks().wait().await;
        assert_eq!(errors(&snap, "settle", "io"), Some(1));
        assert!(!session.budget.is_failed(), "an off-path error does not fail the budget");
        let events = store.inner.events(ORG_A, Scope::Session, &session.session_id.to_string(), "b");
        assert_eq!(events.len(), 1, "the append was written");
    }

    #[test]
    fn settle_plan_appends_an_allowed_call_whose_event_was_not_reserved() {
        let reported = reported_of(
            &[("c-1", "Read_File"), ("c-2", "rm"), ("c-3", "x")],
            &[(Decision::Allow, Some(u(1))), (BLOCK, Some(u(2))), (Decision::Allow, Some(u(3)))],
        );
        let plan = settle_plan(&reported, &|id| id == u(3), &mut counting_mint());
        // The reserved call settles. The unreserved allowed call appends under its decision id. The
        // unreserved denied call gives nothing: it was not forwarded, so it needs no link.
        assert_eq!(plan.settle.len(), 1);
        assert_eq!(plan.settle[0].event_id, u(3));
        assert_eq!(
            plan.append,
            vec![ForwardedCall {
                id: u(1),
                tool: "read_file".into(),
                call_id: Some("c-1".into()),
                item_id: None
            }]
        );
    }

    /// Final review finding 1: an allowed call whose reserve wrote no event is appended under its
    /// decision id. Its result on the next request then links to the session: no `forged_history`.
    async fn an_unreserved_allowed_call_links(mode: Mode, skip_reserve: bool) {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        let store = Fake::new(mode);
        let h = history(&store, Duration::from_secs(1));
        let first = link(&h, &turn(&[], &[])).await.expect("a session").session;
        if skip_reserve {
            // An earlier latency-path error: the budget skips the reserve.
            first.budget.fail();
        }
        let items = [("c-1", "a"), ("c-2", "b")];
        h.reserve(&ctx(), &first, &calls(&items), &[u(1), u(2)]).await;
        assert!(!first.is_reserved(u(1)) && !first.is_reserved(u(2)));
        h.settle(&ctx().with_session(first.clone()), &reported_of(&items, &[(Decision::Allow, Some(u(1))), (BLOCK, Some(u(2)))]));
        h.tasks().close();
        h.tasks().wait().await;
        let sid = first.session_id.to_string();
        let a = store.inner.events(ORG_A, Scope::Session, &sid, "a");
        assert_eq!((a.len(), a[0].id, a[0].verdict), (1, u(1), EventVerdict::Allow));
        assert!(store.inner.events(ORG_A, Scope::Session, &sid, "b").is_empty(), "a denied call with no reserved event is not appended");

        let second = link(&h, &turn(&[], &["c-1"])).await.expect("a session");
        assert!(!second.session.findings.contains("forged_history"), "c-1 has a call link");
        assert_eq!((second.session.session_id, second.session.info.turn), (first.session_id, 2));
        assert_eq!(second.pending.expect("one result write").request.results.len(), 1);
        assert_eq!(finding(&snap, "forged_history").unwrap_or(0), 0);
    }

    #[tokio::test]
    async fn a_failed_reserve_still_links_the_allowed_call() {
        an_unreserved_allowed_call_links(Mode::FailReserve, false).await;
    }

    #[tokio::test]
    async fn a_budget_skipped_reserve_still_links_the_allowed_call() {
        an_unreserved_allowed_call_links(Mode::Pass, true).await;
    }

    #[tokio::test]
    async fn a_reserve_marks_its_decision_ids() {
        let (_store, h) = setup();
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        h.reserve(&ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)]).await;
        assert!(session.is_reserved(u(1)));
        assert!(!session.is_reserved(u(2)));
    }

    /// A latency-path call that the spent budget skips, with no earlier error, counts one
    /// `deadline` for its op and makes no store call.
    #[tokio::test]
    async fn a_spent_budget_counts_the_skipped_call_as_a_deadline() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        let (store, h) = setup();
        let budget = StoreBudget::new(Duration::from_millis(10));
        budget.charge(Duration::from_millis(10));
        let session = Arc::new(SessionContext::new(ORG_A, u(5), 1, Findings::new(), budget));
        h.reserve(&ctx(), &session, &calls(&[("c-1", "t")]), &[u(1)]).await;
        h.reserve(&ctx(), &session, &calls(&[("c-2", "t")]), &[u(2)]).await;
        assert_eq!(store.count(), 0);
        assert_eq!(errors(&snap, "reserve", "deadline"), Some(1), "the first skip counts; it fails the budget");
        assert!(session.budget.is_failed());
    }

    #[tokio::test]
    async fn ids_past_the_cap_are_counted_per_list() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        let (_store, h) = setup();
        let mut f = TurnFacts::default();
        f.latest.dropped_calls = 2;
        f.latest.dropped_results = 5;
        f.latest.dropped_refs = 1;
        link(&h, &ToolTurn::Parsed(f)).await.expect("a session");
        // A snapshot drains the counters: take one and read it three times.
        let m = crate::application::tool_policy::testing::Snap::take(&snap);
        let capped = |l: &str| m.counter(names::GATEWAY_TOOL_HISTORY_IDS_CAPPED_TOTAL, &[("list", l)]);
        assert_eq!((capped("calls"), capped("results"), capped("refs")), (2, 5, 1));
    }

    #[tokio::test]
    async fn an_unavailable_turn_links_with_no_ids() {
        let (_store, h) = setup();
        let linked = link(&h, &ToolTurn::Unavailable("test")).await.expect("a session");
        assert_eq!(linked.session.info.turn, 1);
        assert!(linked.pending.is_none());
    }

    #[test]
    fn priming_sets_every_history_series_to_zero() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        prime_tool_history_metrics();
        for op in ["lookup", "write", "reserve", "settle", "append"] {
            for kind in ["io", "server", "decode", "deadline"] {
                assert_eq!(errors(&snap, op, kind), Some(0), "{op}/{kind}");
            }
        }
        for list in ["calls", "results", "refs"] {
            assert_eq!(counter(&snap, names::GATEWAY_TOOL_HISTORY_IDS_CAPPED_TOTAL, &[("list", list)]), Some(0));
        }
        for reason in ["retention", "cap"] {
            assert_eq!(counter(&snap, names::GATEWAY_TOOL_HISTORY_EVENTS_TRIMMED_TOTAL, &[("reason", reason)]), Some(0));
        }
        let series = snap.snapshot().into_vec().into_iter().filter(|(k, ..)| k.key().name().starts_with("gateway_tool_history_")).count();
        assert_eq!(series, 20 + 3 + 2);
    }
}
