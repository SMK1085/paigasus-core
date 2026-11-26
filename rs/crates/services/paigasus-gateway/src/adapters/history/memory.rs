// SPDX-License-Identifier: Apache-2.0

//! The in-process `SessionStore` (SMA-753). For development and a single replica.
//!
//! It keeps the model of the Redis layout: one sorted list per (org, scope, scope id, tool), a tool
//! index per history, one event body per id with its verdict, the `c:` and `d:` links per
//! (org, principal), and a session state per (org, session). Each entry holds its expiry in ms,
//! `now_ms + KEY_TTL_SECS * 1000`, and every write resets it. A read ignores an entry whose expiry
//! is before the clock.
//!
//! The `c:` link rules and the stored fields follow the Redis adapter (`redis.rs` module doc). A
//! body keeps the fields as values, not as JSON; `put_raw_body` decodes a JSON body for a test.
//!
//! The window read (spec P4) follows the Redis adapter too: the reserve reads after its writes, at
//! most `MAX_READ_SETS` sets and `MAX_READ_EVENTS` events. A cap trim raises the truncation mark of
//! its list (the largest removed time). A tool past `MAX_INDEXED_TOOLS` is not indexed, and it
//! raises the every-tool mark of the history to the time of the write. A mark keeps the larger value
//! and the expiry of its list (or of the index).
//!
//! The store keeps a clock: the highest `now_ms` of any write it has seen. A lookup reads at the
//! higher of that clock and `LookupRequest::now_ms` (ruling T7-e), and does not move the clock. So a
//! test with a fixed time never reads at wall time.
//!
//! One `std::sync::Mutex` guards the state. No lock is held across an `.await`; no method awaits.
//! A poisoned lock is recovered: every update leaves whole entries.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

use paigasus_kernel::tool_policy::history::{Fields, KeyId, SetTool};
use uuid::Uuid;

use super::body::decode_body;
use crate::domain::tool_history::{
    AppendReply, AppendRequest, CallLink, EventKind, EventVerdict, Histories, HistoryStoreError, KEY_TTL_SECS, LookupReply, LookupRequest, MAX_EVENTS_PER_TOOL, MAX_INDEXED_TOOLS, MAX_READ_EVENTS,
    MAX_READ_SETS, RETENTION_MS, ReadRequest, ReserveRequest, Scope, SessionState, SessionStore, SetWindow, SettleRequest, StoredEvent, WriteRequest, link_digest,
};

const TTL_MS: u64 = KEY_TTL_SECS * 1000;
/// The sweep of expired entries runs on the write path, once per this many writes. No background task.
const SWEEP_EVERY: u64 = 1024;

/// A value and its expiry in ms.
struct Exp<T> {
    value: T,
    exp_ms: u64,
}

impl<T> Exp<T> {
    fn new(value: T, now_ms: u64) -> Self {
        Exp {
            value,
            exp_ms: now_ms.saturating_add(TTL_MS),
        }
    }

