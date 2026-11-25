// SPDX-License-Identifier: Apache-2.0

//! The session history store contract suite (SMA-753). One set of cases that BOTH adapters must
//! pass, through the port only: `tests/history_store_memory.rs` runs it on the memory store and
//! the Redis binary runs it on a real Redis. Every case takes its own ids, so cases can share one
//! Redis. Every instant is fixed; nothing sleeps.
//!
//! Counts: `written` counts events, not rows. One call written to three histories is 1.
//! `trimmed_retention` and `trimmed_cap` count distinct events that this call removed (within one call,
//! an event cut from two histories counts once).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use paigasus_gateway::domain::tool_history::{
    AppendCall, AppendRequest, CallEvent, CallLink, EventKind, EventVerdict, Histories, LookupRequest, MAX_EVENTS_PER_TOOL, RETENTION_MS, ReserveRequest, ResultEvent, Scope, SessionState,
    SessionStore, SettleItem, SettleRequest, StoredEvent, WriteRequest,
};
use paigasus_kernel::tool_policy::history::{Digest, Field, FieldPath, Fields, KeyId};
use uuid::Uuid;

/// How a case reaches one backend.
#[async_trait::async_trait]
pub trait Harness: Send + Sync {
    /// Redis: a new store per call (one more replica). Memory: the one shared store.
    async fn store(&self) -> Arc<dyn SessionStore>;
    async fn events(&self, org: Uuid, scope: Scope, scope_id: &str, tool: &str) -> Vec<StoredEvent>;
    async fn ttls_ms(&self, org: Uuid, now_ms: u64) -> Vec<Option<u64>>;
    /// Replace the JSON body of one stored event (a body that another version wrote). The verdict
    /// and the result guard stay.
    async fn put_raw_body(&self, org: Uuid, id: Uuid, body: &str);
}

const NOW: u64 = 1_700_000_000_000;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_n(n: u64) -> u64 {
    NEXT_ID.fetch_add(n, Ordering::SeqCst)
}

/// An id that no other case uses. A counter, not a random UUID: the gateway's `uuid` dependency
/// has no `v4`/`v7` feature. Increasing counters give increasing ids.
fn fresh_id() -> Uuid {
    Uuid::from_u128((0x0190_a1e5u128 << 96) | u128::from(next_n(1)))
}

fn fresh_ids(n: usize) -> Vec<Uuid> {
    let first = next_n(n as u64);
    (0..n as u64).map(|i| Uuid::from_u128((0x0190_a1e5u128 << 96) | u128::from(first + i))).collect()
}

struct Ctx {
    org: Uuid,
    principal: String,
    key: Option<String>,
    session: Uuid,
}

impl Ctx {
    fn new(with_key: bool) -> Self {
        let n = fresh_id();
        Ctx {
            org: fresh_id(),
            principal: format!("prn:pgs:iam:::principal/{n}"),
            key: with_key.then(|| format!("key-{n}")),
            session: fresh_id(),
        }
    }

    /// The same caller in a second session.
    fn other_session(&self) -> Ctx {
        Ctx {
            org: self.org,
            principal: self.principal.clone(),
            key: self.key.clone(),
            session: fresh_id(),
        }
    }

    fn histories(&self) -> Histories {
        Histories {
            org: self.org,
            principal: self.principal.clone(),
            key: self.key.clone(),
            session: self.session,
        }
    }

    fn lookup(&self, call_ids: &[&str], result_call_ids: &[&str], refs: &[Uuid]) -> LookupRequest {
        LookupRequest {
            org: self.org,
            principal: self.principal.clone(),
            call_ids: call_ids.iter().map(|s| s.to_string()).collect(),
            result_call_ids: result_call_ids.iter().map(|s| s.to_string()).collect(),
            refs: refs.to_vec(),
            now_ms: NOW,
        }
    }

    fn reserve(&self, calls: Vec<CallEvent>, now_ms: u64, window_from_ms: Option<u64>) -> ReserveRequest {
        self.reserve_turn(1, calls, now_ms, window_from_ms)
    }

    fn reserve_turn(&self, turn: i64, calls: Vec<CallEvent>, now_ms: u64, window_from_ms: Option<u64>) -> ReserveRequest {
        ReserveRequest {
            histories: self.histories(),
            turn,
            calls,
            now_ms,
            window_from_ms,
        }
    }

    fn settle(&self, items: Vec<SettleItem>, now_ms: u64) -> SettleRequest {
        SettleRequest {
            org: self.org,
            principal: self.principal.clone(),
            session: self.session,
            items,
            now_ms,
        }
    }

    fn write(&self, turn: Option<i64>, results: Vec<ResultEvent>, now_ms: u64) -> WriteRequest {
        WriteRequest {
            histories: self.histories(),
            turn,
            results,
            now_ms,
        }
    }
}

fn no_fields() -> Fields {
    Fields::Values(BTreeMap::new())
}

fn path(text: &str) -> FieldPath {
    FieldPath::new(text.split('.').map(str::to_string).collect()).expect("a valid path")
}

/// One field of each stored kind, and a key id. The Long is past 2^53, so a store that reads a
/// number as a float fails.
fn sample_fields() -> (Fields, Option<KeyId>) {
    let mut m = BTreeMap::new();
    m.insert(path("amount"), Field::Long(-9_007_199_254_740_993));
    m.insert(path("to"), Field::Digest(Digest::new([0xfb; 16])));
    m.insert(path("cc"), Field::Absent);
    m.insert(path("opts.mode"), Field::Mistyped);
    (Fields::Values(m), Some(KeyId::new([1, 2, 3, 4, 5, 6, 7, 0xff])))
}

fn call(tool: &str) -> CallEvent {
    CallEvent {
        id: fresh_id(),
        tool: tool.into(),
        call_id: None,
        item_id: None,
        fields: no_fields(),
        key: None,
    }
}

/// A call with its client call id.
fn call_as(tool: &str, call_id: &str) -> CallEvent {
    CallEvent {
        call_id: Some(call_id.into()),
        ..call(tool)
    }
}

fn calls(tool: &str, n: usize) -> Vec<CallEvent> {
    fresh_ids(n).into_iter().map(|id| CallEvent { id, ..call(tool) }).collect()
}

