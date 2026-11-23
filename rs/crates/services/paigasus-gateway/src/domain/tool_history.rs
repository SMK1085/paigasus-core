// SPDX-License-Identifier: Apache-2.0

//! SMA-753: the session history store port and the linking rule of D4.
//!
//! No dependency on `redis`, `tokio` or `metrics`. Both store adapters implement `SessionStore`
//! (the memory adapter and the Redis adapter). No raw client id becomes part of a key or a metric
//! label: `link_digest` is the only form of an id that an adapter may put in a key (D4).

use std::collections::{HashMap, HashSet};

use paigasus_gateway_core::ToolCall;
use paigasus_kernel::tool_policy::normalise;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::domain::limits::UnavailableKind;

/// The most events that one history keeps for one tool.
pub const MAX_EVENTS_PER_TOOL: usize = 10_000;
/// The longest window a policy can ask for: 24 hours.
pub const MAX_WINDOW_MS: u64 = 86_400_000;
/// How long an event stays: the longest window plus one hour of margin.
pub const RETENTION_MS: u64 = MAX_WINDOW_MS + 3_600_000;
/// The time to live of a history key, in seconds. It follows `RETENTION_MS`.
pub const KEY_TTL_SECS: u64 = RETENTION_MS / 1000;

/// Which history an event belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Session,
    Principal,
    Key,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Session => "s",
            Scope::Principal => "p",
            Scope::Key => "k",
        }
    }
}

/// The kind of an event. The names are the Dogwood names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Call,
    Result,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Call => "request",
            EventKind::Result => "response",
        }
    }
}

/// The verdict that an event records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventVerdict {
    Pending,
    Allow,
    Deny,
}

impl EventVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            EventVerdict::Pending => "pending",
            EventVerdict::Allow => "allow",
            EventVerdict::Deny => "deny",
        }
    }
}

/// The histories that one request writes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histories {
    pub org: Uuid,
    pub principal: String,
    pub key: Option<String>,
    pub session: Uuid,
}