    fn alive(&self, clock_ms: u64) -> bool {
        self.exp_ms > clock_ms
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct HistoryKey {
    org: Uuid,
    scope: &'static str,
    scope_id: String,
}

/// Events ascending by (ts, id).
type EventList = Vec<(u64, Uuid)>;

struct Body {
    kind: EventKind,
    tool: String,
    verdict: EventVerdict,
    fields: Fields,
    key: Option<KeyId>,
    /// The result guard of a call event: true once its result event is written. It lives on the
    /// event, not on the `c:` link, so both aliases of a local shell call share it.
    result_written: bool,
}

#[derive(Default)]
struct State {
    clock_ms: u64,
    writes: u64,
    /// Events of one tool in one history, ascending by (ts, id).
    lists: HashMap<(HistoryKey, String), Exp<EventList>>,
    /// The tools of one history.
    index: HashMap<HistoryKey, Exp<BTreeSet<String>>>,
    bodies: HashMap<(Uuid, Uuid), Exp<Body>>,
    /// `c:` links: (org, principal, digest of the client id).
    calls: HashMap<(Uuid, String, String), Exp<CallLink>>,
    /// `d:` links: (org, principal, decision id) to the session.
    decisions: HashMap<(Uuid, String, Uuid), Exp<Uuid>>,
    sessions: HashMap<(Uuid, Uuid), Exp<SessionState>>,
    /// Truncation marks: (history, `Some(tool)`) for a list, (history, `None`) for every tool.
    marks: HashMap<(HistoryKey, Option<String>), Exp<u64>>,
}

/// One event to add to every history of a request.
struct NewEvent {
    id: Uuid,
    tool: String,
    kind: EventKind,
    verdict: EventVerdict,
    fields: Fields,
    key: Option<KeyId>,
}

fn hkey(org: Uuid, scope: Scope, scope_id: String) -> HistoryKey {
    HistoryKey { org, scope: scope.as_str(), scope_id }
}

impl State {
    /// Start of a write: move the clock, and sweep once per `SWEEP_EVERY` writes.
    fn advance(&mut self, now_ms: u64) {
        self.clock_ms = self.clock_ms.max(now_ms);
        self.writes += 1;
        if self.writes.is_multiple_of(SWEEP_EVERY) {
            self.sweep();
        }
    }

    /// Remove every entry whose expiry has passed.
    fn sweep(&mut self) {
        let clock = self.clock_ms;
        self.lists.retain(|_, e| e.alive(clock));
        self.index.retain(|_, e| e.alive(clock));
        self.bodies.retain(|_, e| e.alive(clock));
        self.calls.retain(|_, e| e.alive(clock));
        self.decisions.retain(|_, e| e.alive(clock));
        self.sessions.retain(|_, e| e.alive(clock));
        self.marks.retain(|_, e| e.alive(clock));
    }

    /// Raise a truncation mark to `value`: the larger value stays, and the expiry is `exp_ms` (the
    /// expiry of its list or index) when that is later. A dead mark counts as absent.
    fn raise_mark(&mut self, key: (HistoryKey, Option<String>), value: u64, exp_ms: u64) {
        let clock = self.clock_ms;
        let mark = self.marks.entry(key).or_insert(Exp { value, exp_ms });
        if mark.alive(clock) {
            mark.value = mark.value.max(value);
            mark.exp_ms = mark.exp_ms.max(exp_ms);
        } else {
            *mark = Exp { value, exp_ms };
        }
    }

    fn mark(&self, key: &(HistoryKey, Option<String>)) -> Option<u64> {
        self.marks.get(key).filter(|m| m.alive(self.clock_ms)).map(|m| m.value)
    }

    fn touch_session(&mut self, org: Uuid, session: Uuid, turn: i64, now_ms: u64) {
        let clock = self.clock_ms;
        match self.sessions.get_mut(&(org, session)).filter(|e| e.alive(clock)) {
            Some(e) => {
                e.value.last_ms = e.value.last_ms.max(now_ms);
                e.value.turn = e.value.turn.max(turn);
                e.exp_ms = now_ms.saturating_add(TTL_MS).max(e.exp_ms);
            }
            None => {
                self.sessions.insert((org, session), Exp::new(SessionState { id: session, last_ms: now_ms, turn }, now_ms));
            }
        }
    }

    /// Put `events` into every history, then trim each touched list.
    fn add_events(&mut self, h: &Histories, events: &[NewEvent], now_ms: u64, window_from_ms: Option<u64>) -> AppendReply {
        let mut reply = AppendReply {
            written: events.len(),
            ..AppendReply::empty()
        };
        for e in events {
            // A rewrite of a live body keeps its result guard (as `HSET` keeps the `r` field).
            let clock = self.clock_ms;
            let live = self.bodies.get(&(h.org, e.id)).filter(|b| b.alive(clock));
            let result_written = live.is_some_and(|b| b.value.result_written);
            // A write with an older `now_ms` never shortens a live expiry.
            let live_exp = live.map_or(0, |b| b.exp_ms);
            let mut fresh = Exp::new(
                Body {
                    kind: e.kind,
                    tool: e.tool.clone(),
                    verdict: e.verdict,
                    fields: e.fields.clone(),
                    key: e.key,
                    result_written,
                },
                now_ms,
            );
            fresh.exp_ms = fresh.exp_ms.max(live_exp);
            self.bodies.insert((h.org, e.id), fresh);
        }
        let cutoff = now_ms.saturating_sub(RETENTION_MS);
        let mut by_retention: HashSet<Uuid> = HashSet::new();
        let mut by_cap: HashSet<Uuid> = HashSet::new();
        let tools: BTreeSet<&str> = events.iter().map(|e| e.tool.as_str()).collect();
        for (scope, scope_id) in h.scopes() {
            let key = hkey(h.org, scope, scope_id);
            // An expired index counts as absent: drop it, so its old tools do not come back.
            if self.index.get(&key).is_some_and(|e| e.exp_ms < now_ms) {
                self.index.remove(&key);
            }
            let index = self.index.entry(key.clone()).or_insert_with(|| Exp::new(BTreeSet::new(), now_ms));
            index.exp_ms = index.exp_ms.max(now_ms.saturating_add(TTL_MS));
            let mut index_full = false;
            for e in events {
                if index.value.contains(&e.tool) || index.value.len() < MAX_INDEXED_TOOLS {
                    index.value.insert(e.tool.clone());
                } else {
                    index_full = true;
                }
                let lkey = (key.clone(), e.tool.clone());
                // An expired list (its expiry is before the time of this write) counts as absent: drop it, so its old members do not come back.
                if self.lists.get(&lkey).is_some_and(|l| l.exp_ms < now_ms)
                    && let Some(dead) = self.lists.remove(&lkey)
                {
                    // A dropped member older than the retention still counts as trimmed by it, as
                    // the Redis list would have trimmed it.
                    by_retention.extend(dead.value.iter().filter(|(ts, _)| *ts < cutoff).map(|(_, id)| *id));
                }
                let list = self.lists.entry(lkey).or_insert_with(|| Exp::new(Vec::new(), now_ms));
                list.exp_ms = list.exp_ms.max(now_ms.saturating_add(TTL_MS));
                // A replay moves the list entry of its id, as `ZADD` moves the score of a member
                // (plan decision D-E). The id in the list decides, not the body: an evicted body
                // leaves its member in the list.
                list.value.retain(|(_, id)| *id != e.id);
                let entry = (now_ms, e.id);
                if let Err(at) = list.value.binary_search(&entry) {
                    list.value.insert(at, entry);
                }
            }
            if index_full {
                let exp_ms = self.index.get(&key).map_or(0, |i| i.exp_ms);
                self.raise_mark((key.clone(), None), now_ms, exp_ms);
            }
            for tool in &tools {
                let Some(list) = self.lists.get_mut(&(key.clone(), (*tool).to_string())) else { continue };
                list.value.retain(|(ts, id)| {
                    let keep = *ts >= cutoff;
                    if !keep {
                        by_retention.insert(*id);
                    }
                    keep
                });
                let mut cut_in_window = false;
                let mut largest_cut: Option<u64> = None;
                let over = list.value.len().saturating_sub(MAX_EVENTS_PER_TOOL);
                for (ts, id) in list.value.drain(..over) {
                    by_cap.insert(id);
                    largest_cut = largest_cut.max(Some(ts));
                    cut_in_window |= window_from_ms.is_some_and(|w| ts >= w);
                }
                if let Some(cut) = largest_cut {
                    let exp_ms = list.exp_ms;
                    self.raise_mark((key.clone(), Some((*tool).to_string())), cut, exp_ms);
                }
                if cut_in_window {
                    let flag = (scope, (*tool).to_string());
                    if !reply.truncated.contains(&flag) {
                        reply.truncated.push(flag);
                    }
                }
            }
        }
        reply.trimmed_retention = by_retention.len() as u64;
        reply.trimmed_cap = by_cap.len() as u64;
        reply
    }

    /// Write the `c:` link of a call under one client id. An existing link is replaced: session,
    /// tool and event id change. The result guard is on the event, so a relink to a new event
    /// reads the fresh guard of that event (ruling T7-b), and a settle retry for the same event
    /// keeps the written guard (ruling T7-f). The stored `result_written` is not read.
    fn put_call_link(&mut self, org: Uuid, principal: &str, client_id: &str, link: CallLink, now_ms: u64) {
        self.calls.insert((org, principal.to_string(), link_digest(client_id)), Exp::new(link, now_ms));
    }

    /// The reserve rule (spec P8): write the link unless a live link names another event.
    fn keep_or_put_call_link(&mut self, org: Uuid, principal: &str, client_id: &str, link: CallLink, now_ms: u64) {
        let clock = self.clock_ms;
        let held = self
            .calls
            .get(&(org, principal.to_string(), link_digest(client_id)))
            .filter(|e| e.alive(clock))
            .map(|e| e.value.event_id);
        if held.is_none_or(|id| id == link.event_id) {
            self.put_call_link(org, principal, client_id, link, now_ms);
        }
    }

    /// The deny rule (spec P8): delete the link only when it names `event_id`.
    fn drop_own_call_link(&mut self, org: Uuid, principal: &str, client_id: &str, event_id: Uuid) {
        let key = (org, principal.to_string(), link_digest(client_id));
        if self.calls.get(&key).is_some_and(|e| e.value.event_id == event_id) {
            self.calls.remove(&key);
        }
    }

    /// The live `c:` link under one client id, with `result_written` read from its event.
    fn call_link(&self, org: Uuid, principal: &str, client_id: &str, clock_ms: u64) -> Option<CallLink> {
        let e = self.calls.get(&(org, principal.to_string(), link_digest(client_id))).filter(|e| e.alive(clock_ms))?;
        let mut link = e.value.clone();
        link.result_written = self.bodies.get(&(org, link.event_id)).is_some_and(|b| b.alive(clock_ms) && b.value.result_written);
        Some(link)
    }

    /// The window read of a reserve (spec P4), at the clock and after its writes. A set past a read
    /// limit, and every later set, is truncated with no event and no mark.
    fn read_window(&self, h: &Histories, read: &ReadRequest) -> (Vec<SetWindow>, u64) {
        let clock = self.clock_ms;
        let mut window = Vec::with_capacity(read.sets.len());
        let (mut read_events, mut missing, mut stopped) = (0usize, 0u64, false);
        for (n, (scope, set)) in read.sets.iter().enumerate() {
            let key = hkey(h.org, *scope, h.scope_id(*scope));
            let (tools, index_mark): (Vec<String>, Option<u64>) = match set {
                SetTool::Tool(t) => (vec![t.clone()], None),
                SetTool::Every => (
                    self.index.get(&key).filter(|i| i.alive(clock)).map(|i| i.value.iter().cloned().collect()).unwrap_or_default(),
                    self.mark(&(key.clone(), None)),
                ),
                // A set kind that this adapter does not know: fail closed.
                _ => {
                    stopped = true;
                    (Vec::new(), None)
                }
            };
            let lists: Vec<&EventList> = tools
                .iter()
                .filter_map(|t| self.lists.get(&(key.clone(), t.clone())).filter(|l| l.alive(clock)).map(|l| &l.value))
                .collect();
            stopped = stopped || n >= MAX_READ_SETS;
            if !stopped {
                let count: usize = lists.iter().map(|l| l.iter().filter(|(ts, _)| *ts >= read.from_ms).count()).sum();
                stopped = read_events + count > MAX_READ_EVENTS;
                read_events += count;
            }
            let mut entry = SetWindow {
                scope: *scope,
                tool: set.clone(),
                events: Vec::new(),
                mark_ms: None,
                truncated: stopped,
            };
            if !stopped {
                entry.mark_ms = tools.iter().filter_map(|t| self.mark(&(key.clone(), Some(t.clone())))).chain(index_mark).max();
                for (ts, id) in lists.iter().flat_map(|l| l.iter()).filter(|(ts, _)| *ts >= read.from_ms) {
                    match self.bodies.get(&(h.org, *id)).filter(|b| b.alive(clock)) {
                        Some(body) => entry.events.push(StoredEvent {
                            id: *id,
                            ts_ms: *ts,
                            kind: body.value.kind,
                            tool: body.value.tool.clone(),
                            verdict: body.value.verdict,
                            fields: body.value.fields.clone(),
                            key: body.value.key,
                        }),
                        None => missing += 1,
                    }
                }
                entry.events.sort_by_key(|e| (e.ts_ms, e.id));
            }
            window.push(entry);
        }
        (window, missing)
    }
}

#[derive(Default)]
pub struct MemorySessionStore {
    state: Mutex<State>,
}

impl MemorySessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// For the test harness: the events of one history and tool, ordered by `ts_ms`. The list decides
    /// what is present, and a body whose expiry is before the clock hides its entry. A body that expires at the clock itself is still read, as the list entry is.
    pub fn events(&self, org: Uuid, scope: Scope, scope_id: &str, tool: &str) -> Vec<StoredEvent> {
        let st = self.lock();
        let key = (hkey(org, scope, scope_id.to_string()), tool.to_string());
        let Some(list) = st.lists.get(&key).filter(|l| l.alive(st.clock_ms)) else {
            return Vec::new();
        };
        list.value
            .iter()
            .filter_map(|(ts, id)| {
                let body = st.bodies.get(&(org, *id)).filter(|b| b.exp_ms >= st.clock_ms)?;
                Some(StoredEvent {
                    id: *id,
                    ts_ms: *ts,
                    kind: body.value.kind,
                    tool: body.value.tool.clone(),
                    verdict: body.value.verdict,
                    fields: body.value.fields.clone(),
                    key: body.value.key,
                })
            })
            .collect()
    }

    /// For the test harness: replace the body of one event with a JSON body (one that another
    /// version wrote). The verdict, the result guard and the expiry stay. An unknown event gets a
    /// pending body.
    ///
    /// # Errors
    ///
    /// A body that `decode_body` refuses.
    pub fn put_raw_body(&self, org: Uuid, id: Uuid, raw: &str) -> Result<(), HistoryStoreError> {
        let decoded = decode_body(raw)?;
        let mut st = self.lock();
        let clock = st.clock_ms;
        let entry = st.bodies.entry((org, id)).or_insert_with(|| {
            Exp::new(
                Body {
                    kind: decoded.kind,
                    tool: decoded.tool.clone(),
                    verdict: EventVerdict::Pending,
                    fields: Fields::Unparsed,
                    key: None,
                    result_written: false,
                },
                clock,
            )
        });
        entry.value.kind = decoded.kind;
        entry.value.tool = decoded.tool;
        entry.value.fields = decoded.fields;
        entry.value.key = decoded.key;
        Ok(())
    }

    /// For the test harness: delete the body of one event, as an eviction does (spec P10). Its list
    /// entries stay. `false` when no body was stored.
    pub fn drop_body(&self, org: Uuid, id: Uuid) -> bool {
        self.lock().bodies.remove(&(org, id)).is_some()
    }

    /// For the test harness: the time to live in ms of every stored key of the org.
    pub fn ttls_ms(&self, org: Uuid, now_ms: u64) -> Vec<Option<u64>> {
        let st = self.lock();
        let ttl = |exp_ms: u64| Some(exp_ms - now_ms);
        let mut out = Vec::new();
        out.extend(st.lists.iter().filter(|(k, e)| k.0.org == org && e.alive(now_ms)).map(|(_, e)| ttl(e.exp_ms)));
        out.extend(st.index.iter().filter(|(k, e)| k.org == org && e.alive(now_ms)).map(|(_, e)| ttl(e.exp_ms)));
        out.extend(st.bodies.iter().filter(|(k, e)| k.0 == org && e.alive(now_ms)).map(|(_, e)| ttl(e.exp_ms)));
        out.extend(st.calls.iter().filter(|(k, e)| k.0 == org && e.alive(now_ms)).map(|(_, e)| ttl(e.exp_ms)));
        out.extend(st.decisions.iter().filter(|(k, e)| k.0 == org && e.alive(now_ms)).map(|(_, e)| ttl(e.exp_ms)));
        out.extend(st.sessions.iter().filter(|(k, e)| k.0 == org && e.alive(now_ms)).map(|(_, e)| ttl(e.exp_ms)));
        out.extend(st.marks.iter().filter(|(k, e)| k.0.org == org && e.alive(now_ms)).map(|(_, e)| ttl(e.exp_ms)));
        out
    }
}

#[async_trait::async_trait]
impl SessionStore for MemorySessionStore {
    async fn lookup(&self, req: &LookupRequest) -> Result<LookupReply, HistoryStoreError> {
        let st = self.lock();
        let clock = st.clock_ms.max(req.now_ms);
        let get = |id: &String| st.call_link(req.org, &req.principal, id, clock);
        let calls: Vec<Option<CallLink>> = req.call_ids.iter().map(get).collect();
        let results: Vec<Option<CallLink>> = req.result_call_ids.iter().map(get).collect();
        let refs: Vec<Option<Uuid>> = req
            .refs
            .iter()
            .map(|id| st.decisions.get(&(req.org, req.principal.clone(), *id)).filter(|e| e.alive(clock)).map(|e| e.value))
            .collect();
        let mut named: Vec<Uuid> = Vec::new();
        for s in calls.iter().chain(results.iter()).flatten().map(|l| l.session).chain(refs.iter().flatten().copied()) {
            if !named.contains(&s) {
                named.push(s);
            }
        }
        let sessions = named
            .iter()
            .filter_map(|s| st.sessions.get(&(req.org, *s)).filter(|e| e.alive(clock)).map(|e| e.value.clone()))
            .collect();
        Ok(LookupReply { calls, results, refs, sessions })
    }