fn append_call(id: Uuid, tool: &str, verdict: EventVerdict, call_id: Option<&str>, item_id: Option<&str>) -> AppendCall {
    AppendCall {
        id,
        tool: tool.into(),
        verdict,
        call_id: call_id.map(str::to_string),
        item_id: item_id.map(str::to_string),
        fields: no_fields(),
        key: None,
    }
}

fn append_req(ctx: &Ctx, calls: Vec<AppendCall>, now_ms: u64) -> AppendRequest {
    AppendRequest {
        histories: ctx.histories(),
        turn: 1,
        calls,
        now_ms,
    }
}

fn item(event_id: Uuid, verdict: EventVerdict, tool: &str, call_id: Option<&str>) -> SettleItem {
    SettleItem {
        event_id,
        verdict,
        tool: tool.into(),
        call_id: call_id.map(str::to_string),
        item_id: None,
    }
}

fn link(ctx: &Ctx, tool: &str, event_id: Uuid, result_written: bool) -> CallLink {
    CallLink {
        session: ctx.session,
        tool: tool.into(),
        event_id,
        result_written,
    }
}

/// Set-up only: reserve many calls in chunks of `FILL_CHUNK`, all at `now_ms`. A real request
/// carries a few calls; one 10,000-call reserve is a Redis request of several MB that went past
/// the 500 ms response budget of the shared client under the parallel load of the suite
/// (measured). The stored state is the same as for one reserve, and no case asserts on the reply
/// of a set-up reserve.
async fn fill(store: &Arc<dyn SessionStore>, ctx: &Ctx, calls: &[CallEvent], now_ms: u64) {
    for chunk in calls.chunks(FILL_CHUNK) {
        store.reserve(&ctx.reserve(chunk.to_vec(), now_ms, None)).await.unwrap();
    }
}

const FILL_CHUNK: usize = 1_000;

async fn principal_events(h: &impl Harness, ctx: &Ctx, tool: &str) -> Vec<StoredEvent> {
    h.events(ctx.org, Scope::Principal, &ctx.principal, tool).await
}

pub async fn a_reserve_appends_one_pending_event_per_history(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("send");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    let key = ctx.key.clone().unwrap();
    let session = ctx.session.to_string();
    for (scope, id) in [(Scope::Session, session.as_str()), (Scope::Principal, ctx.principal.as_str()), (Scope::Key, key.as_str())] {
        let events = h.events(ctx.org, scope, id, "send").await;
        assert_eq!(events.len(), 1, "{scope:?}");
        let e = &events[0];
        assert_eq!(
            (e.id, e.verdict, e.kind, e.ts_ms, e.tool.as_str()),
            (c.id, EventVerdict::Pending, EventKind::Call, NOW, "send"),
            "{scope:?}"
        );
    }
}

pub async fn a_caller_without_a_key_writes_the_principal_history_once(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(false), h.store().await);
    store.reserve(&ctx.reserve(vec![call("send")], NOW, None)).await.unwrap();
    assert_eq!(principal_events(h, &ctx, "send").await.len(), 1);
    assert!(h.events(ctx.org, Scope::Key, &ctx.principal, "send").await.is_empty());
    assert_eq!(h.events(ctx.org, Scope::Session, &ctx.session.to_string(), "send").await.len(), 1);
}

pub async fn the_split_by_tool_keeps_other_tools(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(false), h.store().await);
    store.reserve(&ctx.reserve(vec![call("read")], NOW - 1, None)).await.unwrap();
    fill(&store, &ctx, &calls("send", MAX_EVENTS_PER_TOOL), NOW - 1).await;
    store.reserve(&ctx.reserve(calls("send", 1), NOW, None)).await.unwrap();
    assert_eq!(principal_events(h, &ctx, "read").await.len(), 1);
    assert_eq!(principal_events(h, &ctx, "send").await.len(), MAX_EVENTS_PER_TOOL);
}

pub async fn retention_trims_only_events_older_than_25_hours(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(false), h.store().await);
    let (gone, stays) = (call("send"), call("send"));
    store.reserve(&ctx.reserve(vec![gone.clone()], NOW - RETENTION_MS - 1, None)).await.unwrap();
    store.reserve(&ctx.reserve(vec![stays.clone()], NOW - RETENTION_MS, None)).await.unwrap();
    let reply = store.reserve(&ctx.reserve(vec![call("send")], NOW, None)).await.unwrap();
    assert_eq!(reply.trimmed_retention, 1);
    assert_eq!(reply.trimmed_cap, 0);
    let ids: Vec<Uuid> = principal_events(h, &ctx, "send").await.iter().map(|e| e.id).collect();
    assert!(ids.contains(&stays.id));
    assert!(!ids.contains(&gone.id));
    assert_eq!(ids.len(), 2);
}

pub async fn the_cap_keeps_the_newest_10_000_events_per_tool(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(false), h.store().await);
    let old = calls("send", MAX_EVENTS_PER_TOOL);
    fill(&store, &ctx, &old, NOW - 1).await;
    let reply = store.reserve(&ctx.reserve(calls("send", 1), NOW, None)).await.unwrap();
    assert_eq!((reply.trimmed_cap, reply.trimmed_retention), (1, 0));
    let events = principal_events(h, &ctx, "send").await;
    assert_eq!(events.len(), MAX_EVENTS_PER_TOOL);
    let oldest = old.iter().map(|c| c.id).min().unwrap();
    assert!(events.iter().all(|e| e.id != oldest), "the oldest event went");
    assert_eq!(events.last().unwrap().ts_ms, NOW);
}

pub async fn the_truncated_flag_needs_a_lost_event_inside_the_window(h: &impl Harness) {
    let store = h.store().await;
    for (window_from, flagged) in [(Some(NOW - 100), true), (Some(NOW - 99), false), (None, false)] {
        let ctx = Ctx::new(false);
        fill(&store, &ctx, &calls("send", MAX_EVENTS_PER_TOOL), NOW - 100).await;
        let reply = store.reserve(&ctx.reserve(calls("send", 1), NOW, window_from)).await.unwrap();
        assert_eq!(reply.trimmed_cap, 1, "{window_from:?}");
        if flagged {
            assert!(reply.truncated.contains(&(Scope::Session, "send".to_string())), "{:?}", reply.truncated);
            assert!(reply.truncated.contains(&(Scope::Principal, "send".to_string())), "{:?}", reply.truncated);
        } else {
            assert!(reply.truncated.is_empty(), "{window_from:?}: {:?}", reply.truncated);
        }
    }
}

