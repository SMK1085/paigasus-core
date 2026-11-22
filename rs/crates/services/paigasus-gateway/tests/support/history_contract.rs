// SPDX-License-Identifier: Apache-2.0

//! The session history store contract suite (SMA-753). One set of cases that BOTH adapters must
//! pass, through the port only: `tests/history_store_memory.rs` runs it on the memory store and
//! the Redis binary runs it on a real Redis. Every case takes its own ids, so cases can share one
//! Redis. Every instant is fixed; nothing sleeps.
//!
//! Counts: `written` counts events, not rows. One call written to three histories is 1.
//! `trimmed_retention` and `trimmed_cap` count distinct events that the call removed.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use paigasus_gateway::domain::tool_history::{
    AppendRequest, CallEvent, CallLink, EventKind, EventVerdict, ForwardedCall, Histories, LookupRequest, MAX_EVENTS_PER_TOOL, RETENTION_MS, ReserveRequest, ResultEvent, Scope, SessionState,
    SessionStore, SettleItem, SettleRequest, StoredEvent, WriteRequest,
};
use uuid::Uuid;

/// How a case reaches one backend.
#[async_trait::async_trait]
pub trait Harness: Send + Sync {
    /// Redis: a new store per call (one more replica). Memory: the one shared store.
    async fn store(&self) -> Arc<dyn SessionStore>;
    async fn events(&self, org: Uuid, scope: Scope, scope_id: &str, tool: &str) -> Vec<StoredEvent>;
    async fn ttls_ms(&self, org: Uuid, now_ms: u64) -> Vec<Option<u64>>;
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
        }
    }

    fn reserve(&self, calls: Vec<CallEvent>, now_ms: u64, window_from_ms: Option<u64>) -> ReserveRequest {
        ReserveRequest {
            histories: self.histories(),
            turn: 1,
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

fn call(tool: &str) -> CallEvent {
    CallEvent { id: fresh_id(), tool: tool.into() }
}

fn calls(tool: &str, n: usize) -> Vec<CallEvent> {
    fresh_ids(n).into_iter().map(|id| CallEvent { id, tool: tool.into() }).collect()
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
    store.reserve(&ctx.reserve(calls("send", MAX_EVENTS_PER_TOOL), NOW - 1, None)).await.unwrap();
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
    store.reserve(&ctx.reserve(old.clone(), NOW - 1, None)).await.unwrap();
    let reply = store.reserve(&ctx.reserve(calls("send", 1), NOW, None)).await.unwrap();
    assert_eq!((reply.trimmed_cap, reply.trimmed_retention), (1, 0));
    let events = principal_events(h, &ctx, "send").await;
    assert_eq!(events.len(), MAX_EVENTS_PER_TOOL);
    let oldest = old.iter().map(|c| c.id).min().unwrap();
    assert!(events.iter().all(|e| e.id != oldest), "the oldest event went");
    assert_eq!(events.last().unwrap().ts_ms, NOW);
}

pub async fn the_truncated_flag_needs_a_lost_event_inside_the_window(h: &impl Harness) {
    let _ = h;
    let store = h.store().await;
    for (window_from, flagged) in [(Some(NOW - 100), true), (Some(NOW - 99), false), (None, false)] {
        let ctx = Ctx::new(false);
        store.reserve(&ctx.reserve(calls("send", MAX_EVENTS_PER_TOOL), NOW - 100, None)).await.unwrap();
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
    let _ = h;
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
    let _ = h;
}

pub async fn a_result_event_is_written_once_per_call_id(h: &impl Harness) {
    let (ctx, store) = (Ctx::new(true), h.store().await);
    let c = call("send");
    store.reserve(&ctx.reserve(vec![c.clone()], NOW, None)).await.unwrap();
    store.settle(&ctx.settle(vec![item(c.id, EventVerdict::Allow, "send", Some("call-1"))], NOW)).await.unwrap();
    let result = |id| ResultEvent {
        id,
        call_id: "call-1".into(),
        tool: "send".into(),
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
    let _ = h;
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
    let req = AppendRequest {
        histories: ctx.histories(),
        turn: 1,
        calls: vec![ForwardedCall {
            id,
            tool: "send".into(),
            call_id: Some("call-1".into()),
            item_id: None,
        }],
        now_ms: NOW,
    };
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
    let req = AppendRequest {
        histories: ctx.histories(),
        turn: 1,
        calls: vec![ForwardedCall {
            id,
            tool: "shell".into(),
            call_id: Some("call-2".into()),
            item_id: Some("item-2".into()),
        }],
        now_ms: NOW,
    };
    store.append(&req).await.unwrap();
    let expected = Some(link(&ctx, "shell", id, false));
    let reply = store.lookup(&ctx.lookup(&[], &["item-2", "call-2"], &[])).await.unwrap();
    assert_eq!(reply.results, vec![expected.clone(), expected]);
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
}