    async fn write(&self, req: &WriteRequest) -> Result<AppendReply, HistoryStoreError> {
        let mut st = self.lock();
        st.advance(req.now_ms);
        let h = &req.histories;
        let mut events = Vec::new();
        for r in &req.results {
            // The guard is on the call event that the lookup found, not on the link: a relink
            // after the lookup cannot move it, and two aliases of one call share it. A call event
            // that is gone (F5) gets no result and no new entry.
            let clock = st.clock_ms;
            let Some(call) = st.bodies.get_mut(&(h.org, r.event_id)).filter(|e| e.alive(clock)) else {
                continue;
            };
            if call.value.result_written {
                continue;
            }
            call.value.result_written = true;
            call.exp_ms = req.now_ms.saturating_add(TTL_MS).max(call.exp_ms);
            // A result event gets the stored fields of its call (spec P3, design D5).
            events.push(NewEvent {
                id: r.id,
                tool: r.tool.clone(),
                kind: EventKind::Result,
                verdict: EventVerdict::Allow,
                fields: call.value.fields.clone(),
                key: call.value.key,
            });
        }
        let reply = if events.is_empty() { AppendReply::empty() } else { st.add_events(h, &events, req.now_ms, None) };
        if let Some(turn) = req.turn {
            st.touch_session(h.org, h.session, turn, req.now_ms);
        }
        Ok(reply)
    }