pub async fn settle_sets_the_verdict_and_writes_the_call_link(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("send");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    store.settle(&ctx.settle(vec![item(c.id, EventVerdict::Allow, "send", Some("call-1"))], NOW)).await.unwrap();
    assert_eq!(principal_events(h, &ctx, "send").await[0].verdict, EventVerdict::Allow);
    let reply = store.lookup(&ctx.lookup(&["call-1"], &[], &[])).await.unwrap();
    assert_eq!(reply.calls, vec![Some(link(&ctx, "send", c.id, false))]);
}

pub async fn settle_twice_is_idempotent(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("send");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    let req = ctx.settle(vec![item(c.id, EventVerdict::Allow, "send", Some("call-1"))], NOW);
    store.settle(&req).await.unwrap();
    let events = principal_events(h, &ctx, "send").await;
    let reply = store.lookup(&ctx.lookup(&["call-1"], &[], &[])).await.unwrap();
    store.settle(&req).await.unwrap();
    assert_eq!(principal_events(h, &ctx, "send").await, events);
    assert_eq!(store.lookup(&ctx.lookup(&["call-1"], &[], &[])).await.unwrap(), reply);
}

pub async fn a_deny_settle_writes_no_call_link(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("send");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    store.settle(&ctx.settle(vec![item(c.id, EventVerdict::Deny, "send", Some("call-1"))], NOW)).await.unwrap();
    assert_eq!(principal_events(h, &ctx, "send").await[0].verdict, EventVerdict::Deny);
    assert_eq!(store.lookup(&ctx.lookup(&["call-1"], &[], &[])).await.unwrap().calls, vec![None]);
}

pub async fn a_lost_settle_leaves_the_event_pending_and_no_call_link(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    store.reserve(&ctx.reserve(vec![call("send")], NOW, None)).await.unwrap();
    assert_eq!(principal_events(h, &ctx, "send").await[0].verdict, EventVerdict::Pending);
    assert_eq!(store.lookup(&ctx.lookup(&["call-1"], &[], &[])).await.unwrap().calls, vec![None]);
}

pub async fn a_settle_for_an_unknown_event_writes_nothing(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let unknown = fresh_id();
    store.settle(&ctx.settle(vec![item(unknown, EventVerdict::Allow, "send", Some("call-1"))], NOW)).await.unwrap();
    assert!(principal_events(h, &ctx, "send").await.is_empty());
    assert_eq!(store.lookup(&ctx.lookup(&["call-1"], &[], &[])).await.unwrap().calls, vec![None]);
    assert!(h.ttls_ms(ctx.org, NOW).await.is_empty());
}

pub async fn the_reserve_writes_decision_links(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("send");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    let other = fresh_id();
    let reply = store.lookup(&ctx.lookup(&[], &[], &[c.id, other])).await.unwrap();
    assert_eq!(reply.refs, vec![Some(ctx.session), None]);
}

pub async fn links_are_isolated_per_principal_and_org(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("send");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    store.settle(&ctx.settle(vec![item(c.id, EventVerdict::Allow, "send", Some("call-1"))], NOW)).await.unwrap();
    let own = store.lookup(&ctx.lookup(&["call-1"], &["call-1"], &[c.id])).await.unwrap();
    assert!(own.calls[0].is_some() && own.results[0].is_some() && own.refs[0].is_some());

    let mut other_principal = ctx.lookup(&["call-1"], &["call-1"], &[c.id]);
    other_principal.principal = format!("{}-b", ctx.principal);
    let reply = store.lookup(&other_principal).await.unwrap();
    assert_eq!((reply.calls, reply.results, reply.refs), (vec![None], vec![None], vec![None]));

    let mut other_org = ctx.lookup(&["call-1"], &["call-1"], &[c.id]);
    other_org.org = fresh_id();
    let reply = store.lookup(&other_org).await.unwrap();
    assert_eq!((reply.calls, reply.results, reply.refs), (vec![None], vec![None], vec![None]));
}

/// The guard is on the linked call event: a second result for the same call id writes nothing.
pub async fn a_result_event_is_written_once_per_call_id(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("send");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    store.settle(&ctx.settle(vec![item(c.id, EventVerdict::Allow, "send", Some("call-1"))], NOW)).await.unwrap();
    let result = |id| ResultEvent {
        id,
        call_id: "call-1".into(),
        tool: "send".into(),
        event_id: c.id,
    };
    let first = store.write(&ctx.write(Some(2), vec![result(fresh_id())], NOW + 1)).await.unwrap();
    let second = store.write(&ctx.write(Some(2), vec![result(fresh_id())], NOW + 2)).await.unwrap();
    assert_eq!((first.written, second.written), (1, 0));
    let results: Vec<_> = principal_events(h, &ctx, "send").await.into_iter().filter(|e| e.kind == EventKind::Result).collect();
    assert_eq!(results.len(), 1);
    let reply = store.lookup(&ctx.lookup(&[], &["call-1"], &[])).await.unwrap();
    assert!(reply.results[0].as_ref().unwrap().result_written);
}

pub async fn a_write_without_results_and_without_turn_writes_nothing(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let reply = store.write(&ctx.write(None, vec![], NOW)).await.unwrap();
    assert_eq!(reply.written, 0);
    assert!(h.ttls_ms(ctx.org, NOW).await.is_empty());
}

pub async fn touch_keeps_the_latest_activity_and_the_highest_turn(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("send");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW - 1000, None)).await.unwrap();
    store.settle(&ctx.settle(vec![item(c.id, EventVerdict::Allow, "send", Some("call-1"))], NOW - 1000)).await.unwrap();
    store.write(&ctx.write(Some(5), vec![], NOW)).await.unwrap();
    store.write(&ctx.write(Some(3), vec![], NOW - 50)).await.unwrap();
    let reply = store.lookup(&ctx.lookup(&["call-1"], &[], &[])).await.unwrap();
    assert_eq!(
        reply.sessions,
        vec![SessionState {
            id: ctx.session,
            last_ms: NOW,
            turn: 5
        }]
    );
}

