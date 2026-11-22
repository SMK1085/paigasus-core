// SPDX-License-Identifier: Apache-2.0

//! The in-process `SessionStore` (SMA-753). For development and a single replica.
//!
//! It keeps the model of the Redis layout: one sorted list per (org, scope, scope id, tool), a tool
//! index per history, one event body per id with its verdict, the `c:` and `d:` links per
//! (org, principal), and a session state per (org, session). Each entry holds its expiry in ms,
//! `now_ms + KEY_TTL_SECS * 1000`, and every write resets it. A read ignores an entry whose expiry
//! is before the clock.
//!
//! The store keeps a clock: the highest `now_ms` of any write it has seen. A lookup reads at the
//! higher of that clock and `LookupRequest::now_ms` (ruling T7-e), and does not move the clock. So a
//! test with a fixed time never reads at wall time.
//!
//! One `std::sync::Mutex` guards the state. No lock is held across an `.await`; no method awaits.
//! A poisoned lock is recovered: every update leaves whole entries.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

use uuid::Uuid;

use crate::domain::tool_history::{
    AppendReply, AppendRequest, CallLink, EventKind, EventVerdict, Histories, HistoryStoreError, KEY_TTL_SECS, LookupReply, LookupRequest, MAX_EVENTS_PER_TOOL, RETENTION_MS, ReserveRequest, Scope,
    SessionState, SessionStore, SettleRequest, StoredEvent, WriteRequest, link_digest,
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
}

/// One event to add to every history of a request.
struct NewEvent {
    id: Uuid,
    tool: String,
    kind: EventKind,
    verdict: EventVerdict,
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
            trimmed_retention: 0,
            trimmed_cap: 0,
            truncated: Vec::new(),
        };
        for e in events {
            self.bodies.insert(
                (h.org, e.id),
                Exp::new(
                    Body {
                        kind: e.kind,
                        tool: e.tool.clone(),
                        verdict: e.verdict,
                    },
                    now_ms,
                ),
            );
        }
        let cutoff = now_ms.saturating_sub(RETENTION_MS);
        let mut by_retention: HashSet<Uuid> = HashSet::new();
        let mut by_cap: HashSet<Uuid> = HashSet::new();
        let tools: BTreeSet<&str> = events.iter().map(|e| e.tool.as_str()).collect();
        for (scope, scope_id) in h.scopes() {
            let key = hkey(h.org, scope, scope_id);
            let index = self.index.entry(key.clone()).or_insert_with(|| Exp::new(BTreeSet::new(), now_ms));
            index.exp_ms = now_ms.saturating_add(TTL_MS);
            for e in events {
                index.value.insert(e.tool.clone());
                let list = self.lists.entry((key.clone(), e.tool.clone())).or_insert_with(|| Exp::new(Vec::new(), now_ms));
                list.exp_ms = now_ms.saturating_add(TTL_MS);
                let entry = (now_ms, e.id);
                if let Err(at) = list.value.binary_search(&entry) {
                    list.value.insert(at, entry);
                }
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
                let over = list.value.len().saturating_sub(MAX_EVENTS_PER_TOOL);
                for (ts, id) in list.value.drain(..over) {
                    by_cap.insert(id);
                    cut_in_window |= window_from_ms.is_some_and(|w| ts >= w);
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

    /// Write the `c:` link of a call under one client id. An existing link for a different event is
    /// replaced: session, tool and event id change, and `result_written` is false again (ruling
    /// T7-b). A live link for the same event keeps `result_written`, so a settle retry does not
    /// re-open a written result (ruling T7-f).
    fn put_call_link(&mut self, org: Uuid, principal: &str, client_id: &str, mut link: CallLink, now_ms: u64) {
        let key = (org, principal.to_string(), link_digest(client_id));
        let clock = self.clock_ms;
        if let Some(old) = self.calls.get(&key).filter(|e| e.alive(clock) && e.value.event_id == link.event_id) {
            link.result_written = old.value.result_written;
        }
        self.calls.insert(key, Exp::new(link, now_ms));
    }

    fn call_link(&self, org: Uuid, principal: &str, client_id: &str, clock_ms: u64) -> Option<&Exp<CallLink>> {
        self.calls.get(&(org, principal.to_string(), link_digest(client_id))).filter(|e| e.alive(clock_ms))
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
    /// what is present. A body that expires at the same instant as its list entry is still read.
    pub fn events(&self, org: Uuid, scope: Scope, scope_id: &str, tool: &str) -> Vec<StoredEvent> {
        let st = self.lock();
        let key = (hkey(org, scope, scope_id.to_string()), tool.to_string());
        let Some(list) = st.lists.get(&key).filter(|l| l.alive(st.clock_ms)) else {
            return Vec::new();
        };
        list.value
            .iter()
            .filter_map(|(ts, id)| {
                let body = st.bodies.get(&(org, *id))?;
                Some(StoredEvent {
                    id: *id,
                    ts_ms: *ts,
                    kind: body.value.kind,
                    tool: body.value.tool.clone(),
                    verdict: body.value.verdict,
                })
            })
            .collect()
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
        out
    }
}

#[async_trait::async_trait]
impl SessionStore for MemorySessionStore {
    async fn lookup(&self, req: &LookupRequest) -> Result<LookupReply, HistoryStoreError> {
        let st = self.lock();
        let clock = st.clock_ms.max(req.now_ms);
        let get = |id: &String| st.call_link(req.org, &req.principal, id, clock).map(|e| e.value.clone());
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
            let clock = st.clock_ms;
            let key = (h.org, h.principal.clone(), link_digest(&r.call_id));
            let Some(link) = st.calls.get_mut(&key).filter(|e| e.alive(clock)) else { continue };
            if link.value.result_written {
                continue;
            }
            link.value.result_written = true;
            link.exp_ms = req.now_ms.saturating_add(TTL_MS);
            events.push(NewEvent {
                id: r.id,
                tool: r.tool.clone(),
                kind: EventKind::Result,
                verdict: EventVerdict::Allow,
            });
        }
        let reply = if events.is_empty() {
            AppendReply {
                written: 0,
                trimmed_retention: 0,
                trimmed_cap: 0,
                truncated: Vec::new(),
            }
        } else {
            st.add_events(h, &events, req.now_ms, None)
        };
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
            })
            .collect();
        let reply = st.add_events(h, &events, req.now_ms, req.window_from_ms);
        for c in &req.calls {
            st.decisions.insert((h.org, h.principal.clone(), c.id), Exp::new(h.session, req.now_ms));
        }
        st.touch_session(h.org, h.session, req.turn, req.now_ms);
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
            if item.verdict != EventVerdict::Allow {
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
                verdict: EventVerdict::Allow,
            })
            .collect();
        let reply = st.add_events(h, &events, req.now_ms, None);
        for c in &req.calls {
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
    use crate::domain::tool_history::{CallEvent, ForwardedCall};

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
            calls: vec![ForwardedCall {
                id: Uuid::from_u128(3),
                tool: "send".into(),
                call_id: Some("c1".into()),
                item_id: None,
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
                calls: vec![CallEvent {
                    id: Uuid::from_u128(4),
                    tool: "send".into(),
                }],
                now_ms: later,
                window_from_ms: None,
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