    async fn reserve(&self, req: &ReserveRequest) -> Result<AppendReply, HistoryStoreError> {
        let mut st = self.lock();
        st.advance(req.now_ms);
        let h = &req.histories;
        let events: Vec<NewEvent> = req
            .calls
            .iter()
            .map(|c| NewEvent {
                id: c.id,
                tool: c.tool.clone(),
                kind: EventKind::Call,
                verdict: EventVerdict::Pending,
                fields: c.fields.clone(),
                key: c.key,
            })
            .collect();
        let mut reply = st.add_events(h, &events, req.now_ms, req.read.as_ref().map(|r| r.from_ms));
        let clock = st.clock_ms;
        for c in &req.calls {
            let key = (h.org, h.principal.clone(), c.id);
            let live_exp = st.decisions.get(&key).filter(|e| e.alive(clock)).map_or(0, |e| e.exp_ms);
            let mut link = Exp::new(h.session, req.now_ms);
            link.exp_ms = link.exp_ms.max(live_exp);
            st.decisions.insert(key, link);
            let call_link = CallLink {
                session: h.session,
                tool: c.tool.clone(),
                event_id: c.id,
                result_written: false,
            };
            for id in c.call_id.iter().chain(c.item_id.iter()) {
                st.keep_or_put_call_link(h.org, &h.principal, id, call_link.clone(), req.now_ms);
            }
        }
        st.touch_session(h.org, h.session, req.turn, req.now_ms);
        if let Some(read) = &req.read {
            (reply.window, reply.missing_bodies) = st.read_window(h, read);
        }
        Ok(reply)
    }

