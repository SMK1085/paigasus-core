// SPDX-License-Identifier: Apache-2.0

//! SMA-753: the `ToolHistory` service. It wraps the `SessionStore` port with the store budget of one
//! request (spec D10), the error counting and the history metrics (D13).
//!
//! The deadline is one cumulative store-time budget per request. Each store call on the latency path
//! (`lookup`, `write`, `reserve`) runs under `timeout(budget.remaining())` and charges its elapsed
//! time. After the first store error, every later store call of the request is skipped. In PR 1 a
//! store error has no policy effect: it is counted and the request continues. No client id (call id,
//! result id, `ref`) becomes a metric label or a log field.
//!
//! The reserve also reads the history window of the policy set (spec P4) and gives it to the
//! decider with the hold events. A failed read is `WindowRead::Failed`: the slots that need it are
//! missing (fail closed, spec P7).

use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use metrics::counter;
use paigasus_gateway_core::decision_log::{CallFacts, FindingRecord, ReportedCall};
use paigasus_gateway_core::{DecisionContext, HistoryPolicy, LatestTurn, ReadPlan, ReportDecision, SessionContext, StoreBudget, ToolCall, ToolTurn, call_fields};
use paigasus_kernel::detect::{DETECTOR_KERNEL, FORGED_HISTORY, HISTORY_TRUNCATED, MIXED_SESSIONS, Severity};
use paigasus_kernel::mint_uuid7;
use paigasus_kernel::tool_policy::history::{Event, EventKind as KernelEventKind, Fields, HistoryDigester, KeyId, MissingReason, Scope as KernelScope, Window, WindowSet};
use paigasus_kernel::tool_policy::{Findings, normalise};
use paigasus_observability::names;
use tokio::time::Instant;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

use crate::application::tool_policy::decider::count_finding;
use crate::application::tool_policy::slots::PARTS as SLOT_PARTS;
use crate::domain::decision_log::Entropy;
use crate::domain::limits::{Clock, UnavailableKind};
use crate::domain::tool_history::{
    AppendCall, AppendReply, AppendRequest, CallEvent, EventKind, EventVerdict, Histories, HistoryStoreError, LookupReply, LookupRequest, ReadRequest, ReserveRequest, ResultEvent, Scope,
    SessionStore, SetWindow, SettleItem, SettleRequest, StoredEvent, WriteRequest, check_window, history_tool, link_session,
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

/// The history window of a hold (spec P4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowRead {
    /// The policy set has no history condition: no set was read.
    NotNeeded,
    /// One `WindowSet` per set of the read plan, in its order, with the events of this hold
    /// removed. A `Key` set of a caller with no API key holds the principal events (F14).
    Read(Window),
    /// The reserve failed, ran past the deadline, or the budget skipped it.
    Failed,
}

/// What the reserve gives the decider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReserveOutcome {
    /// `history_truncated` when the reply flags a set.
    pub findings: Findings,
    /// The time of the reserve: the time of the hold events, and the base of the window.
    pub now_ms: u64,
    /// One request event per call of the hold, in call order, with its stored fields.
    pub hold: Vec<Event>,
    pub window: WindowRead,
}

/// The store scope of a kernel scope. A `Key` scope with no API key is the principal history (D5,
/// F14). A later kernel scope has no store scope: its sets stay out of the window, so the
/// evaluator finds no set (fail closed).
fn store_scope(scope: KernelScope, keyed: bool) -> Option<Scope> {
    match scope {
        KernelScope::Session => Some(Scope::Session),
        KernelScope::Principal => Some(Scope::Principal),
        KernelScope::Key if keyed => Some(Scope::Key),
        KernelScope::Key => Some(Scope::Principal),
        _ => None,
    }
}

/// The store read of a read plan: `from_ms = now_ms - w_max` (saturating), and each set once in
/// the plan order after the key alias.
fn read_request(plan: &ReadPlan, now_ms: u64, keyed: bool) -> ReadRequest {
    let mut seen = HashSet::new();
    let sets = plan
        .sets
        .iter()
        .filter_map(|s| store_scope(s.scope, keyed).map(|scope| (scope, s.tool.clone())))
        .filter(|set| seen.insert(set.clone()))
        .collect();
    ReadRequest {
        from_ms: now_ms.saturating_sub(plan.w_max_ms),
        sets,
    }
}

fn kernel_event(e: &StoredEvent) -> Event {
    let kind = match e.kind {
        EventKind::Call => KernelEventKind::Request,
        EventKind::Result => KernelEventKind::Response,
    };
    Event::new(e.id, e.ts_ms, kind, e.tool.clone(), e.fields.clone(), e.key)
}

/// The kernel window of a checked reply: one set per plan set, under the kernel scope of the plan.
/// The events of this hold (`own`) are removed: the evaluator gets them through the hold.
fn window_of(plan: &ReadPlan, keyed: bool, reply: &[SetWindow], own: &HashSet<Uuid>) -> Window {
    let sets = plan
        .sets
        .iter()
        .filter_map(|s| {
            let scope = store_scope(s.scope, keyed)?;
            let got = reply.iter().find(|w| w.scope == scope && w.tool == s.tool)?;
            let events = got.events.iter().filter(|e| !own.contains(&e.id)).map(kernel_event).collect();
            Some(WindowSet::new(s.scope, s.tool.clone(), events, got.mark_ms, got.truncated))
        })
        .collect();
    Window::new(sets)
}

/// The store calls that a report needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlePlan {
    pub settle: Vec<SettleItem>,
    pub append: Vec<AppendCall>,
}