pub async fn a_future_event_survives_a_reserve_with_an_older_clock(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(false), h.store().await);
    let future = call("send");
    store.reserve(&ctx.reserve(vec![future.clone()], NOW + 50, None)).await.unwrap();
    store.reserve(&ctx.reserve(vec![call("send")], NOW, None)).await.unwrap();
    let ids: Vec<Uuid> = principal_events(h, &ctx, "send").await.iter().map(|e| e.id).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&future.id));
}

pub async fn every_written_key_expires_after_the_retention(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("send");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    store.settle(&ctx.settle(vec![item(c.id, EventVerdict::Allow, "send", Some("call-1"))], NOW)).await.unwrap();
    let result = ResultEvent {
        id: fresh_id(),
        call_id: "call-1".into(),
        tool: "send".into(),
        event_id: c.id,
    };
    store.write(&ctx.write(Some(2), vec![result], NOW)).await.unwrap();
    let ttls = h.ttls_ms(ctx.org, NOW).await;
    assert!(!ttls.is_empty());
    for ttl in ttls {
        let ttl = ttl.expect("every key has a time to live");
        assert!(ttl > RETENTION_MS - 60_000 && ttl <= RETENTION_MS, "{ttl}");
    }
}

pub async fn append_writes_an_allowed_event_and_its_call_link(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let id = fresh_id();
    let req = append_req(&ctx, vec![append_call(id, "send", EventVerdict::Allow, Some("call-1"), None)], NOW);
    let reply = store.append(&req).await.unwrap();
    assert_eq!(reply.written, 1);
    assert_eq!(principal_events(h, &ctx, "send").await[0].verdict, EventVerdict::Allow);
    let found = store.lookup(&ctx.lookup(&["call-1"], &[], &[])).await.unwrap();
    assert_eq!(found.calls, vec![Some(link(&ctx, "send", id, false))]);
}

pub async fn a_local_shell_settle_links_the_item_id_too(h: &impl Harness) {
    // By settle.
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("shell");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    let mut it = item(c.id, EventVerdict::Allow, "shell", Some("call-1"));
    it.item_id = Some("item-1".into());
    store.settle(&ctx.settle(vec![it], NOW)).await.unwrap();
    let expected = Some(link(&ctx, "shell", c.id, false));
    let reply = store.lookup(&ctx.lookup(&["call-1"], &["item-1"], &[])).await.unwrap();
    assert_eq!((reply.calls, reply.results), (vec![expected.clone()], vec![expected.clone()]));
    let reply = store.lookup(&ctx.lookup(&["item-1"], &["call-1"], &[])).await.unwrap();
    assert_eq!((reply.calls, reply.results), (vec![expected.clone()], vec![expected]));

    // By append.
    let ctx = Ctx::new(true);
    let id = fresh_id();
    let req = append_req(&ctx, vec![append_call(id, "shell", EventVerdict::Allow, Some("call-2"), Some("item-2"))], NOW);
    store.append(&req).await.unwrap();
    let expected = Some(link(&ctx, "shell", id, false));
    let reply = store.lookup(&ctx.lookup(&[], &["item-2", "call-2"], &[])).await.unwrap();
    assert_eq!(reply.results, vec![expected.clone(), expected]);
}

pub async fn a_cap_trim_in_the_principal_history_keeps_the_event_in_its_session_history(h: &impl Harness) {
    let (first, store) = (Ctx::new(false), h.store().await);
    let second = first.other_session();
    let old = calls("send", MAX_EVENTS_PER_TOOL);
    fill(&store, &first, &old, NOW - 1).await;
    let reply = store.reserve(&second.reserve(calls("send", 1), NOW, None)).await.unwrap();
    assert_eq!(reply.trimmed_cap, 1);
    let oldest = old.iter().map(|c| c.id).min().unwrap();
    assert!(principal_events(h, &first, "send").await.iter().all(|e| e.id != oldest));
    let in_session = h.events(first.org, Scope::Session, &first.session.to_string(), "send").await;
    assert_eq!(in_session.len(), MAX_EVENTS_PER_TOOL);
    let kept = in_session.iter().find(|e| e.id == oldest).expect("the event stays in its session history");
    assert_eq!(kept.verdict, EventVerdict::Pending);
}

pub async fn a_cut_of_two_flagged_events_gives_one_truncated_entry_per_scope_and_tool(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(false), h.store().await);
    fill(&store, &ctx, &calls("send", MAX_EVENTS_PER_TOOL), NOW - 100).await;
    let reply = store.reserve(&ctx.reserve(calls("send", 2), NOW, Some(NOW - 100))).await.unwrap();
    assert_eq!(reply.trimmed_cap, 2);
    let count = |scope: Scope| reply.truncated.iter().filter(|t| **t == (scope, "send".to_string())).count();
    assert_eq!((count(Scope::Session), count(Scope::Principal), count(Scope::Key)), (1, 1, 0), "{:?}", reply.truncated);
}

pub async fn a_reserve_on_a_new_session_writes_its_state(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("send");
    store.reserve(&ctx.reserve_turn(4, vec![c.clone()], NOW, None)).await.unwrap();
    let reply = store.lookup(&ctx.lookup(&[], &[], &[c.id])).await.unwrap();
    assert_eq!(
        reply.sessions,
        vec![SessionState {
            id: ctx.session,
            last_ms: NOW,
            turn: 4
        }]
    );
}

pub async fn a_reserve_with_a_lower_turn_does_not_lower_the_turn(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let (first, second) = (call("send"), call("send"));
    store.reserve(&ctx.reserve_turn(7, vec![first.clone()], NOW, None)).await.unwrap();
    store.reserve(&ctx.reserve_turn(3, vec![second.clone()], NOW + 10, None)).await.unwrap();
    let reply = store.lookup(&ctx.lookup(&[], &[], &[first.id])).await.unwrap();
    assert_eq!(
        reply.sessions,
        vec![SessionState {
            id: ctx.session,
            last_ms: NOW + 10,
            turn: 7
        }]
    );
}

pub async fn an_append_on_a_new_session_writes_its_state(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let mut req = append_req(&ctx, vec![append_call(fresh_id(), "send", EventVerdict::Allow, Some("call-1"), None)], NOW);
    req.turn = 6;
    store.append(&req).await.unwrap();
    let reply = store.lookup(&ctx.lookup(&["call-1"], &[], &[])).await.unwrap();
    assert_eq!(
        reply.sessions,
        vec![SessionState {
            id: ctx.session,
            last_ms: NOW,
            turn: 6
        }]
    );
}