    async fn settle(&self, req: &SettleRequest) -> Result<(), HistoryStoreError> {
        let mut st = self.lock();
        st.advance(req.now_ms);
        for item in &req.items {
            let clock = st.clock_ms;
            let Some(body) = st.bodies.get_mut(&(req.org, item.event_id)).filter(|b| b.alive(clock)) else {
                continue;
            };
            if body.value.verdict == EventVerdict::Pending {
                body.value.verdict = item.verdict;
            }
            body.exp_ms = req.now_ms.saturating_add(TTL_MS).max(body.exp_ms);
            // The first settle wins the verdict (T7-c), so the link follows the STORED verdict: a
            // later allow settle of a denied event writes no link, and a later deny settle of an
            // allowed event deletes none.
            let stored = body.value.verdict;
            if item.verdict == EventVerdict::Deny && stored == EventVerdict::Deny {
                for id in item.call_id.iter().chain(item.item_id.iter()) {
                    st.drop_own_call_link(req.org, &req.principal, id, item.event_id);
                }
                continue;
            }
            if item.verdict != EventVerdict::Allow || stored != EventVerdict::Allow {
                continue;
            }
            let link = CallLink {
                session: req.session,
                tool: item.tool.clone(),
                event_id: item.event_id,
                result_written: false,
            };
            for id in item.call_id.iter().chain(item.item_id.iter()) {
                st.put_call_link(req.org, &req.principal, id, link.clone(), req.now_ms);
            }
        }
        Ok(())
    }