/// Split the report entries into the settles and the appends (spec P9, plan decision D-D). Pure.
///
/// The event id of an entry is its decision id, else `claim(call facts)`: the hold intent of a call
/// that the interceptor decided without the decider (a decide timeout or a decider error).
///
/// - An id that `reserved` holds is a settle: `Allow` when the entry is allowed, else `Deny`. Both
///   carry the call id and the item id: an allow writes the `c:` links, a deny deletes its own.
/// - An id that `reserved` does not hold, or no id, with call facts is an append with the verdict of
///   the entry (`Allow`, or `Deny` for a block) and the stored fields of `fields`. No id mints one.
///   The reserve failed, the budget skipped it, or the call was never decided. A denied call is
///   appended too, so it counts.
/// - No call facts gives nothing. An unenforceable entry gives nothing.
pub fn settle_plan(
    reported: &[ReportedCall],
    reserved: &dyn Fn(Uuid) -> bool,
    claim: &mut dyn FnMut(&CallFacts) -> Option<Uuid>,
    fields: &dyn Fn(&ReportedCall) -> (Fields, Option<KeyId>),
    mint: &mut dyn FnMut() -> Uuid,
) -> SettlePlan {
    let mut plan = SettlePlan {
        settle: Vec::new(),
        append: Vec::new(),
    };
    for r in reported {
        let verdict = match r.entry.decision {
            ReportDecision::Allow => EventVerdict::Allow,
            ReportDecision::Block => EventVerdict::Deny,
            ReportDecision::Unenforceable => continue,
        };
        let id = r.decision_id.or_else(|| r.call.as_ref().and_then(&mut *claim));
        // `normalise(&r.entry.name)` equals `history_tool(call)` only because `report.rs` sets `name`
        // from `ToolCall::display_name()`. Keep the two in step.
        let tool = normalise(&r.entry.name);
        let (call_id, item_id) = r.call.as_ref().map_or((None, None), |c| (c.id.clone(), c.item_id.clone()));
        match id {
            Some(event_id) if reserved(event_id) => plan.settle.push(SettleItem {
                event_id,
                verdict,
                tool,
                call_id,
                item_id,
            }),
            _ if r.call.is_some() => {
                let (stored, key) = fields(r);
                plan.append.push(AppendCall {
                    id: id.unwrap_or_else(&mut *mint),
                    tool,
                    verdict,
                    call_id,
                    item_id,
                    fields: stored,
                    key,
                });
            }
            _ => {}
        }
    }
    plan
}

/// The stored fields of a report entry: its extracted values, digested under the current key. An
/// entry with no field plan stores no fields.
fn stored_fields(reported: &ReportedCall, org: Uuid, digester: &dyn HistoryDigester) -> (Fields, Option<KeyId>) {
    match &reported.fields {
        Some(extracted) => extracted.clone().into_fields(org, digester),
        None => (Fields::Values(BTreeMap::new()), None),
    }
}

/// Prime every series of the history metrics at 0, the 16 `(part, reason)` pairs of the slots metric included.
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
    counter!(names::GATEWAY_TOOL_HISTORY_EVENTS_MISSING_TOTAL).absolute(0);
    for part in SLOT_PARTS {
        for reason in MissingReason::ALL {
            counter!(names::GATEWAY_TOOL_HISTORY_SLOTS_MISSING_TOTAL, "part" => part.as_str(), "reason" => reason.as_str()).absolute(0);
        }
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
    /// The one digester of the gateway: the policy cache binds its literals with it too (spec P3).
    digester: Arc<dyn HistoryDigester>,
    tasks: TaskTracker,
}

impl ToolHistory {
    #[must_use]
    pub fn new(store: Arc<dyn SessionStore>, clock: Arc<dyn Clock>, entropy: Arc<dyn Entropy>, deadline: Duration, digester: Arc<dyn HistoryDigester>) -> Self {
        Self {
            store,
            clock,
            entropy,
            deadline,
            digester,
            tasks: TaskTracker::new(),
        }
    }