impl Histories {
    /// (Session, session id), (Principal, principal), and (Key, key id) only when `key` is `Some`.
    /// With no API key the key history is the principal history, written once (D5).
    pub fn scopes(&self) -> Vec<(Scope, String)> {
        let mut scopes = vec![(Scope::Session, self.session.to_string()), (Scope::Principal, self.principal.clone())];
        if let Some(key) = &self.key {
            scopes.push((Scope::Key, key.clone()));
        }
        scopes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
    pub id: Uuid,
    pub ts_ms: u64,
    pub kind: EventKind,
    pub tool: String,
    pub verdict: EventVerdict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupRequest {
    pub org: Uuid,
    pub principal: String,
    pub call_ids: Vec<String>,
    pub result_call_ids: Vec<String>,
    pub refs: Vec<Uuid>,
    /// The time of the read. The memory adapter judges expiry at `max(its clock, now_ms)`. The Redis
    /// adapter ignores it: the server TTL decides.
    pub now_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallLink {
    pub session: Uuid,
    pub tool: String,
    pub event_id: Uuid,
    /// The linked EVENT has its result (the guard lives on the event, not on the link). So the
    /// two aliases of a local shell call share one flag, and a relink to a new event reads the
    /// flag of the new event.
    pub result_written: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionState {
    pub id: Uuid,
    pub last_ms: u64,
    pub turn: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupReply {
    pub calls: Vec<Option<CallLink>>,
    pub results: Vec<Option<CallLink>>,
    pub refs: Vec<Option<Uuid>>,
    pub sessions: Vec<SessionState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultEvent {
    pub id: Uuid,
    pub call_id: String,
    pub tool: String,
    /// The call event that the lookup found for `call_id`. `write` guards the result on this
    /// event: one result per event, whatever the link holds at write time.
    pub event_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteRequest {
    pub histories: Histories,
    pub turn: Option<i64>,
    pub results: Vec<ResultEvent>,
    pub now_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallEvent {
    pub id: Uuid,
    pub tool: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReserveRequest {
    pub histories: Histories,
    pub turn: i64,
    pub calls: Vec<CallEvent>,
    pub now_ms: u64,
    pub window_from_ms: Option<u64>,
}

/// A call that the gateway forwarded with no decision. `call_id` and `item_id` are the two `c:`
/// links of the call: a local shell call has both (spec D4). Each is written when it is `Some`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardedCall {
    pub id: Uuid,
    pub tool: String,
    pub call_id: Option<String>,
    pub item_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendRequest {
    pub histories: Histories,
    pub turn: i64,
    pub calls: Vec<ForwardedCall>,
    pub now_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendReply {
    pub written: usize,
    pub trimmed_retention: u64,
    pub trimmed_cap: u64,
    /// The histories that the cap cut, as (scope, tool name). The `String` is the tool name (the
    /// `name_key`), not a digest. A store adapter that keys by digest maps the digest back to the
    /// name from the calls of the request.
    pub truncated: Vec<(Scope, String)>,
}

/// `item_id`: the item id of a forwarded local shell call. The `c:` link is written under `call_id`
/// and also under `item_id`, with the same session, tool and event id (spec D4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettleItem {
    pub event_id: Uuid,
    pub verdict: EventVerdict,
    pub tool: String,
    pub call_id: Option<String>,
    pub item_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettleRequest {
    pub org: Uuid,
    pub principal: String,
    pub session: Uuid,
    pub items: Vec<SettleItem>,
    pub now_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HistoryStoreError {
    /// `detail` is the store error's text. It never contains the store URL.
    #[error("history store unavailable ({kind:?}): {detail}")]
    Unavailable { kind: UnavailableKind, detail: String },
}

/// The session history store port. Consumed as `dyn SessionStore`.
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// Read only. One answer per id, in order. A reply of the wrong length is an error (`link_session` refuses it). `sessions` holds the state of every session that a link names.
    async fn lookup(&self, req: &LookupRequest) -> Result<LookupReply, HistoryStoreError>;
    /// After the admission: touch the session when `turn` is Some, and write each result event once
    /// per linked call event (`ResultEvent::event_id`). A call event that is gone gets no result.
    async fn write(&self, req: &WriteRequest) -> Result<AppendReply, HistoryStoreError>;
    /// Inside the decide future: one pending call event per call in every history, and its `d:` link.
    async fn reserve(&self, req: &ReserveRequest) -> Result<AppendReply, HistoryStoreError>;
    /// At the report site: set each verdict; write the `c:` link of each forwarded call. Idempotent.
    async fn settle(&self, req: &SettleRequest) -> Result<(), HistoryStoreError>;
    /// At the report site, for a call forwarded with no decision: an allowed event and its `c:` link.
    async fn append(&self, req: &AppendRequest) -> Result<AppendReply, HistoryStoreError>;
}

/// SHA-256 of the id, as 64 lower-case hex digits (D4 "Key names"). The id is client input of any
/// length and any bytes, so no key and no label uses it raw.
pub fn link_digest(id: &str) -> String {
    Sha256::digest(id.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The history tool of a call: `normalise(&call.display_name())`. The catalog stores normalised names.
pub fn history_tool(call: &ToolCall) -> String {
    normalise(&call.display_name())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedResult {
    pub call_id: String,
    pub tool: String,
    /// The call event of the link at lookup time. The write guards the result on it.
    pub event_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLink {
    pub session: Uuid,
    pub turn: i64,
    pub joined: bool,
    pub mixed_sessions: bool,
    pub forged_results: usize,
    pub results: Vec<LinkedResult>,
}

/// The linking rule of D4. `new_session` is used when no id matches.
///
/// A live link can come back with no session state row in `reply.sessions`: the session row can
/// expire before the link. The link still names the session, so the request joins it, and the
/// turn is 1 because no state holds a turn to count on.
pub fn link_session(req: &LookupRequest, reply: &LookupReply, new_session: Uuid) -> Result<SessionLink, HistoryStoreError> {
    check_len("calls", req.call_ids.len(), reply.calls.len())?;
    check_len("results", req.result_call_ids.len(), reply.results.len())?;
    check_len("refs", req.refs.len(), reply.refs.len())?;
    let mut candidates: HashSet<Uuid> = HashSet::new();
    for link in reply.calls.iter().chain(reply.results.iter()).flatten() {
        candidates.insert(link.session);
    }
    candidates.extend(reply.refs.iter().flatten().copied());

    let states: HashMap<Uuid, &SessionState> = reply.sessions.iter().map(|s| (s.id, s)).collect();
    let last_ms = |id: &Uuid| states.get(id).map_or(0, |s| s.last_ms);
    let chosen = candidates.iter().copied().max_by(|a, b| last_ms(a).cmp(&last_ms(b)).then(a.cmp(b)));

    let (session, turn, joined) = match chosen {
        Some(id) => (id, states.get(&id).map_or(1, |s| s.turn.saturating_add(1)), true),
        None => (new_session, 1, false),
    };

    let forged_results = reply.results.iter().filter(|r| r.is_none()).count();
    // One result per call EVENT: a call id sent twice, and the two aliases of a local shell call,
    // name the same event.
    let mut seen: HashSet<Uuid> = HashSet::new();
    let mut results = Vec::new();
    for (call_id, link) in req.result_call_ids.iter().zip(reply.results.iter()) {
        let Some(link) = link else { continue };
        if link.result_written || !seen.insert(link.event_id) {
            continue;
        }
        results.push(LinkedResult {
            call_id: call_id.clone(),
            tool: link.tool.clone(),
            event_id: link.event_id,
        });
    }

    Ok(SessionLink {
        session,
        turn,
        joined,
        mixed_sessions: candidates.len() > 1,
        forged_results,
        results,
    })
}

/// A reply must hold one answer per id. A short or long reply is a store error, never a partial answer.
fn check_len(what: &str, want: usize, got: usize) -> Result<(), HistoryStoreError> {
    if want == got {
        return Ok(());
    }
    Err(HistoryStoreError::Unavailable {
        kind: UnavailableKind::Decode,
        detail: format!("lookup reply holds {got} {what} for {want} ids"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }
    fn req() -> LookupRequest {
        LookupRequest {
            org: u(1),
            principal: "p".into(),
            call_ids: vec![],
            result_call_ids: vec![],
            refs: vec![],
            now_ms: 0,
        }
    }
    fn link(session: u128, tool: &str, written: bool) -> CallLink {
        CallLink {
            session: u(session),
            tool: tool.into(),
            event_id: u(99),
            result_written: written,
        }
    }
    fn state(id: u128, last_ms: u64, turn: i64) -> SessionState {
        SessionState { id: u(id), last_ms, turn }
    }
    fn reply() -> LookupReply {
        LookupReply {
            calls: vec![],
            results: vec![],
            refs: vec![],
            sessions: vec![],
        }
    }

    /// A request whose id lists have the length of the reply, so the length check passes.
    fn go(r: &LookupReply) -> SessionLink {
        let mut q = req();
        q.call_ids = (0..r.calls.len()).map(|i| format!("c{i}")).collect();
        q.result_call_ids = (0..r.results.len()).map(|i| format!("r{i}")).collect();
        q.refs = (0..r.refs.len()).map(|i| u(1000 + i as u128)).collect();
        link_session(&q, r, u(7)).unwrap()
    }

    fn is_decode(e: HistoryStoreError) -> bool {
        matches!(e, HistoryStoreError::Unavailable { kind: UnavailableKind::Decode, .. })
    }

    #[test]
    fn a_short_results_reply_is_a_store_error_not_a_forged_count() {
        let mut q = req();
        q.result_call_ids = vec!["x".into(), "y".into()];
        let mut r = reply();
        r.results = vec![Some(link(5, "a", false))];
        assert!(is_decode(link_session(&q, &r, u(7)).unwrap_err()));
    }

    #[test]
    fn a_long_results_reply_is_a_store_error() {
        let mut q = req();
        q.result_call_ids = vec!["x".into()];
        let mut r = reply();
        r.results = vec![None, None];
        assert!(is_decode(link_session(&q, &r, u(7)).unwrap_err()));
    }

    #[test]
    fn a_short_calls_reply_is_a_store_error() {
        let mut q = req();
        q.call_ids = vec!["a".into(), "b".into()];
        let mut r = reply();
        r.calls = vec![Some(link(5, "a", false))];
        assert!(is_decode(link_session(&q, &r, u(7)).unwrap_err()));
    }

    #[test]
    fn a_short_refs_reply_is_a_store_error() {
        let mut q = req();
        q.refs = vec![u(1), u(2)];
        let mut r = reply();
        r.refs = vec![Some(u(5))];
        assert!(is_decode(link_session(&q, &r, u(7)).unwrap_err()));
    }

    #[test]
    fn a_duplicate_unlinked_result_counts_once_per_entry() {
        let mut r = reply();
        r.results = vec![None, None];
        // The request names the same id twice, as a client can.
        let mut q = req();
        q.result_call_ids = vec!["x".into(), "x".into()];
        let l = link_session(&q, &r, u(7)).unwrap();
        assert_eq!(l.forged_results, 2);
        assert!(l.results.is_empty());
    }

    #[test]
    fn a_second_session_from_a_ref_alone_flags_mixed_sessions() {
        let mut r = reply();
        r.calls = vec![Some(link(5, "a", false))];
        r.refs = vec![Some(u(6))];
        r.sessions = vec![state(5, 100, 1), state(6, 50, 1)];
        let l = go(&r);
        assert_eq!((l.session, l.mixed_sessions), (u(5), true));
    }

    #[test]
    fn no_match_starts_a_new_session_at_turn_1() {
        let l = go(&reply());
        assert_eq!((l.session, l.turn, l.joined, l.mixed_sessions, l.forged_results), (u(7), 1, false, false, 0));
        assert!(l.results.is_empty());
    }

    #[test]
    fn all_matches_in_one_session_join_it_with_the_next_turn() {
        let mut r = reply();
        r.calls = vec![Some(link(5, "a", false)), Some(link(5, "b", false))];
        r.sessions = vec![state(5, 100, 3)];
        let l = go(&r);
        assert_eq!((l.session, l.turn, l.joined, l.mixed_sessions), (u(5), 4, true, false));
    }

    #[test]
    fn matches_in_two_sessions_join_the_most_recent_and_flag_mixed_sessions() {
        let mut r = reply();
        r.calls = vec![Some(link(5, "a", false)), Some(link(6, "b", false))];
        r.sessions = vec![state(5, 100, 3), state(6, 200, 8)];
        let l = go(&r);
        assert_eq!((l.session, l.turn, l.mixed_sessions), (u(6), 9, true));
    }

    #[test]
    fn a_tie_goes_to_the_larger_uuid() {
        let mut r = reply();
        r.calls = vec![Some(link(6, "a", false)), Some(link(5, "b", false))];
        r.sessions = vec![state(5, 100, 1), state(6, 100, 1)];
        assert_eq!(go(&r).session, u(6));
    }

    #[test]
    fn a_session_without_state_still_joins_with_last_ms_zero() {
        let mut r = reply();
        r.calls = vec![Some(link(5, "a", false))];
        let l = go(&r);
        assert_eq!((l.session, l.turn, l.joined), (u(5), 1, true));
        r.calls.push(Some(link(4, "b", false)));
        r.sessions = vec![state(4, 1, 2)];
        let l = go(&r);
        assert_eq!((l.session, l.turn), (u(4), 3));
    }

    #[test]
    fn an_unlinked_result_counts_as_forged_and_writes_nothing() {
        let mut q = req();
        q.result_call_ids = vec!["x".into(), "y".into()];
        let mut r = reply();
        r.results = vec![None, Some(link(5, "a", false))];
        let l = link_session(&q, &r, u(7)).unwrap();
        assert_eq!(l.forged_results, 1);
        assert_eq!(
            l.results,
            vec![LinkedResult {
                call_id: "y".into(),
                tool: "a".into(),
                event_id: u(99),
            }]
        );
    }

    #[test]
    fn a_ref_alone_joins_its_session() {
        let mut r = reply();
        r.refs = vec![Some(u(5))];
        r.sessions = vec![state(5, 10, 2)];
        let l = go(&r);
        assert_eq!((l.session, l.turn, l.joined), (u(5), 3, true));
    }

    #[test]
    fn a_result_sent_twice_in_one_request_is_linked_once() {
        let mut q = req();
        q.result_call_ids = vec!["x".into(), "x".into()];
        let mut r = reply();
        r.results = vec![Some(link(5, "a", false)), Some(link(5, "a", false))];
        assert_eq!(link_session(&q, &r, u(7)).unwrap().results.len(), 1);
    }

    #[test]
    fn an_already_written_result_is_not_written_again() {
        let mut q = req();
        q.result_call_ids = vec!["x".into()];
        let mut r = reply();
        r.results = vec![Some(link(5, "a", true))];
        let l = link_session(&q, &r, u(7)).unwrap();
        assert!(l.results.is_empty());
        assert_eq!(l.forged_results, 0);
    }

    #[test]
    fn two_aliases_of_one_call_event_link_one_result() {
        let mut q = req();
        q.result_call_ids = vec!["call-1".into(), "item-1".into(), "other".into()];
        let mut r = reply();
        let mut other = link(5, "a", false);
        other.event_id = u(98);
        r.results = vec![Some(link(5, "shell", false)), Some(link(5, "shell", false)), Some(other)];
        let ids: Vec<Uuid> = link_session(&q, &r, u(7)).unwrap().results.iter().map(|l| l.event_id).collect();
        assert_eq!(ids, vec![u(99), u(98)]);
    }

    #[test]
    fn the_result_tool_comes_from_the_link() {
        let mut q = req();
        q.result_call_ids = vec!["x".into()];
        let mut r = reply();
        r.results = vec![Some(link(5, "from_link", false))];
        assert_eq!(link_session(&q, &r, u(7)).unwrap().results[0].tool, "from_link");
    }

    #[test]
    fn link_digest_is_64_lower_hex_for_any_id() {
        for id in ["c1", &"x".repeat(10_000), "ü:*?\n", ""] {
            let d = link_digest(id);
            assert_eq!(d.len(), 64);
            assert!(d.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        }
        assert_eq!(link_digest(""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn a_keyless_caller_has_two_histories_and_a_key_caller_three() {
        let mut h = Histories {
            org: u(1),
            principal: "prn:p".into(),
            key: None,
            session: u(2),
        };
        assert_eq!(h.scopes(), vec![(Scope::Session, u(2).to_string()), (Scope::Principal, "prn:p".into())]);
        h.key = Some("k1".into());
        let s = h.scopes();
        assert_eq!(s.len(), 3);
        assert_eq!(s[2], (Scope::Key, "k1".into()));
        assert_eq!((Scope::Session.as_str(), Scope::Principal.as_str(), Scope::Key.as_str()), ("s", "p", "k"));
    }

    #[test]
    fn retention_is_25_hours_and_the_ttl_follows_it() {
        assert_eq!(RETENTION_MS, 90_000_000);
        assert_eq!(KEY_TTL_SECS, 90_000);
    }

    #[test]
    fn enum_names_are_the_dogwood_names() {
        assert_eq!((EventKind::Call.as_str(), EventKind::Result.as_str()), ("request", "response"));
        assert_eq!(
            (EventVerdict::Pending.as_str(), EventVerdict::Allow.as_str(), EventVerdict::Deny.as_str()),
            ("pending", "allow", "deny")
        );
    }
}