    async fn append(&self, req: &AppendRequest) -> Result<AppendReply, HistoryStoreError> {
        let mut st = self.lock();
        st.advance(req.now_ms);
        let h = &req.histories;
        let events: Vec<NewEvent> = req
            .calls
            .iter()
            .map(|c| NewEvent {
                id: c.id,
                tool: c.tool.clone(),
                kind: EventKind::Call,
                verdict: c.verdict,
                fields: c.fields.clone(),
                key: c.key,
            })
            .collect();
        let reply = st.add_events(h, &events, req.now_ms, None);
        // Only an allowed call is linked: a denied call was not forwarded (spec P9). A denied call
        // deletes each link that still names its event: a reserve that returned too late for the
        // report site wrote them (spec P8).
        for c in req.calls.iter().filter(|c| c.verdict == EventVerdict::Deny) {
            for id in c.call_id.iter().chain(c.item_id.iter()) {
                st.drop_own_call_link(h.org, &h.principal, id, c.id);
            }
        }
        for c in req.calls.iter().filter(|c| c.verdict == EventVerdict::Allow) {
            let link = CallLink {
                session: h.session,
                tool: c.tool.clone(),
                event_id: c.id,
                result_written: false,
            };
            for id in c.call_id.iter().chain(c.item_id.iter()) {
                st.put_call_link(h.org, &h.principal, id, link.clone(), req.now_ms);
            }
        }
        st.touch_session(h.org, h.session, req.turn, req.now_ms);
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::domain::tool_history::{AppendCall, CallEvent};

    fn histories(org: u128) -> Histories {
        Histories {
            org: Uuid::from_u128(org),
            principal: "p".into(),
            key: None,
            session: Uuid::from_u128(2),
        }
    }

    fn append_req(org: u128, now_ms: u64) -> AppendRequest {
        AppendRequest {
            histories: histories(org),
            turn: 1,
            calls: vec![AppendCall {
                id: Uuid::from_u128(3),
                tool: "send".into(),
                verdict: EventVerdict::Allow,
                call_id: Some("c1".into()),
                item_id: None,
                fields: Fields::Values(BTreeMap::new()),
                key: None,
            }],
            now_ms,
        }
    }

    fn lookup_req(org: u128, now_ms: u64) -> LookupRequest {
        LookupRequest {
            org: Uuid::from_u128(org),
            principal: "p".into(),
            call_ids: vec!["c1".into()],
            result_call_ids: vec![],
            refs: vec![],
            now_ms,
        }
    }

    #[tokio::test]
    async fn an_expired_link_reads_as_absent() {
        let store = MemorySessionStore::new();
        store.append(&append_req(1, 1_000)).await.unwrap();
        // Still alive at the last instant before the expiry, gone at the expiry. No other write.
        let alive = store.lookup(&lookup_req(1, 1_000 + TTL_MS - 1)).await.unwrap();
        assert!(alive.calls[0].is_some());
        let gone = store.lookup(&lookup_req(1, 1_000 + TTL_MS)).await.unwrap();
        assert_eq!(gone.calls, vec![None]);
        assert!(gone.sessions.is_empty());
    }

    fn call_event(id: u128) -> CallEvent {
        CallEvent {
            id: Uuid::from_u128(id),
            tool: "send".into(),
            call_id: None,
            item_id: None,
            fields: Fields::Values(BTreeMap::new()),
            key: None,
        }
    }

    fn reserve_at(now_ms: u64, id: u128) -> ReserveRequest {
        ReserveRequest {
            histories: histories(1),
            turn: 1,
            calls: vec![call_event(id)],
            now_ms,
            read: None,
        }
    }

    #[tokio::test]
    async fn an_expired_list_does_not_bring_back_its_old_members() {
        let store = MemorySessionStore::new();
        store.reserve(&reserve_at(1_000, 10)).await.unwrap();
        let later = 1_000 + TTL_MS + 5;
        store.reserve(&reserve_at(later, 11)).await.unwrap();
        let ids: Vec<Uuid> = store.events(Uuid::from_u128(1), Scope::Session, &Uuid::from_u128(2).to_string(), "send").iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![Uuid::from_u128(11)]);
    }