    /// The digester of the stored fields, for the policy cache: the literals and the call values
    /// must use the same key.
    #[must_use]
    pub fn digester(&self) -> Arc<dyn HistoryDigester> {
        Arc::clone(&self.digester)
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
        // The guard charges the elapsed time once, also when the caller drops this future (a hold
        // timeout), so the next call of the same request does not get the time back.
        let guard = ChargeOnDrop { budget, start: Instant::now() };
        let out = tokio::time::timeout(remaining, call).await;
        drop(guard);
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
        // The metric labels come from the closed triple table (SMA-855 P-9): detector `kernel`.
        if link.mixed_sessions {
            findings.add(MIXED_SESSIONS, Severity::Medium);
            count_finding("request", &FindingRecord::new(MIXED_SESSIONS, Some(Severity::Medium), DETECTOR_KERNEL, ""), None);
        }
        if link.forged_results > 0 {
            findings.add(FORGED_HISTORY, Severity::High);
            let forged = FindingRecord::new(FORGED_HISTORY, Some(Severity::High), DETECTOR_KERNEL, "");
            for _ in 0..link.forged_results {
                count_finding("request", &forged, None);
            }
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

    /// Inside the decide future. Writes one pending event per call with the fields of the field
    /// plan of `policy` (spec P3), and reads the window of its read plan (spec P4).
    ///
    /// The findings hold `history_truncated` when the reply flags a set. A store error is counted
    /// (op `reserve`), gives no finding and a `Failed` window. A reply whose window does not hold
    /// each requested set in order is a `decode` error. With no call, nothing is written or read.
    pub async fn reserve(&self, ctx: &DecisionContext, session: &SessionContext, calls: &[ToolCall], ids: &[Uuid], policy: Option<&HistoryPolicy>) -> ReserveOutcome {
        let now_ms = self.now_ms();
        let mut outcome = ReserveOutcome {
            findings: Findings::new(),
            now_ms,
            hold: Vec::new(),
            window: WindowRead::NotNeeded,
        };
        if calls.is_empty() {
            return outcome;
        }
        debug_assert_eq!(calls.len(), ids.len(), "one id per call");
        // The helper of the report site: the reserve and the report store the same fields.
        let plan = policy.map(|p| p.fields().as_ref());
        let events: Vec<CallEvent> = calls
            .iter()
            .zip(ids)
            .map(|(c, id)| {
                let (fields, key) = call_fields(c, plan).map_or_else(|| (Fields::Values(BTreeMap::new()), None), |x| x.into_fields(session.org, self.digester.as_ref()));
                CallEvent {
                    id: *id,
                    tool: history_tool(c),
                    call_id: c.id.clone(),
                    item_id: c.item_id(),
                    fields,
                    key,
                }
            })
            .collect();
        outcome.hold = events
            .iter()
            .map(|e| Event::new(e.id, now_ms, KernelEventKind::Request, e.tool.clone(), e.fields.clone(), e.key))
            .collect();
        let keyed = ctx.api_key_id.is_some();
        let read_plan = policy.and_then(HistoryPolicy::read);
        let req = ReserveRequest {
            histories: Histories {
                org: session.org,
                principal: ctx.principal.clone(),
                key: ctx.api_key_id.clone(),
                session: session.session_id,
            },
            turn: session.info.turn,
            calls: events,
            now_ms,
            read: read_plan.map(|p| read_request(p, now_ms, keyed)),
        };
        let reply = self
            .timed(&session.budget, "reserve", async {
                let reply = self.store.reserve(&req).await?;
                if let Some(read) = &req.read {
                    check_window(read, &reply.window)?;
                }
                Ok(reply)
            })
            .await;
        let Some(reply) = reply else {
            if read_plan.is_some() {
                outcome.window = WindowRead::Failed;
            }
            return outcome;
        };
        // Only a reserve that returned wrote its events. The report site appends the others.
        session.mark_reserved(ids);
        count_trimmed(&reply);
        if reply.missing_bodies > 0 {
            counter!(names::GATEWAY_TOOL_HISTORY_EVENTS_MISSING_TOTAL).increment(reply.missing_bodies);
        }
        if !reply.truncated.is_empty() {
            outcome.findings.add(HISTORY_TRUNCATED, Severity::Low);
        }
        if let Some(plan) = read_plan {
            let own: HashSet<Uuid> = ids.iter().copied().collect();
            outcome.window = WindowRead::Read(window_of(plan, keyed, &reply.window, &own));
        }
        outcome
    }

    /// At the report site. Spawns one task on the tracker for the settle and, when needed, the append.
    pub fn settle(&self, ctx: &DecisionContext, reported: &[ReportedCall]) {
        let Some(session) = ctx.session.clone() else { return };
        // Settle and append are off the latency path: a failed budget does not skip them (T10-a, T10-b).
        if self.tasks.is_closed() {
            return;
        }
        let org = session.org;
        let plan = settle_plan(
            reported,
            &|id| session.is_reserved(id),
            &mut |call| session.claim_intent(call),
            &|r| stored_fields(r, org, self.digester.as_ref()),
            &mut || self.mint(),
        );
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

/// Charges the time since `start` to the budget when it drops. It drops once, so it charges once.
struct ChargeOnDrop<'a> {
    budget: &'a StoreBudget,
    start: Instant,
}

impl Drop for ChargeOnDrop<'_> {
    fn drop(&mut self) {
        self.budget.charge(self.start.elapsed());
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use metrics_util::debugging::{DebuggingRecorder, Snapshotter};
    use paigasus_gateway_core::decision_log::CallFacts;
    use paigasus_gateway_core::decision_log::ReportedCall;
    use paigasus_gateway_core::{Basis, BlockCode, DecidedCall, Decision, DecisionContext, Format, ReportDecision, ReportEntry, ToolTurn, TurnFacts, parse_body};
    use paigasus_kernel::tool_policy::Findings;
    use uuid::Uuid;

    use super::*;
    use crate::adapters::history::HmacHistoryDigester;
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
        /// The `read` of each reserve, in call order.
        reads: Mutex<Vec<Option<ReadRequest>>>,
    }

    impl Fake {
        fn new(mode: Mode) -> Arc<Self> {
            Arc::new(Fake {
                inner: MemorySessionStore::new(),
                mode,
                calls: AtomicUsize::new(0),
                reserve_reply: Mutex::new(VecDeque::new()),
                reads: Mutex::new(Vec::new()),
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
            self.reads.lock().unwrap().push(req.read.clone());
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
        ToolHistory::new(
            store,
            Arc::new(FixedClock(at("2026-11-22T10:00:00Z"))),
            Arc::new(CountingEntropy(AtomicU64::new(0))),
            deadline,
            Arc::new(HmacHistoryDigester::keyless()),
        )
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

    /// The severity label of a history kind (SMA-855 P-9): its triple in `FINDING_TRIPLES`.
    fn severity_of(kind: &str) -> &'static str {
        match kind {
            "forged_history" => "high",
            "mixed_sessions" => "medium",
            _ => "low",
        }
    }

    fn finding(s: &Snapshotter, kind: &str) -> Option<u64> {
        counter(
            s,
            names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL,
            &[("side", "request"), ("kind", kind), ("detector", "kernel"), ("severity", severity_of(kind))],
        )
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
        chat_body(items).reported(&decided, Basis::Decider, None)
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

    fn no_fields() -> Fields {
        Fields::Values(BTreeMap::new())
    }

    /// `settle_plan` with no hold intent and no stored fields.
    fn plan_of(reported: &[ReportedCall], reserved: &dyn Fn(Uuid) -> bool) -> SettlePlan {
        settle_plan(reported, reserved, &mut |_| None, &|_| (no_fields(), None), &mut counting_mint())
    }

    fn appended(id: Uuid, tool: &str, verdict: EventVerdict, call_id: &str) -> AppendCall {
        AppendCall {
            id,
            tool: tool.into(),
            verdict,
            call_id: Some(call_id.into()),
            item_id: None,
            fields: no_fields(),
            key: None,
        }
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
        h.reserve(&ctx(), &first, &calls(&items), &[decision_id], None).await;
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
        let f = |k: &str| {
            m.counter(
                names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL,
                &[("side", "request"), ("kind", k), ("detector", "kernel"), ("severity", severity_of(k))],
            )
        };
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
            h.reserve(&ctx(), &s, &calls(&items), &[u(0xa0 + n)], None).await;
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
        let findings = h.reserve(&ctx(), &session, &calls(&[("c-1", "t")]), &[u(1)], None).await.findings;
        assert_eq!(findings, Findings::new());
        assert_eq!(store.count(), 1, "no second call");
        assert_eq!(errors(&snap, "reserve", "deadline").unwrap_or(0), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_dropped_store_call_still_charges_the_budget() {
        let store = Fake::new(Mode::Sleep(Duration::from_secs(2)));
        let h = history(&store, Duration::from_secs(1));
        let budget = StoreBudget::new(Duration::from_secs(1));
        let session = Arc::new(SessionContext::new(ORG_A, u(5), 1, Findings::new(), budget));
        // The caller drops the reserve after 300 ms (a hold timeout).
        let held = tokio::time::timeout(Duration::from_millis(300), h.reserve(&ctx(), &session, &calls(&[("c-1", "t")]), &[u(1)], None)).await;
        assert!(held.is_err());
        assert_eq!(session.budget.remaining(), Some(Duration::from_millis(700)));
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
        h.reserve(&ctx(), &session, &calls(&[("c-1", "t")]), &[u(1)], None).await;
        assert_eq!(errors(&snap, "reserve", "deadline"), Some(1));
        assert!(session.budget.is_failed());
    }

    #[tokio::test]
    async fn a_reserve_writes_one_pending_event_per_call_in_every_history() {
        let (store, h) = setup();
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let findings = h.reserve(&ctx(), &session, &calls(&[("c-1", "Read_File"), ("c-2", "write")]), &[u(1), u(2)], None).await.findings;
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
            window: Vec::new(),
            missing_bodies: 0,
        });
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let findings = h.reserve(&ctx(), &session, &calls(&[("c-1", "t")]), &[u(1)], None).await.findings;
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
        let findings = h.reserve(&ctx(), &session, &calls(&[("c-1", "t")]), &[u(1)], None).await.findings;
        assert_eq!(findings, Findings::new());
        assert_eq!(errors(&snap, "reserve", "io"), Some(1));
        assert!(session.budget.is_failed());
        // The next store call of the request is skipped.
        h.reserve(&ctx(), &session, &calls(&[("c-2", "t")]), &[u(2)], None).await;
        assert_eq!(store.count(), 1);
    }

    /// A deny settle carries the call ids too: it deletes the `c:` links that the reserve wrote
    /// when they still name its event (spec P8).
    #[test]
    fn settle_plan_marks_allow_and_deny_and_both_carry_the_call_ids() {
        let reported = reported_of(&[("c-1", "Read_File"), ("c-2", "rm")], &[(Decision::Allow, Some(u(1))), (BLOCK, Some(u(2)))]);
        let plan = plan_of(&reported, &|_| true);
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
                    call_id: Some("c-2".into()),
                    item_id: None
                },
            ]
        );
    }

    #[test]
    fn settle_plan_appends_a_forwarded_call_without_a_decision() {
        let reported = reported_of(&[("c-1", "Read_File")], &[(Decision::Allow, None)]);
        let plan = plan_of(&reported, &|_| true);
        assert!(plan.settle.is_empty());
        assert_eq!(plan.append, vec![appended(u(101), "read_file", EventVerdict::Allow, "c-1")]);
    }

    #[test]
    fn settle_plan_carries_the_item_id_of_a_forwarded_local_shell_call() {
        let body = serde_json::json!({"id": "resp_1", "object": "response", "status": "completed", "output": [
            {"type": "local_shell_call", "id": "lsh_1", "call_id": "call_1", "action": {"type": "exec", "command": ["ls"]}, "status": "completed"}
        ]});
        let parsed = parse_body(Format::OpenAiResponses, body.to_string().as_bytes(), 1 << 20).expect("body");
        let allow = parsed.reported(&[DecidedCall::new(Decision::Allow, None).with_decision_id(u(1))], Basis::Decider, None);
        let item = plan_of(&allow, &|_| true).settle.remove(0);
        assert_eq!((item.verdict, item.call_id.as_deref(), item.item_id.as_deref()), (EventVerdict::Allow, Some("call_1"), Some("lsh_1")));
        let deny = parsed.reported(&[DecidedCall::new(BLOCK, None).with_decision_id(u(1))], Basis::Decider, None);
        let item = plan_of(&deny, &|_| true).settle.remove(0);
        assert_eq!((item.verdict, item.call_id.as_deref(), item.item_id.as_deref()), (EventVerdict::Deny, Some("call_1"), Some("lsh_1")));
        // No decision: the append carries both links.
        let none = parsed.reported(&[DecidedCall::new(Decision::Allow, None)], Basis::Decider, None);
        let call = plan_of(&none, &|_| true).append.remove(0);
        assert_eq!((call.call_id.as_deref(), call.item_id.as_deref()), (Some("call_1"), Some("lsh_1")));
    }

    /// No call facts gives nothing: no settle, no append, and no claim of a hold intent. An
    /// unreserved decision id does not change it.
    #[test]
    fn an_entry_without_call_facts_gives_nothing() {
        let reported = vec![
            ReportedCall::new(entry("web_search", ReportDecision::Unenforceable), None, None),
            ReportedCall::new(entry("x", ReportDecision::Allow), None, None),
            ReportedCall::new(entry("y", ReportDecision::Block), None, None),
            ReportedCall::new(entry("z", ReportDecision::Allow), None, None).with_decision_id(Some(u(9))),
        ];
        let mut claims = 0;
        let plan = settle_plan(
            &reported,
            &|id| id == u(1),
            &mut |_| {
                claims += 1;
                Some(u(1))
            },
            &|_| (no_fields(), None),
            &mut counting_mint(),
        );
        assert!(plan.settle.is_empty() && plan.append.is_empty(), "{plan:?}");
        assert_eq!(claims, 0);
    }

    /// The fact pair of a hold intent: the call of `items[0]` and the reserved id `u(7)`.
    fn claim_first(reported: &[ReportedCall]) -> impl FnMut(&CallFacts) -> Option<Uuid> {
        let facts = reported[0].call.clone().expect("call facts");
        let mut left = Some(u(7));
        move |c: &CallFacts| if *c == facts { left.take() } else { None }
    }

    /// Spec R10 revised: the interceptor forwards a timed-out hold under `on_failure = allow` with no
    /// decision id. The report site claims the hold intent and settles the reserved event `allow`:
    /// one event, no append.
    #[test]
    fn a_timed_out_hold_under_allow_settles_its_reserved_event_and_does_not_append() {
        let reported = reported_of(&[("c-1", "Send")], &[(Decision::Allow, None)]);
        let mut claim = claim_first(&reported);
        let plan = settle_plan(&reported, &|id| id == u(7), &mut claim, &|_| (no_fields(), None), &mut counting_mint());
        assert!(plan.append.is_empty(), "{plan:?}");
        assert_eq!(
            plan.settle,
            vec![SettleItem {
                event_id: u(7),
                verdict: EventVerdict::Allow,
                tool: "send".into(),
                call_id: Some("c-1".into()),
                item_id: None
            }]
        );
    }

    /// Spec P9: a block that the interceptor makes itself (a decide timeout under `block`, or a
    /// decider error) settles the reserved event `deny`, with the call id for the link delete.
    #[test]
    fn an_interceptor_block_settles_the_reserved_event_with_deny() {
        let reported = reported_of(&[("c-1", "Send")], &[(Decision::Block { code: BlockCode::DeciderError }, None)]);
        let mut claim = claim_first(&reported);
        let plan = settle_plan(&reported, &|id| id == u(7), &mut claim, &|_| (no_fields(), None), &mut counting_mint());
        assert!(plan.append.is_empty(), "{plan:?}");
        assert_eq!(plan.settle.len(), 1);
        let item = &plan.settle[0];
        assert_eq!((item.event_id, item.verdict, item.call_id.as_deref()), (u(7), EventVerdict::Deny, Some("c-1")));
    }

    /// Spec P9: a denied call whose event was not reserved is appended as `deny`, with its stored
    /// fields, under its decision id. So it counts.
    #[test]
    fn a_denied_call_with_no_reserved_event_is_appended_as_deny() {
        let reported = reported_of(&[("c-1", "rm")], &[(BLOCK, Some(u(2)))]);
        let plan = settle_plan(&reported, &|_| false, &mut |_| None, &|_| (Fields::Unparsed, None), &mut counting_mint());
        assert!(plan.settle.is_empty());
        let mut want = appended(u(2), "rm", EventVerdict::Deny, "c-1");
        want.fields = Fields::Unparsed;
        assert_eq!(plan.append, vec![want]);
    }

    /// No decision id and no hold intent: an append under a new id, with the verdict of the entry.
    #[test]
    fn a_call_with_no_decision_and_no_intent_is_appended_under_a_new_id() {
        let reported = reported_of(&[("c-1", "a"), ("c-2", "b")], &[(Decision::Allow, None), (BLOCK, None)]);
        let plan = settle_plan(&reported, &|_| true, &mut |_| None, &|_| (no_fields(), None), &mut counting_mint());
        assert!(plan.settle.is_empty());
        assert_eq!(plan.append, vec![appended(u(101), "a", EventVerdict::Allow, "c-1"), appended(u(102), "b", EventVerdict::Deny, "c-2")]);
    }

    /// `ToolHistory::settle` claims the hold intent of the session: the reserved event is settled,
    /// and nothing is appended.
    #[tokio::test]
    async fn settle_claims_the_hold_intent_of_a_call_without_a_decision_id() {
        let (store, h) = setup();
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let items = [("c-1", "a")];
        let parsed = calls(&items);
        session.record_intent(vec![(CallFacts::of(&parsed[0]), u(1))]);
        h.reserve(&ctx(), &session, &parsed, &[u(1)], None).await;
        h.settle(&ctx().with_session(session.clone()), &reported_of(&items, &[(Decision::Allow, None)]));
        drain_tasks(&h).await;
        let events = store.inner.events(ORG_A, Scope::Session, &session.session_id.to_string(), "a");
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!((events[0].id, events[0].verdict), (u(1), EventVerdict::Allow));
        assert_eq!(session.claim_intent(&CallFacts::of(&parsed[0])), None, "the intent is claimed");
    }

    /// The stored fields of an appended call are its extracted values, digested with the digester
    /// of the history. With no key, a string is stored as `Mistyped` and a Long as itself.
    #[tokio::test]
    async fn an_appended_call_stores_its_digested_fields() {
        use paigasus_kernel::tool_policy::ToolInput;
        use paigasus_kernel::tool_policy::history::{Field, FieldPath, PathSet, extract_fields};

        let (store, h) = setup();
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let path = |p: &str| FieldPath::new(vec![p.to_owned()]).expect("a path");
        let plan: PathSet = [path("n"), path("s")].into_iter().collect();
        let extracted = extract_fields(&ToolInput::Json(r#"{"n":5,"s":"secret"}"#.to_owned()), &plan);
        let reported: Vec<ReportedCall> = reported_of(&[("c-1", "a")], &[(Decision::Allow, None)])
            .into_iter()
            .map(|r| r.with_fields(Some(extracted.clone())))
            .collect();
        h.settle(&ctx().with_session(session.clone()), &reported);
        drain_tasks(&h).await;
        let events = store.inner.events(ORG_A, Scope::Session, &session.session_id.to_string(), "a");
        let want: BTreeMap<FieldPath, Field> = [(path("n"), Field::Long(5)), (path("s"), Field::Mistyped)].into_iter().collect();
        assert_eq!((&events[0].fields, events[0].key), (&Fields::Values(want), None));
    }

    async fn drain_tasks(h: &ToolHistory) {
        h.tasks().close();
        h.tasks().wait().await;
    }

    #[tokio::test]
    async fn settle_runs_on_the_tracker_and_sets_the_verdict() {
        let (store, h) = setup();
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let items = [("c-1", "a"), ("c-2", "b"), ("c-3", "c")];
        h.reserve(&ctx(), &session, &calls(&items[..2]), &[u(1), u(2)], None).await;
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
        h.reserve(&ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)], None).await;
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
        h.reserve(&ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)], None).await;
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
        h.reserve(&ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)], None).await;
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
        let plan = plan_of(&reported, &|id| id == u(3));
        // The reserved call settles. The unreserved allowed call appends under its decision id. The
        // unreserved denied call appends as deny (spec P9): it counts, and it writes no link.
        assert_eq!(plan.settle.len(), 1);
        assert_eq!(plan.settle[0].event_id, u(3));
        assert_eq!(
            plan.append,
            vec![appended(u(1), "read_file", EventVerdict::Allow, "c-1"), appended(u(2), "rm", EventVerdict::Deny, "c-2")]
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
        h.reserve(&ctx(), &first, &calls(&items), &[u(1), u(2)], None).await;
        assert!(!first.is_reserved(u(1)) && !first.is_reserved(u(2)));
        h.settle(&ctx().with_session(first.clone()), &reported_of(&items, &[(Decision::Allow, Some(u(1))), (BLOCK, Some(u(2)))]));
        h.tasks().close();
        h.tasks().wait().await;
        let sid = first.session_id.to_string();
        let a = store.inner.events(ORG_A, Scope::Session, &sid, "a");
        assert_eq!((a.len(), a[0].id, a[0].verdict), (1, u(1), EventVerdict::Allow));
        let b = store.inner.events(ORG_A, Scope::Session, &sid, "b");
        assert_eq!(
            b.iter().map(|e| (e.id, e.verdict)).collect::<Vec<_>>(),
            [(u(2), EventVerdict::Deny)],
            "a denied call with no reserved event is appended as deny (spec P9)"
        );

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
        h.reserve(&ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)], None).await;
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
        h.reserve(&ctx(), &session, &calls(&[("c-1", "t")]), &[u(1)], None).await;
        h.reserve(&ctx(), &session, &calls(&[("c-2", "t")]), &[u(2)], None).await;
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
        assert_eq!(counter(&snap, names::GATEWAY_TOOL_HISTORY_EVENTS_MISSING_TOTAL, &[]), Some(0));
        for part in ["floor", "tenant"] {
            for reason in ["disabled", "store", "no_session", "truncated", "unknown", "retention", "no_key", "catalog"] {
                assert_eq!(
                    counter(&snap, names::GATEWAY_TOOL_HISTORY_SLOTS_MISSING_TOTAL, &[("part", part), ("reason", reason)]),
                    Some(0),
                    "{part}/{reason}"
                );
            }
        }
        assert_eq!(series, 20 + 3 + 2 + 1 + 16);
    }

    use paigasus_gateway_core::HistoryPolicy;
    use paigasus_kernel::tool_policy::history::{
        Catalog, CmpOp, Condition, Event, EventKind as KernelEventKind, FieldPath, FieldTest, Literal, Part, Predicate, Scope as KernelScope, SetTool, Shape, ToolMatch, Window, WindowSet, evaluate,
    };

    use crate::domain::tool_history::ReadRequest;

    fn tool_is(name: &str) -> Predicate {
        Predicate::Tool(ToolMatch::NameKey(name.into()))
    }

    /// `count_within(tool == name) >= 2` over one minute, in `scope`.
    fn count_of(scope: KernelScope, name: &str) -> Condition {
        Condition::new(
            format!("c_{name}"),
            Part::Tenant,
            scope,
            60_000,
            Shape::CountWithin {
                predicate: tool_is(name),
                op: CmpOp::Ge,
                threshold: 2,
            },
        )
    }

    fn policy_of(conditions: &[Condition]) -> HistoryPolicy {
        HistoryPolicy::build(&[], conditions, ORG_A, Catalog::unknown(), None, &HmacHistoryDigester::keyless())
    }

    fn request_event(id: Uuid, ts_ms: u64, tool: &str) -> Event {
        Event::new(id, ts_ms, KernelEventKind::Request, tool, no_fields(), None)
    }

    fn keyless_ctx() -> DecisionContext {
        DecisionContext::new(PRINCIPAL, ORG_A_PRN, None).with_request(None, "chat", "m")
    }

    fn set(scope: KernelScope, tool: &str, events: Vec<Event>) -> WindowSet {
        WindowSet::new(scope, SetTool::Tool(tool.into()), events, None, false)
    }

    /// The reply holds the pending events of this hold (spec P4); the application removes them, so
    /// the hold counts once, through `hold`.
    #[tokio::test]
    async fn the_hold_own_events_are_removed_from_the_window() {
        let (store, h) = setup();
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let policy = policy_of(&[count_of(KernelScope::Session, "a")]);
        let first = h.reserve(&ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)], Some(&policy)).await;
        assert_eq!(first.hold, vec![request_event(u(1), first.now_ms, "a")]);
        assert_eq!(first.window, WindowRead::Read(Window::new(vec![set(KernelScope::Session, "a", vec![])])));
        let second = h.reserve(&ctx(), &session, &calls(&[("c-2", "a")]), &[u(2)], Some(&policy)).await;
        assert_eq!(
            second.window,
            WindowRead::Read(Window::new(vec![set(KernelScope::Session, "a", vec![request_event(u(1), first.now_ms, "a")])]))
        );
        let read = store.reads.lock().unwrap()[1].clone().expect("a read");
        assert_eq!(read.from_ms, second.now_ms - 60_000, "from = now - w_max");
        assert_eq!(read.sets, vec![(Scope::Session, SetTool::Tool("a".into()))]);
    }

    /// The fixed time of `history`: the clock of every reserve in these tests.
    fn fixed_now_ms() -> u64 {
        u64::try_from(at("2026-11-22T10:00:00Z").duration_since(std::time::UNIX_EPOCH).expect("after the epoch").as_millis()).expect("ms fit")
    }

    /// The mark of a cap trim reaches the kernel window, so a flood of `a` makes a condition on
    /// `a` missing (`Truncated`), also on a reserve that calls only `b` (spec P4, fail closed).
    #[tokio::test]
    async fn a_flood_of_one_tool_gives_its_window_set_the_mark_and_the_slot_is_missing() {
        use paigasus_kernel::tool_policy::history::{Missing, MissingReason};

        use crate::domain::tool_history::MAX_EVENTS_PER_TOOL;

        let (store, h) = setup();
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let flooded_at = fixed_now_ms() - 1_000;
        let flood: Vec<CallEvent> = (0..=MAX_EVENTS_PER_TOOL as u128)
            .map(|i| CallEvent {
                id: u(10_000 + i),
                tool: "a".into(),
                call_id: None,
                item_id: None,
                fields: no_fields(),
                key: None,
            })
            .collect();
        let histories = Histories {
            org: ORG_A,
            principal: PRINCIPAL.into(),
            key: Some(KEY.into()),
            session: session.session_id,
        };
        let reply = store
            .inner
            .reserve(&ReserveRequest {
                histories,
                turn: 1,
                calls: flood,
                now_ms: flooded_at,
                read: None,
            })
            .await
            .expect("the flood");
        assert_eq!(reply.trimmed_cap, 1);
        let policy = policy_of(&[count_of(KernelScope::Session, "a")]);
        let out = h.reserve(&ctx(), &session, &calls(&[("c-1", "b")]), &[u(1)], Some(&policy)).await;
        let kept: Vec<Event> = store.inner.events(ORG_A, Scope::Session, &session.session_id.to_string(), "a").iter().map(kernel_event).collect();
        assert_eq!(kept.len(), MAX_EVENTS_PER_TOOL);
        let window = Window::new(vec![WindowSet::new(KernelScope::Session, SetTool::Tool("a".into()), kept, Some(flooded_at), false)]);
        assert_eq!(out.window, WindowRead::Read(window.clone()), "the set of `a` carries the mark of the trim");
        let e = evaluate(policy.conditions(), &window, &out.hold, 0, out.now_ms, policy.catalog());
        assert_eq!(e.slots.get("c_a"), None);
        assert_eq!(e.missing, vec![Missing::new("c_a", Part::Tenant, MissingReason::Truncated)]);
    }

    /// A read plan of more than 64 sets: each set past 64 comes back truncated in the kernel
    /// window, and its condition is missing (`Truncated`). A set before the limit is evaluated.
    #[tokio::test]
    async fn a_set_past_the_read_set_limit_is_truncated_and_its_slot_is_missing() {
        use paigasus_kernel::tool_policy::history::{Missing, MissingReason};

        use crate::domain::tool_history::MAX_READ_SETS;

        let (_store, h) = setup();
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let conditions: Vec<Condition> = (0..=MAX_READ_SETS).map(|i| count_of(KernelScope::Session, &format!("t{i}"))).collect();
        let policy = policy_of(&conditions);
        let plan = policy.read().expect("a read plan");
        assert_eq!(plan.sets.len(), MAX_READ_SETS + 1);
        let out = h.reserve(&ctx(), &session, &calls(&[("c-1", "z")]), &[u(1)], Some(&policy)).await;
        let want: Vec<WindowSet> = plan
            .sets
            .iter()
            .enumerate()
            .map(|(i, s)| WindowSet::new(s.scope, s.tool.clone(), vec![], None, i >= MAX_READ_SETS))
            .collect();
        let window = Window::new(want);
        assert_eq!(out.window, WindowRead::Read(window.clone()));
        let slot_of = |i: usize| match &plan.sets[i].tool {
            SetTool::Tool(name) => format!("c_{name}"),
            other => panic!("a named set, not {other:?}"),
        };
        let e = evaluate(policy.conditions(), &window, &out.hold, 0, out.now_ms, policy.catalog());
        assert_eq!(e.missing, vec![Missing::new(slot_of(MAX_READ_SETS), Part::Tenant, MissingReason::Truncated)]);
        assert_eq!(e.slots.get(&slot_of(0)), Some(false), "a set before the limit is read");
    }

    /// F14: with no API key, a `Key` condition reads the principal set. One store set serves the
    /// `Principal` and the `Key` condition, and the window holds both kernel sets.
    #[tokio::test]
    async fn a_key_scope_without_an_api_key_reads_the_principal_sets() {
        let (store, h) = setup();
        let session = h.link(PRINCIPAL, ORG_A, None, &turn(&[], &[])).await.expect("a session").session;
        let mut by_key = count_of(KernelScope::Key, "a");
        by_key.slot = "k_a".into();
        let policy = policy_of(&[by_key, count_of(KernelScope::Principal, "a")]);
        let first = h.reserve(&keyless_ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)], Some(&policy)).await;
        let second = h.reserve(&keyless_ctx(), &session, &calls(&[("c-2", "a")]), &[u(2)], Some(&policy)).await;
        let prior = || vec![request_event(u(1), first.now_ms, "a")];
        let window = Window::new(vec![set(KernelScope::Principal, "a", prior()), set(KernelScope::Key, "a", prior())]);
        assert_eq!(second.window, WindowRead::Read(window.clone()));
        let read = store.reads.lock().unwrap()[1].clone().expect("a read");
        assert_eq!(read.sets, vec![(Scope::Principal, SetTool::Tool("a".into()))], "one store set, no key set");
        let e = evaluate(policy.conditions(), &window, &second.hold, 0, second.now_ms, policy.catalog());
        assert_eq!((e.slots.get("k_a"), e.slots.get("c_a")), (Some(true), Some(true)), "{e:?}");
    }

    /// The reserve stores the fields of the field plan (spec P3), with the helper of the report
    /// site, and the hold events carry them too.
    #[tokio::test]
    async fn the_reserve_stores_the_fields_of_the_field_plan() {
        use paigasus_kernel::tool_policy::history::Field;

        let (store, h) = setup();
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let n = FieldPath::new(vec!["n".to_owned()]).expect("a path");
        let predicate = Predicate::AllOf(Box::new(tool_is("a")), Box::new(Predicate::Field(FieldTest::new(n.clone(), CmpOp::Eq, Literal::Long(5)))));
        let policy = policy_of(&[Condition::new("f", Part::Tenant, KernelScope::Session, 60_000, Shape::Formerly { predicate })]);
        let body = serde_json::json!({"object": "chat.completion", "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [
            {"id": "c-1", "type": "function", "function": {"name": "a", "arguments": "{\"n\":5,\"m\":7}"}}
        ]}, "finish_reason": "tool_calls"}]});
        let parsed = parse_body(Format::OpenAiChat, body.to_string().as_bytes(), 1 << 20).expect("body");
        let out = h.reserve(&ctx(), &session, &parsed.calls, &[u(1)], Some(&policy)).await;
        let want = Fields::Values([(n, Field::Long(5))].into_iter().collect());
        assert_eq!(out.hold[0].fields, want, "only the path of the plan");
        let stored = store.inner.events(ORG_A, Scope::Session, &session.session_id.to_string(), "a");
        assert_eq!((&stored[0].fields, stored[0].key), (&want, None));
    }

    #[tokio::test]
    async fn a_reserve_error_gives_a_failed_window() {
        let store = Fake::new(Mode::FailReserve);
        let h = history(&store, Duration::from_secs(1));
        let policy = policy_of(&[count_of(KernelScope::Session, "a")]);
        let session = Arc::new(SessionContext::new(ORG_A, u(5), 1, Findings::new(), StoreBudget::new(Duration::from_secs(1))));
        let out = h.reserve(&ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)], Some(&policy)).await;
        assert_eq!(out.window, WindowRead::Failed);
        assert_eq!(out.hold.len(), 1, "the hold events do not need the store");
        // The failed budget skips the next reserve: still a failed window.
        let out = h.reserve(&ctx(), &session, &calls(&[("c-2", "a")]), &[u(2)], Some(&policy)).await;
        assert_eq!(out.window, WindowRead::Failed);
        // No read plan: nothing was needed.
        let out = h.reserve(&ctx(), &session, &calls(&[("c-3", "a")]), &[u(3)], None).await;
        assert_eq!(out.window, WindowRead::NotNeeded);
    }

    #[tokio::test(start_paused = true)]
    async fn a_reserve_past_the_deadline_gives_a_failed_window() {
        let store = Fake::new(Mode::Sleep(Duration::from_secs(2)));
        let h = history(&store, Duration::from_secs(1));
        let policy = policy_of(&[count_of(KernelScope::Session, "a")]);
        let session = Arc::new(SessionContext::new(ORG_A, u(5), 1, Findings::new(), StoreBudget::new(Duration::from_secs(1))));
        let out = h.reserve(&ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)], Some(&policy)).await;
        assert_eq!(out.window, WindowRead::Failed);
    }

    #[tokio::test]
    async fn no_read_plan_reads_nothing() {
        let (store, h) = setup();
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let none = h.reserve(&ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)], None).await;
        let empty = policy_of(&[]);
        assert!(empty.read().is_none());
        let no_conditions = h.reserve(&ctx(), &session, &calls(&[("c-2", "a")]), &[u(2)], Some(&empty)).await;
        assert_eq!((none.window, no_conditions.window), (WindowRead::NotNeeded, WindowRead::NotNeeded));
        assert_eq!(*store.reads.lock().unwrap(), vec![None, None], "no reserve reads a set");
    }

    /// A store adapter that leaves out a requested set gives a decode error (spec P4): a failed
    /// window, never an empty set.
    #[tokio::test]
    async fn a_reply_without_the_requested_set_is_a_reserve_decode_error() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        let (store, h) = setup();
        store.reserve_reply.lock().unwrap().push_back(AppendReply {
            written: 1,
            trimmed_retention: 0,
            trimmed_cap: 0,
            truncated: Vec::new(),
            window: Vec::new(),
            missing_bodies: 0,
        });
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        let policy = policy_of(&[count_of(KernelScope::Session, "a")]);
        let out = h.reserve(&ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)], Some(&policy)).await;
        assert_eq!(out.window, WindowRead::Failed);
        assert_eq!(errors(&snap, "reserve", "decode"), Some(1));
    }

    #[tokio::test]
    async fn the_missing_body_metric_is_primed_and_counts() {
        let (rec, snap) = metrics();
        let _local = metrics::set_default_local_recorder(&rec);
        prime_tool_history_metrics();
        let (store, h) = setup();
        store.reserve_reply.lock().unwrap().push_back(AppendReply {
            written: 1,
            trimmed_retention: 0,
            trimmed_cap: 0,
            truncated: Vec::new(),
            window: Vec::new(),
            missing_bodies: 3,
        });
        let session = link(&h, &turn(&[], &[])).await.expect("a session").session;
        h.reserve(&ctx(), &session, &calls(&[("c-1", "a")]), &[u(1)], None).await;
        assert_eq!(counter(&snap, names::GATEWAY_TOOL_HISTORY_EVENTS_MISSING_TOTAL, &[]), Some(3));
    }
}