pub async fn a_second_allow_settle_of_a_call_id_relinks_it_to_the_new_event(h: &impl Harness) {
    let (first, store) = (Ctx::new(true), h.store().await);
    let second = first.other_session();
    let (a, b) = (call("send"), call("read"));
    store.reserve(&first.reserve(vec![a.clone()], NOW, None)).await.unwrap();
    store.settle(&first.settle(vec![item(a.id, EventVerdict::Allow, "send", Some("call-1"))], NOW)).await.unwrap();
    let result = ResultEvent {
        id: fresh_id(),
        call_id: "call-1".into(),
        tool: "send".into(),
        event_id: a.id,
    };
    store.write(&first.write(None, vec![result], NOW)).await.unwrap();
    let before = store.lookup(&first.lookup(&["call-1"], &[], &[])).await.unwrap();
    assert!(before.calls[0].as_ref().unwrap().result_written);

    store.reserve(&second.reserve(vec![b.clone()], NOW + 1, None)).await.unwrap();
    store.settle(&second.settle(vec![item(b.id, EventVerdict::Allow, "read", Some("call-1"))], NOW + 1)).await.unwrap();
    // The new event has no result yet: the flag is the guard of the new event (T7-b).
    let after = store.lookup(&first.lookup(&["call-1"], &[], &[])).await.unwrap();
    assert_eq!(after.calls, vec![Some(link(&second, "read", b.id, false))]);
}

pub async fn the_first_settle_wins(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("send");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    store.settle(&ctx.settle(vec![item(c.id, EventVerdict::Allow, "send", Some("call-1"))], NOW)).await.unwrap();
    store.settle(&ctx.settle(vec![item(c.id, EventVerdict::Deny, "send", Some("call-1"))], NOW + 1)).await.unwrap();
    assert_eq!(principal_events(h, &ctx, "send").await[0].verdict, EventVerdict::Allow);
}

pub async fn a_repeated_settle_of_the_same_event_keeps_result_written(h: &impl Harness) {
    // Ruling T7-f: a settle retry for the same event must not re-open a written result.
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("send");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    let settle = ctx.settle(vec![item(c.id, EventVerdict::Allow, "send", Some("call-1"))], NOW);
    store.settle(&settle).await.unwrap();
    let result = ResultEvent {
        id: fresh_id(),
        call_id: "call-1".into(),
        tool: "send".into(),
        event_id: c.id,
    };
    store.write(&ctx.write(None, vec![result], NOW + 1)).await.unwrap();
    store.settle(&settle).await.unwrap();
    let reply = store.lookup(&ctx.lookup(&["call-1"], &[], &[])).await.unwrap();
    assert_eq!(reply.calls, vec![Some(link(&ctx, "send", c.id, true))]);
}

pub async fn a_result_for_a_call_event_that_is_gone_writes_nothing(h: &impl Harness) {
    // Ruling F5: a call event that is gone (expired between lookup and write) gets no result
    // event, and the guard makes no event key with no fields and no time to live.
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let result = ResultEvent {
        id: fresh_id(),
        call_id: "call-gone".into(),
        tool: "send".into(),
        event_id: fresh_id(),
    };
    let reply = store.write(&ctx.write(None, vec![result], NOW)).await.unwrap();
    assert_eq!(reply.written, 0);
    assert!(principal_events(h, &ctx, "send").await.is_empty());
    assert!(h.ttls_ms(ctx.org, NOW).await.is_empty());
}

pub async fn a_later_allow_settle_of_a_denied_event_writes_no_call_link(h: &impl Harness) {
    // The first settle wins the verdict (T7-c), and the link follows the stored verdict.
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("send");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    store.settle(&ctx.settle(vec![item(c.id, EventVerdict::Deny, "send", Some("call-1"))], NOW)).await.unwrap();
    store.settle(&ctx.settle(vec![item(c.id, EventVerdict::Allow, "send", Some("call-1"))], NOW + 1)).await.unwrap();
    assert_eq!(principal_events(h, &ctx, "send").await[0].verdict, EventVerdict::Deny);
    assert_eq!(store.lookup(&ctx.lookup(&["call-1"], &["call-1"], &[])).await.unwrap().calls, vec![None]);
}

pub async fn a_local_shell_result_under_both_aliases_is_written_once(h: &impl Harness) {
    // The two aliases (call id and item id) link to ONE call event, so they share one guard.
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("shell");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    let mut it = item(c.id, EventVerdict::Allow, "shell", Some("call-1"));
    it.item_id = Some("item-1".into());
    store.settle(&ctx.settle(vec![it], NOW)).await.unwrap();
    let result = |alias: &str| ResultEvent {
        id: fresh_id(),
        call_id: alias.into(),
        tool: "shell".into(),
        event_id: c.id,
    };
    let both = store.write(&ctx.write(None, vec![result("call-1"), result("item-1")], NOW + 1)).await.unwrap();
    let again = store.write(&ctx.write(None, vec![result("item-1")], NOW + 2)).await.unwrap();
    assert_eq!((both.written, again.written), (1, 0));
    let results = principal_events(h, &ctx, "shell").await.into_iter().filter(|e| e.kind == EventKind::Result).count();
    assert_eq!(results, 1);
    let reply = store.lookup(&ctx.lookup(&["call-1"], &["item-1"], &[])).await.unwrap();
    let written = Some(link(&ctx, "shell", c.id, true));
    assert_eq!((reply.calls, reply.results), (vec![written.clone()], vec![written]));
}