    #[tokio::test]
    async fn an_older_write_does_not_shorten_a_live_expiry() {
        let store = MemorySessionStore::new();
        store.reserve(&reserve_at(10_000, 10)).await.unwrap();
        store.reserve(&reserve_at(2_000, 10)).await.unwrap();
        let ttls = store.ttls_ms(Uuid::from_u128(1), 10_000);
        assert!(ttls.iter().flatten().all(|t| *t >= TTL_MS), "{ttls:?}");
    }

    #[tokio::test]
    async fn a_replay_with_a_missing_body_counts_once() {
        let store = MemorySessionStore::new();
        store.reserve(&reserve_at(1_000, 10)).await.unwrap();
        assert!(store.drop_body(Uuid::from_u128(1), Uuid::from_u128(10)));
        store.reserve(&reserve_at(2_000, 10)).await.unwrap();
        let ids: Vec<Uuid> = store.events(Uuid::from_u128(1), Scope::Session, &Uuid::from_u128(2).to_string(), "send").iter().map(|e| e.id).collect();
        // `ZADD` moves the member: one list entry, at the time of the replay.
        assert_eq!(ids, vec![Uuid::from_u128(10)]);
        let st = store.lock();
        let entries: Vec<_> = st.lists.values().flat_map(|l| l.value.iter().copied()).collect();
        assert!(entries.iter().all(|(ts, _)| *ts == 2_000), "{entries:?}");
    }