pub async fn a_relink_between_lookup_and_write_marks_the_original_event(h: &impl Harness) {
    // The write guards the event that the LOOKUP found. A relink of the call id to a new event
    // after the lookup must not mark the new event.
    let (first, store) = (Ctx::new(true), h.store().await);
    let second = first.other_session();
    let (a, b) = (call("send"), call("read"));
    store.reserve(&first.reserve(vec![a.clone()], NOW, None)).await.unwrap();
    store.settle(&first.settle(vec![item(a.id, EventVerdict::Allow, "send", Some("call-1"))], NOW)).await.unwrap();
    let seen = store.lookup(&first.lookup(&[], &["call-1"], &[])).await.unwrap();
    let linked = seen.results[0].clone().expect("a link");
    assert_eq!(linked.event_id, a.id);

    // The relink, between the lookup and the write.
    store.reserve(&second.reserve(vec![b.clone()], NOW + 1, None)).await.unwrap();
    store.settle(&second.settle(vec![item(b.id, EventVerdict::Allow, "read", Some("call-1"))], NOW + 1)).await.unwrap();

    // Each result takes its tool from the `CallLink` of its own event, not from a constant.
    let result = |l: &CallLink| ResultEvent {
        id: fresh_id(),
        call_id: "call-1".into(),
        tool: l.tool.clone(),
        event_id: l.event_id,
    };
    let reply = store.write(&first.write(None, vec![result(&linked)], NOW + 2)).await.unwrap();
    assert_eq!(reply.written, 1);
    // The new event is not marked: its link still reads `result_written` false.
    let after = store.lookup(&first.lookup(&["call-1"], &[], &[])).await.unwrap();
    assert_eq!(after.calls, vec![Some(link(&second, "read", b.id, false))]);
    let new_link = after.calls[0].clone().expect("a link");
    // The original event is marked: a second result for it writes nothing; the new event's first does.
    let again = store.write(&first.write(None, vec![result(&linked)], NOW + 3)).await.unwrap();
    let fresh = store.write(&second.write(None, vec![result(&new_link)], NOW + 3)).await.unwrap();
    assert_eq!((again.written, fresh.written), (0, 1));
    // The result of event B is in the read history of its tool.
    let read_results = principal_events(h, &first, "read").await.into_iter().filter(|e| e.kind == EventKind::Result).count();
    assert_eq!(read_results, 1);
}