    #[tokio::test]
    async fn events_hides_a_body_that_is_not_live() {
        let store = MemorySessionStore::new();
        store.reserve(&reserve_at(1_000, 10)).await.unwrap();
        {
            let mut st = store.lock();
            st.bodies.get_mut(&(Uuid::from_u128(1), Uuid::from_u128(10))).unwrap().exp_ms = 0;
        }
        assert!(store.events(Uuid::from_u128(1), Scope::Session, &Uuid::from_u128(2).to_string(), "send").is_empty());
    }

    #[tokio::test]
    async fn the_sweep_removes_expired_entries_and_keeps_live_ones() {
        let store = MemorySessionStore::new();
        store.append(&append_req(1, 1_000)).await.unwrap();
        let later = 1_000 + TTL_MS + 1;
        // One live org write, then enough writes to reach the sweep.
        store
            .reserve(&ReserveRequest {
                histories: histories(9),
                turn: 1,
                calls: vec![call_event(4)],
                now_ms: later,
                read: None,
            })
            .await
            .unwrap();
        for _ in 0..SWEEP_EVERY {
            store
                .write(&WriteRequest {
                    histories: histories(9),
                    turn: None,
                    results: vec![],
                    now_ms: later,
                })
                .await
                .unwrap();
        }
        let st = store.lock();
        assert!(st.calls.is_empty() && st.sessions.keys().all(|k| k.0 == Uuid::from_u128(9)));
        assert!(st.lists.keys().all(|k| k.0.org == Uuid::from_u128(9)));
        assert!(st.bodies.keys().all(|k| k.0 == Uuid::from_u128(9)));
        assert!(!st.lists.is_empty() && !st.bodies.is_empty() && !st.index.is_empty() && !st.sessions.is_empty());
    }
}