pub async fn concurrent_reserves_all_land(h: &impl Harness) {
    let ctx = Ctx::new(true);
    let stores = [h.store().await, h.store().await];
    let mut tasks = Vec::new();
    for i in 0..50 {
        let store = stores[i % 2].clone();
        let req = ctx.reserve(vec![call("send")], NOW, None);
        tasks.push(tokio::spawn(async move { store.reserve(&req).await.unwrap() }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(principal_events(h, &ctx, "send").await.len(), 50);
    assert_eq!(h.events(ctx.org, Scope::Session, &ctx.session.to_string(), "send").await.len(), 50);
}

pub async fn a_reserved_event_keeps_its_fields_and_key_id(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let (fields, key) = sample_fields();
    let c = CallEvent {
        fields: fields.clone(),
        key,
        ..call("send")
    };
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    let e = &principal_events(h, &ctx, "send").await[0];
    assert_eq!((e.id, &e.fields, e.key), (c.id, &fields, key));
    // A settle changes the verdict only.
    store.settle(&ctx.settle(vec![item(c.id, EventVerdict::Allow, "send", None)], NOW)).await.unwrap();
    let e = &principal_events(h, &ctx, "send").await[0];
    assert_eq!((e.verdict, &e.fields, e.key), (EventVerdict::Allow, &fields, key));
}

pub async fn an_unparsed_call_is_stored_as_unparsed(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = CallEvent {
        fields: Fields::Unparsed,
        ..call("send")
    };
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    let e = &principal_events(h, &ctx, "send").await[0];
    assert_eq!((&e.fields, e.key), (&Fields::Unparsed, None));
    // An empty field plan is not "unparsed".
    let plain = call("send");
    store.reserve(&ctx.reserve(vec![plain.clone()], NOW + 1, None)).await.unwrap();
    let e = principal_events(h, &ctx, "send").await.into_iter().find(|e| e.id == plain.id).expect("the plain call");
    assert_eq!(e.fields, no_fields());
}

pub async fn a_pr1_body_reads_as_no_fields(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let (fields, key) = sample_fields();
    let c = CallEvent { fields, key, ..call("send") };
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    let id = c.id;
    h.put_raw_body(ctx.org, id, &format!(r#"{{"v":1,"id":"{id}","ts":{NOW},"k":"request","t":"send"}}"#)).await;
    let e = &principal_events(h, &ctx, "send").await[0];
    assert_eq!((e.id, e.verdict, &e.fields, e.key), (id, EventVerdict::Pending, &no_fields(), None));
}

pub async fn a_result_event_copies_the_fields_of_its_call(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let (fields, key) = sample_fields();
    let parsed = CallEvent {
        fields: fields.clone(),
        key,
        ..call_as("send", "call-1")
    };
    let unparsed = CallEvent {
        fields: Fields::Unparsed,
        ..call_as("send", "call-2")
    };
    let plain = call_as("send", "call-3");
    store.reserve(&ctx.reserve(vec![parsed.clone(), unparsed.clone(), plain.clone()], NOW, None)).await.unwrap();
    let settle = vec![
        item(parsed.id, EventVerdict::Allow, "send", Some("call-1")),
        item(unparsed.id, EventVerdict::Allow, "send", Some("call-2")),
        item(plain.id, EventVerdict::Allow, "send", Some("call-3")),
    ];
    store.settle(&ctx.settle(settle, NOW)).await.unwrap();
    // The call of the third result has a PR 1 body: its result has no fields either.
    let pid = plain.id;
    h.put_raw_body(ctx.org, pid, &format!(r#"{{"v":1,"id":"{pid}","ts":{NOW},"k":"request","t":"send"}}"#)).await;
    let result = |call_id: &str, event_id| ResultEvent {
        id: fresh_id(),
        call_id: call_id.into(),
        tool: "send".into(),
        event_id,
    };
    let (r1, r2, r3) = (result("call-1", parsed.id), result("call-2", unparsed.id), result("call-3", plain.id));
    let reply = store.write(&ctx.write(None, vec![r1.clone(), r2.clone(), r3.clone()], NOW + 1)).await.unwrap();
    assert_eq!(reply.written, 3);
    let events = principal_events(h, &ctx, "send").await;
    let of = |id: Uuid| events.iter().find(|e| e.id == id).expect("the result event");
    assert_eq!((of(r1.id).kind, &of(r1.id).fields, of(r1.id).key), (EventKind::Result, &fields, key));
    assert_eq!((&of(r2.id).fields, of(r2.id).key), (&Fields::Unparsed, None));
    assert_eq!((&of(r3.id).fields, of(r3.id).key), (&no_fields(), None));
}

pub async fn the_reserve_writes_the_call_links_of_each_call(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let shell = CallEvent {
        item_id: Some("item-1".into()),
        ..call_as("shell", "call-1")
    };
    let send = call_as("send", "call-2");
    store.reserve(&ctx.reserve(vec![shell.clone(), send.clone()], NOW, None)).await.unwrap();
    let reply = store.lookup(&ctx.lookup(&["call-1", "item-1", "call-2"], &["call-2"], &[])).await.unwrap();
    let shell_link = Some(link(&ctx, "shell", shell.id, false));
    let send_link = Some(link(&ctx, "send", send.id, false));
    assert_eq!(reply.calls, vec![shell_link.clone(), shell_link, send_link.clone()]);
    assert_eq!(reply.results, vec![send_link]);
    // Every new key has a time to live.
    assert!(h.ttls_ms(ctx.org, NOW).await.iter().all(Option::is_some));
}

pub async fn the_reserve_keeps_a_link_of_another_event(h: &impl Harness) {
    let (first, store) = (Ctx::new(true), h.store().await);
    let second = first.other_session();
    let a = call_as("send", "call_0");
    store.reserve(&first.reserve(vec![a.clone()], NOW, None)).await.unwrap();
    store.settle(&first.settle(vec![item(a.id, EventVerdict::Allow, "send", Some("call_0"))], NOW)).await.unwrap();
    // A later turn reuses the call id.
    let b = call_as("read", "call_0");
    store.reserve(&second.reserve(vec![b.clone()], NOW + 1, None)).await.unwrap();
    let reply = store.lookup(&first.lookup(&["call_0"], &[], &[])).await.unwrap();
    assert_eq!(reply.calls, vec![Some(link(&first, "send", a.id, false))]);
}

pub async fn a_deny_settle_deletes_only_its_own_link(h: &impl Harness) {
    let (first, store) = (Ctx::new(true), h.store().await);
    let second = first.other_session();
    let a = call_as("send", "call-1");
    let b = call_as("send", "call-1");
    store.reserve(&first.reserve(vec![a.clone()], NOW, None)).await.unwrap();
    store.reserve(&second.reserve(vec![b.clone()], NOW + 1, None)).await.unwrap();
    // B is denied: the link names A, so it stays.
    store.settle(&second.settle(vec![item(b.id, EventVerdict::Deny, "send", Some("call-1"))], NOW + 2)).await.unwrap();
    let reply = store.lookup(&first.lookup(&["call-1"], &[], &[])).await.unwrap();
    assert_eq!(reply.calls, vec![Some(link(&first, "send", a.id, false))]);
    // A is denied: the link names A, so it goes.
    store.settle(&first.settle(vec![item(a.id, EventVerdict::Deny, "send", Some("call-1"))], NOW + 3)).await.unwrap();
    assert_eq!(store.lookup(&first.lookup(&["call-1"], &[], &[])).await.unwrap().calls, vec![None]);
}

pub async fn an_allow_settle_relinks_a_reused_call_id(h: &impl Harness) {
    let (first, store) = (Ctx::new(true), h.store().await);
    let second = first.other_session();
    let a = call_as("send", "call_0");
    let b = call_as("read", "call_0");
    store.reserve(&first.reserve(vec![a.clone()], NOW, None)).await.unwrap();
    store.reserve(&second.reserve(vec![b.clone()], NOW + 1, None)).await.unwrap();
    store.settle(&second.settle(vec![item(b.id, EventVerdict::Allow, "read", Some("call_0"))], NOW + 2)).await.unwrap();
    let reply = store.lookup(&first.lookup(&["call_0"], &[], &[])).await.unwrap();
    assert_eq!(reply.calls, vec![Some(link(&second, "read", b.id, false))]);
}

pub async fn an_appended_denied_call_counts_and_writes_no_link(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let id = fresh_id();
    let (fields, key) = sample_fields();
    let denied = AppendCall {
        fields: fields.clone(),
        key,
        ..append_call(id, "send", EventVerdict::Deny, Some("call-1"), Some("item-1"))
    };
    let reply = store.append(&append_req(&ctx, vec![denied], NOW)).await.unwrap();
    assert_eq!(reply.written, 1);
    let events = principal_events(h, &ctx, "send").await;
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].id, events[0].verdict, &events[0].fields, events[0].key), (id, EventVerdict::Deny, &fields, key));
    let reply = store.lookup(&ctx.lookup(&["call-1", "item-1"], &[], &[])).await.unwrap();
    assert_eq!(reply.calls, vec![None, None]);
}

pub async fn an_append_under_a_reserved_id_keeps_one_event(h: &impl Harness) {
    for verdict in [EventVerdict::Allow, EventVerdict::Deny] {
        let (ctx, store) = (Ctx::new(true), h.store().await);
        let c = call_as("send", "call-1");
        store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
        let (fields, key) = sample_fields();
        let late = AppendCall {
            fields: fields.clone(),
            key,
            ..append_call(c.id, "send", verdict, Some("call-1"), None)
        };
        store.append(&append_req(&ctx, vec![late], NOW + 5)).await.unwrap();
        let session = ctx.session.to_string();
        let api_key = ctx.key.clone().expect("a key caller");
        for (scope, scope_id) in [(Scope::Session, session.as_str()), (Scope::Principal, ctx.principal.as_str()), (Scope::Key, api_key.as_str())] {
            let events = h.events(ctx.org, scope, scope_id, "send").await;
            assert_eq!(events.len(), 1, "{verdict:?} {scope:?}: one event");
            let e = &events[0];
            assert_eq!((e.id, e.verdict, e.ts_ms, &e.fields, e.key), (c.id, verdict, NOW + 5, &fields, key), "{verdict:?} {scope:?}");
        }
        let reply = store.lookup(&ctx.lookup(&["call-1"], &[], &[])).await.unwrap();
        let want = (verdict == EventVerdict::Allow).then(|| link(&ctx, "send", c.id, false));
        assert_eq!(reply.calls, vec![want], "{verdict:?}");
    }
}

/// A reserve that returned too late for `mark_reserved` wrote its `c:` links, and the report site
/// then appends the blocked call under the same id. The denied append deletes each link that still
/// names its event, so a result for the blocked call is not linked (spec P8). A link of another
/// event stays.
pub async fn a_denied_append_under_a_reserved_id_deletes_its_own_link(h: &impl Harness) {
    let (first, store) = (Ctx::new(true), h.store().await);
    let second = first.other_session();
    let other = call_as("read", "item-1");
    store.reserve(&first.reserve(vec![other.clone()], NOW, None)).await.unwrap();
    let blocked = CallEvent {
        item_id: Some("item-1".into()),
        ..call_as("send", "call-1")
    };
    store.reserve(&second.reserve(vec![blocked.clone()], NOW + 1, None)).await.unwrap();
    let before = store.lookup(&second.lookup(&["call-1", "item-1"], &[], &[])).await.unwrap();
    assert_eq!(before.calls, vec![Some(link(&second, "send", blocked.id, false)), Some(link(&first, "read", other.id, false))]);

    let denied = append_call(blocked.id, "send", EventVerdict::Deny, Some("call-1"), Some("item-1"));
    store.append(&append_req(&second, vec![denied], NOW + 2)).await.unwrap();
    let after = store.lookup(&second.lookup(&["call-1", "item-1"], &["call-1"], &[])).await.unwrap();
    assert_eq!(after.calls, vec![None, Some(link(&first, "read", other.id, false))]);
    assert_eq!(after.results, vec![None], "a result for the blocked call is forged");
}

/// The Redis write script copies the stored fields of a call into its result by text. A tool name
/// that holds the text of a fields member, and an unparsed body with a key id, still give the
/// result exactly the fields of its call.
pub async fn a_result_copies_the_exact_fields_of_an_adversarial_call(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let tool = r#"x","f":{"amount":1}"#;
    let (fields, key) = sample_fields();
    let parsed = CallEvent {
        fields: fields.clone(),
        key,
        ..call_as(tool, "call-1")
    };
    let unparsed_key = Some(KeyId::new([9; 8]));
    let unparsed = CallEvent {
        fields: Fields::Unparsed,
        key: unparsed_key,
        ..call_as(tool, "call-2")
    };
    store.reserve(&ctx.reserve(vec![parsed.clone(), unparsed.clone()], NOW, None)).await.unwrap();
    let settle = vec![item(parsed.id, EventVerdict::Allow, tool, Some("call-1")), item(unparsed.id, EventVerdict::Allow, tool, Some("call-2"))];
    store.settle(&ctx.settle(settle, NOW)).await.unwrap();
    let result = |call_id: &str, event_id| ResultEvent {
        id: fresh_id(),
        call_id: call_id.into(),
        tool: tool.into(),
        event_id,
    };
    let (r1, r2) = (result("call-1", parsed.id), result("call-2", unparsed.id));
    assert_eq!(store.write(&ctx.write(None, vec![r1.clone(), r2.clone()], NOW + 1)).await.unwrap().written, 2);
    let events = principal_events(h, &ctx, tool).await;
    let of = |id: Uuid| events.iter().find(|e| e.id == id).expect("the result event");
    assert_eq!((of(r1.id).tool.as_str(), &of(r1.id).fields, of(r1.id).key), (tool, &fields, key));
    assert_eq!((of(r2.id).tool.as_str(), &of(r2.id).fields, of(r2.id).key), (tool, &Fields::Unparsed, unparsed_key));
}

/// Every case, in table order. The Redis binary calls this once per image.
pub async fn run_all(h: &impl Harness) {
    a_reserve_appends_one_pending_event_per_history(h).await;
    a_caller_without_a_key_writes_the_principal_history_once(h).await;
    the_split_by_tool_keeps_other_tools(h).await;
    retention_trims_only_events_older_than_25_hours(h).await;
    the_cap_keeps_the_newest_10_000_events_per_tool(h).await;
    the_truncated_flag_needs_a_lost_event_inside_the_window(h).await;
    settle_sets_the_verdict_and_writes_the_call_link(h).await;
    settle_twice_is_idempotent(h).await;
    a_deny_settle_writes_no_call_link(h).await;
    a_lost_settle_leaves_the_event_pending_and_no_call_link(h).await;
    a_settle_for_an_unknown_event_writes_nothing(h).await;
    the_reserve_writes_decision_links(h).await;
    links_are_isolated_per_principal_and_org(h).await;
    a_result_event_is_written_once_per_call_id(h).await;
    a_write_without_results_and_without_turn_writes_nothing(h).await;
    touch_keeps_the_latest_activity_and_the_highest_turn(h).await;
    a_future_event_survives_a_reserve_with_an_older_clock(h).await;
    every_written_key_expires_after_the_retention(h).await;
    append_writes_an_allowed_event_and_its_call_link(h).await;
    a_local_shell_settle_links_the_item_id_too(h).await;
    concurrent_reserves_all_land(h).await;
    a_cap_trim_in_the_principal_history_keeps_the_event_in_its_session_history(h).await;
    a_cut_of_two_flagged_events_gives_one_truncated_entry_per_scope_and_tool(h).await;
    a_reserve_on_a_new_session_writes_its_state(h).await;
    a_reserve_with_a_lower_turn_does_not_lower_the_turn(h).await;
    an_append_on_a_new_session_writes_its_state(h).await;
    a_second_allow_settle_of_a_call_id_relinks_it_to_the_new_event(h).await;
    the_first_settle_wins(h).await;
    a_repeated_settle_of_the_same_event_keeps_result_written(h).await;
    a_result_for_a_call_event_that_is_gone_writes_nothing(h).await;
    a_later_allow_settle_of_a_denied_event_writes_no_call_link(h).await;
    a_local_shell_result_under_both_aliases_is_written_once(h).await;
    a_relink_between_lookup_and_write_marks_the_original_event(h).await;
    a_reserved_event_keeps_its_fields_and_key_id(h).await;
    an_unparsed_call_is_stored_as_unparsed(h).await;
    a_pr1_body_reads_as_no_fields(h).await;
    a_result_event_copies_the_fields_of_its_call(h).await;
    the_reserve_writes_the_call_links_of_each_call(h).await;
    the_reserve_keeps_a_link_of_another_event(h).await;
    a_deny_settle_deletes_only_its_own_link(h).await;
    an_allow_settle_relinks_a_reused_call_id(h).await;
    an_appended_denied_call_counts_and_writes_no_link(h).await;
    an_append_under_a_reserved_id_keeps_one_event(h).await;
    a_denied_append_under_a_reserved_id_deletes_its_own_link(h).await;
    a_result_copies_the_exact_fields_of_an_adversarial_call(h).await;
}
