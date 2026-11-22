// SPDX-License-Identifier: Apache-2.0

//! `RedisSessionStore` (SMA-753 spec § 7, D4, D5, D8, D11): the session history on a shared Redis,
//! so every replica that points at it sees the same histories and links.
//!
//! Each operation is ONE Lua script call. Redis runs a script atomically, so two concurrent
//! requests on two replicas see each other: a result guard, a trim or a session touch never
//! interleaves with another. Every instant comes from the gateway's clock; no script calls `TIME`.
//! Rust builds every key name and passes it in `KEYS`. The one exception is the `s:` key of a
//! session that a link names, which `LOOKUP_SCRIPT` builds from a prefix in `ARGV`. That is
//! allowed because Redis Cluster is not supported (N4).
//!
//! Layout. A client string in a key name is a digest (`link_digest`), never the raw string:
//! - `h:<org>:<s|p|k>:<sha256(scope id)>:<sha256(tool)>`: a sorted set, score `ts_ms`, member the event id.
//! - `t:<org>:<s|p|k>:<sha256(scope id)>`: the set of tool digests of a history.
//! - `e:<org>:<event id>`: a hash, `b` = the JSON body, `v` = the verdict.
//! - `l:<org>:<sha256(principal)>:c:<sha256(call id)>`: a hash, `s`, `t`, `e`, and `r` once the result exists.
//! - `l:<org>:<sha256(principal)>:d:<decision id>`: a string, the session.
//! - `s:<org>:<session>`: a hash, `last_ms` and `turn`.
//!
//! Every key gets `EXPIRE KEY_TTL_SECS` on each write. A trim never deletes an `e:` body: another
//! history can still hold the event, and the body expires by its TTL.
//!
//! The connection is `paigasus_redis::connect` with the SMA-476 breaker; its series carry the
//! gateway's names and `role="tool_history"`.

use std::sync::Arc;

use paigasus_observability::names;
use paigasus_redis::{BreakerMetrics, RedisHandle};
use serde::Deserialize;
use uuid::Uuid;

use crate::domain::limits::UnavailableKind;
use crate::domain::tool_history::{
    AppendReply, AppendRequest, CallLink, EventKind, EventVerdict, Histories, HistoryStoreError, KEY_TTL_SECS, LookupReply, LookupRequest, MAX_EVENTS_PER_TOOL, RETENTION_MS, ReserveRequest, Scope,
    SessionState, SessionStore, SettleRequest, StoredEvent, WriteRequest, link_digest,
};

/// Every key starts with this. `v1` lets a later format use new keys with no migration.
pub const KEY_PREFIX: &str = "paigasus:gateway:history:v1:";

/// The `role` label of the breaker series of this connection.
pub const HISTORY_BREAKER_ROLE: &str = "tool_history";

/// The version of the event body encoding. A reader ignores unknown fields (rolling upgrade).
const BODY_VERSION: u64 = 1;

/// The `c:` link rule, shared by `APPEND_SCRIPT` and `SETTLE_SCRIPT`.
macro_rules! call_link_lua {
    () => {
        r#"
-- Write the c: link of a call. A link for a DIFFERENT event is replaced and its result guard `r`
-- is cleared (ruling T7-b). A link for the SAME event keeps `r`, so a settle retry does not
-- re-open a written result (ruling T7-f).
local function put_call_link(key, session, tool, id, ttl)
  if redis.call('HGET', key, 'e') ~= id then
    redis.call('HDEL', key, 'r')
  end
  redis.call('HSET', key, 's', session, 't', tool, 'e', id)
  redis.call('EXPIRE', key, ttl)
end
"#
    };
}

/// Shared by `write`, `reserve` and `append`. Redis 6.2 commands only.
const APPEND_SCRIPT: &str = concat!(
    call_link_lua!(),
    r#"
-- ARGV: now_ms, retention_ms, cap, ttl_secs, window_from ('' = none), turn ('' = no touch),
--       session, verdict, guard ('0'|'1'), link kind ('-'|'d'|'c'), H, T, N,
--       then per tool (2 values): tool digest, tool name,
--       then per event (4 values): id, body, tool index (1..T), link count.
-- KEYS: s-key, H t-keys, T * H h-keys (tool t, history i at 1 + H + (t - 1) * H + i),
--       then per event: e-key, the guard key when guard = '1', then `link count` link keys
--       (a local shell call has two c: links, F7).
-- A history key and a tool go once per distinct tool, not once per event: a reserve of 10,000
-- events must stay small on the wire.
local now = tonumber(ARGV[1])
local cutoff = now - tonumber(ARGV[2])
local cap = tonumber(ARGV[3])
local ttl = tonumber(ARGV[4])
local window_from = ARGV[5]
local turn = ARGV[6]
local session = ARGV[7]
local verdict = ARGV[8]
local guard = ARGV[9]
local link = ARGV[10]
local H = tonumber(ARGV[11])
local T = tonumber(ARGV[12])
local N = tonumber(ARGV[13])

local digests, names, members = {}, {}, {}
for t = 1, T do
  digests[t] = ARGV[13 + 2 * t - 1]
  names[t] = ARGV[13 + 2 * t]
  members[t] = {}
end
local function hkey(t, i)
  return KEYS[1 + H + (t - 1) * H + i]
end

local written = 0
local k, a = 1 + H + T * H, 13 + 2 * T
for j = 1, N do
  local id, body, t, nlinks = ARGV[a + 1], ARGV[a + 2], tonumber(ARGV[a + 3]), tonumber(ARGV[a + 4])
  a = a + 4
  k = k + 1
  local ekey = KEYS[k]
  local skip = false
  if guard == '1' then
    k = k + 1
    local gkey = KEYS[k]
    -- F5: a link that expired before this write gets no result and no new key.
    if redis.call('EXISTS', gkey) == 0 then
      skip = true
    -- The result is already written: write it once per call id.
    elseif redis.call('HSETNX', gkey, 'r', id) == 0 then
      skip = true
    else
      redis.call('EXPIRE', gkey, ttl)
    end
  end
  local lbase = k
  k = k + nlinks
  if not skip then
    redis.call('HSET', ekey, 'b', body, 'v', verdict)
    redis.call('EXPIRE', ekey, ttl)
    table.insert(members[t], id)
    for l = 1, nlinks do
      local lkey = KEYS[lbase + l]
      if link == 'd' then
        redis.call('SET', lkey, session, 'EX', ttl)
      elseif link == 'c' then
        put_call_link(lkey, session, names[t], id, ttl)
      end
    end
    written = written + 1
  end
end

-- Add the events of each tool to every history, in batches, then trim each list once. The counts
-- are of DISTINCT events (T7-a): an event cut from two histories counts once. No e: body is
-- deleted: it expires by its TTL.
local trimmed_retention, trimmed_cap = 0, 0
local retention_seen, cap_seen = {}, {}
local flagged = {}
local tools_touched = false
local cut = '(' .. string.format('%.0f', cutoff)   -- strictly older than the retention
for t = 1, T do
  local m = members[t]
  if #m > 0 then
    tools_touched = true
    for i = 1, H do
      local key = hkey(t, i)
      for from = 1, #m, 1000 do
        local batch = {}
        for x = from, math.min(from + 999, #m) do
          batch[#batch + 1] = ARGV[1]
          batch[#batch + 1] = m[x]
        end
        redis.call('ZADD', key, unpack(batch))
      end
      redis.call('SADD', KEYS[1 + i], digests[t])
      for _, old in ipairs(redis.call('ZRANGEBYSCORE', key, '-inf', cut)) do
        if not retention_seen[old] then
          retention_seen[old] = true
          trimmed_retention = trimmed_retention + 1
        end
      end
      redis.call('ZREMRANGEBYSCORE', key, '-inf', cut)
      local over = redis.call('ZCARD', key) - cap
      if over > 0 then
        local popped = redis.call('ZPOPMIN', key, over)
        local flag = false
        for p = 1, #popped, 2 do
          if not cap_seen[popped[p]] then
            cap_seen[popped[p]] = true
            trimmed_cap = trimmed_cap + 1
          end
          if window_from ~= '' and tonumber(popped[p + 1]) >= tonumber(window_from) then
            flag = true
          end
        end
        if flag then
          table.insert(flagged, tostring(i))
          table.insert(flagged, tostring(t))
        end
      end
      redis.call('EXPIRE', key, ttl)
    end
  end
end
if tools_touched then
  for i = 1, H do
    redis.call('EXPIRE', KEYS[1 + i], ttl)
  end
end

if turn ~= '' then
  local skey = KEYS[1]
  local last = redis.call('HGET', skey, 'last_ms')
  if not last or tonumber(last) < now then
    redis.call('HSET', skey, 'last_ms', ARGV[1])
  end
  local t = redis.call('HGET', skey, 'turn')
  if not t or tonumber(t) < tonumber(turn) then
    redis.call('HSET', skey, 'turn', turn)
  end
  redis.call('EXPIRE', skey, ttl)
end

local out = {tostring(written), tostring(trimmed_retention), tostring(trimmed_cap)}
for _, v in ipairs(flagged) do
  table.insert(out, v)
end
return out
"#
);

/// Read only.
const LOOKUP_SCRIPT: &str = r#"
-- ARGV: the number of c: keys (calls, then results), the number of d: keys, the s: key prefix.
-- KEYS: the c: keys, then the d: keys.
local nc, nd, prefix = tonumber(ARGV[1]), tonumber(ARGV[2]), ARGV[3]
local links, refs, sessions, named, order = {}, {}, {}, {}, {}
local function name(s)
  if s and not named[s] then
    named[s] = true
    table.insert(order, s)
  end
end
for i = 1, nc do
  local f = redis.call('HMGET', KEYS[i], 's', 't', 'e', 'r')
  links[i] = f
  name(f[1])
end
for i = 1, nd do
  local s = redis.call('GET', KEYS[nc + i])
  refs[i] = s
  name(s)
end
for _, s in ipairs(order) do
  local f = redis.call('HMGET', prefix .. s, 'last_ms', 'turn')
  if f[1] then
    table.insert(sessions, {s, f[1], f[2] or '0'})
  end
end
return {links, refs, sessions}
"#;

const SETTLE_SCRIPT: &str = concat!(
    call_link_lua!(),
    r#"
-- ARGV: ttl_secs, session, N, then per item (4 values): event id, verdict, tool name, link count.
-- KEYS: per item: e-key, then `link count` c: keys (the call id and the item id, F7).
local ttl, session, N = tonumber(ARGV[1]), ARGV[2], tonumber(ARGV[3])
local k, a = 0, 3
for j = 1, N do
  local id, verdict, tool, nlinks = ARGV[a + 1], ARGV[a + 2], ARGV[a + 3], tonumber(ARGV[a + 4])
  a = a + 4
  local ekey = KEYS[k + 1]
  local lbase = k + 1
  k = k + 1 + nlinks
  if redis.call('EXISTS', ekey) == 1 then
    -- The first settle wins (T7-c).
    if redis.call('HGET', ekey, 'v') == 'pending' then
      redis.call('HSET', ekey, 'v', verdict)
    end
    redis.call('EXPIRE', ekey, ttl)
    if verdict == 'allow' then
      for l = 1, nlinks do
        put_call_link(KEYS[lbase + l], session, tool, id, ttl)
      end
    end
  end
end
return 0
"#
);

pub fn history_key(org: Uuid, scope: Scope, scope_id: &str, tool: &str) -> String {
    format!("{KEY_PREFIX}h:{org}:{}:{}:{}", scope.as_str(), link_digest(scope_id), link_digest(tool))
}

fn tools_key(org: Uuid, scope: Scope, scope_id: &str) -> String {
    format!("{KEY_PREFIX}t:{org}:{}:{}", scope.as_str(), link_digest(scope_id))
}

pub fn event_key(org: Uuid, id: Uuid) -> String {
    format!("{KEY_PREFIX}e:{org}:{id}")
}

pub fn call_link_key(org: Uuid, principal: &str, call_id: &str) -> String {
    format!("{KEY_PREFIX}l:{org}:{}:c:{}", link_digest(principal), link_digest(call_id))
}

pub fn decision_link_key(org: Uuid, principal: &str, id: Uuid) -> String {
    format!("{KEY_PREFIX}l:{org}:{}:d:{id}", link_digest(principal))
}

pub fn session_key(org: Uuid, session: Uuid) -> String {
    format!("{KEY_PREFIX}s:{org}:{session}")
}

fn encode_body(id: Uuid, ts_ms: u64, kind: EventKind, tool: &str) -> String {
    serde_json::json!({ "v": BODY_VERSION, "id": id.to_string(), "ts": ts_ms, "k": kind.as_str(), "t": tool }).to_string()
}

/// The JSON body. serde ignores unknown keys, so a body of a later version still decodes.
#[derive(Deserialize)]
struct Body {
    v: u64,
    id: String,
    ts: u64,
    k: String,
    t: String,
}

fn decode_error(detail: impl Into<String>) -> HistoryStoreError {
    HistoryStoreError::Unavailable {
        kind: UnavailableKind::Decode,
        detail: detail.into(),
    }
}

/// Decode the fields of an `e:` hash (`b` = the JSON body, `v` = the verdict). Unknown fields of
/// the hash and unknown keys of the body are ignored (rolling upgrade).
pub fn decode_event(fields: &[(String, String)]) -> Result<StoredEvent, HistoryStoreError> {
    let field = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
    let raw = field("b").ok_or_else(|| decode_error("event body has no `b` field"))?;
    let body: Body = serde_json::from_str(raw).map_err(|_| decode_error("event body is not valid JSON"))?;
    if body.v < BODY_VERSION {
        return Err(decode_error(format!("event body has version {}", body.v)));
    }
    let id = Uuid::try_parse(&body.id).map_err(|_| decode_error("event body id is not a uuid"))?;
    let kind = match body.k.as_str() {
        "request" => EventKind::Call,
        "response" => EventKind::Result,
        _ => return Err(decode_error("event body has an unknown kind")),
    };
    let verdict = parse_verdict(field("v").ok_or_else(|| decode_error("event has no `v` field"))?)?;
    Ok(StoredEvent {
        id,
        ts_ms: body.ts,
        kind,
        tool: body.t,
        verdict,
    })
}

fn parse_verdict(raw: &str) -> Result<EventVerdict, HistoryStoreError> {
    match raw {
        "pending" => Ok(EventVerdict::Pending),
        "allow" => Ok(EventVerdict::Allow),
        "deny" => Ok(EventVerdict::Deny),
        _ => Err(decode_error("event has an unknown verdict")),
    }
}

/// The breaker series carry the gateway's names and the role `tool_history`.
pub fn breaker_metrics() -> BreakerMetrics {
    BreakerMetrics {
        state: names::GATEWAY_REDIS_BREAKER_STATE,
        transitions: names::GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL,
        role: HISTORY_BREAKER_ROLE,
    }
}

/// The rule of `adapters/limits/redis.rs`: `Io` for a connection failure and the breaker's
/// short-circuit, `Decode` for a reply of the wrong type, `Server` for every other answer.
fn unavailable_kind(err: &redis::RedisError) -> UnavailableKind {
    if err.is_io_error() {
        return UnavailableKind::Io;
    }
    match err.kind() {
        redis::ErrorKind::UnexpectedReturnType | redis::ErrorKind::Parse => UnavailableKind::Decode,
        _ => UnavailableKind::Server,
    }
}

/// The detail is the error kind and the server error code only. Never `err.to_string()`: that
/// text can carry the Redis URL, a host or a password.
fn unavailable(err: &redis::RedisError) -> HistoryStoreError {
    let detail = match err.code() {
        Some(code) => format!("redis error kind={:?} code={code}", err.kind()),
        None => format!("redis error kind={:?}", err.kind()),
    };
    HistoryStoreError::Unavailable { kind: unavailable_kind(err), detail }
}

/// The result of the boot check of `maxmemory-policy` (D11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvictionCheck {
    /// `noeviction`: no history key can be evicted.
    Safe,
    /// A policy that can evict a history key. Every history key has a TTL, so every `allkeys-*`
    /// and every `volatile-*` policy can.
    Evicts(String),
    /// The policy could not be read (`NOPERM`, an unknown command, another error).
    Unknown,
}

/// Classify the reply of `CONFIG GET maxmemory-policy`. Only `noeviction` is `Safe`.
pub fn eviction_check(reply: &redis::RedisResult<Vec<String>>) -> EvictionCheck {
    match reply {
        Ok(pair) => match pair.as_slice() {
            [name, policy] if name == "maxmemory-policy" => {
                if policy == "noeviction" {
                    EvictionCheck::Safe
                } else {
                    EvictionCheck::Evicts(policy.clone())
                }
            }
            _ => EvictionCheck::Unknown,
        },
        Err(_) => EvictionCheck::Unknown,
    }
}

#[derive(Clone, Copy)]
enum LinkKind {
    None,
    Decision,
    Call,
}

impl LinkKind {
    fn as_arg(self) -> &'static str {
        match self {
            LinkKind::None => "-",
            LinkKind::Decision => "d",
            LinkKind::Call => "c",
        }
    }
}

/// One event for `APPEND_SCRIPT`.
struct NewEvent {
    id: Uuid,
    tool: String,
    /// The `c:` key whose `r` guard decides if the event is written. `write` only.
    guard: Option<String>,
    link_keys: Vec<String>,
}

/// One `APPEND_SCRIPT` call. Every event of a call has the same kind, verdict and link kind.
struct Batch<'a> {
    histories: &'a Histories,
    turn: Option<i64>,
    kind: EventKind,
    verdict: EventVerdict,
    link: LinkKind,
    events: Vec<NewEvent>,
    now_ms: u64,
    window_from_ms: Option<u64>,
}

/// The shared-Redis store. `Clone` shares the handle (one breaker) and the scripts.
#[derive(Clone)]
pub struct RedisSessionStore {
    handle: RedisHandle,
    append: Arc<redis::Script>,
    lookup: Arc<redis::Script>,
    settle: Arc<redis::Script>,
}

impl RedisSessionStore {
    /// Call after `paigasus_observability::init`: the breaker sets its gauge in its constructor.
    pub async fn connect(redis_url: &str) -> redis::RedisResult<Self> {
        Ok(Self::from_handle(paigasus_redis::connect(redis_url, breaker_metrics()).await?))
    }

    pub fn from_handle(handle: RedisHandle) -> Self {
        RedisSessionStore {
            handle,
            append: Arc::new(redis::Script::new(APPEND_SCRIPT)),
            lookup: Arc::new(redis::Script::new(LOOKUP_SCRIPT)),
            settle: Arc::new(redis::Script::new(SETTLE_SCRIPT)),
        }
    }

    /// `CONFIG GET maxmemory-policy`, classified by `eviction_check`.
    pub async fn read_eviction_policy(&self) -> EvictionCheck {
        let mut con = self.handle.clone();
        let reply = redis::cmd("CONFIG").arg("GET").arg("maxmemory-policy").query_async::<Vec<String>>(&mut con).await;
        eviction_check(&reply)
    }

    async fn run_append(&self, batch: &Batch<'_>) -> Result<AppendReply, HistoryStoreError> {
        let h = batch.histories;
        if batch.events.is_empty() && batch.turn.is_none() {
            return Ok(empty_reply());
        }
        let scopes = h.scopes();
        // The distinct tools in first-seen order. An event names its tool by index (1..T).
        let mut tools: Vec<&str> = Vec::new();
        for e in &batch.events {
            if !tools.contains(&e.tool.as_str()) {
                tools.push(&e.tool);
            }
        }
        let guarded = batch.events.iter().any(|e| e.guard.is_some());
        let mut inv = self.append.prepare_invoke();
        inv.key(session_key(h.org, h.session));
        for (scope, id) in &scopes {
            inv.key(tools_key(h.org, *scope, id));
        }
        for tool in &tools {
            for (scope, id) in &scopes {
                inv.key(history_key(h.org, *scope, id, tool));
            }
        }
        inv.arg(batch.now_ms)
            .arg(RETENTION_MS)
            .arg(MAX_EVENTS_PER_TOOL)
            .arg(KEY_TTL_SECS)
            .arg(batch.window_from_ms.map(|w| w.to_string()).unwrap_or_default())
            .arg(batch.turn.map(|t| t.to_string()).unwrap_or_default())
            .arg(h.session.to_string())
            .arg(batch.verdict.as_str())
            .arg(if guarded { "1" } else { "0" })
            .arg(batch.link.as_arg())
            .arg(scopes.len())
            .arg(tools.len())
            .arg(batch.events.len());
        for tool in &tools {
            inv.arg(link_digest(tool)).arg(*tool);
        }
        for e in &batch.events {
            inv.key(event_key(h.org, e.id));
            // `guarded` holds for every event of a `write` and for none of a reserve or append.
            if let Some(guard) = &e.guard {
                inv.key(guard);
            }
            for key in &e.link_keys {
                inv.key(key);
            }
            let index = tools.iter().position(|t| *t == e.tool).map_or(0, |i| i + 1);
            inv.arg(e.id.to_string()).arg(encode_body(e.id, batch.now_ms, batch.kind, &e.tool)).arg(index).arg(e.link_keys.len());
        }
        let mut con = self.handle.clone();
        let reply: Vec<String> = inv.invoke_async(&mut con).await.map_err(|err| unavailable(&err))?;
        append_reply(&reply, &scopes, &tools)
    }
}

fn empty_reply() -> AppendReply {
    AppendReply {
        written: 0,
        trimmed_retention: 0,
        trimmed_cap: 0,
        truncated: Vec::new(),
    }
}

/// Map the script reply: three counts, then (history index, tool index) pairs. The key names use
/// the tool digest; the tool index maps back to the tool NAME of the request (F6).
fn append_reply(reply: &[String], scopes: &[(Scope, String)], tools: &[&str]) -> Result<AppendReply, HistoryStoreError> {
    let [written, retention, cap, flagged @ ..] = reply else {
        return Err(decode_error(format!("the append script returned {} values", reply.len())));
    };
    let count = |raw: &String| raw.parse::<u64>().map_err(|_| decode_error("the append script returned a count that is not an integer"));
    let one_based = |raw: &String, len: usize| raw.parse::<usize>().ok().and_then(|i| i.checked_sub(1)).filter(|i| *i < len);
    if flagged.len() % 2 != 0 {
        return Err(decode_error("the append script returned an odd flag list"));
    }
    let mut truncated = Vec::new();
    for pair in flagged.chunks(2) {
        let scope = one_based(&pair[0], scopes.len()).ok_or_else(|| decode_error("the append script returned an unknown history"))?;
        let tool = one_based(&pair[1], tools.len()).ok_or_else(|| decode_error("the append script returned an unknown tool"))?;
        let flag = (scopes[scope].0, tools[tool].to_string());
        if !truncated.contains(&flag) {
            truncated.push(flag);
        }
    }
    Ok(AppendReply {
        written: usize::try_from(count(written)?).map_err(|_| decode_error("the written count is too large"))?,
        trimmed_retention: count(retention)?,
        trimmed_cap: count(cap)?,
        truncated,
    })
}

/// One `c:` link from `HMGET s t e r`. All of `s`, `t`, `e` absent is no link; some absent is a
/// decode error.
fn decode_link(fields: &[Option<String>]) -> Result<Option<CallLink>, HistoryStoreError> {
    let [s, t, e, r] = fields else {
        return Err(decode_error(format!("a call link read returned {} fields", fields.len())));
    };
    match (s, t, e) {
        (None, None, None) => Ok(None),
        (Some(s), Some(t), Some(e)) => Ok(Some(CallLink {
            session: Uuid::try_parse(s).map_err(|_| decode_error("a call link session is not a uuid"))?,
            tool: t.clone(),
            event_id: Uuid::try_parse(e).map_err(|_| decode_error("a call link event id is not a uuid"))?,
            result_written: r.is_some(),
        })),
        _ => Err(decode_error("a call link has missing fields")),
    }
}

fn decode_session(row: &[String]) -> Result<SessionState, HistoryStoreError> {
    let [id, last_ms, turn] = row else {
        return Err(decode_error(format!("a session read returned {} fields", row.len())));
    };
    Ok(SessionState {
        id: Uuid::try_parse(id).map_err(|_| decode_error("a session id is not a uuid"))?,
        last_ms: last_ms.parse().map_err(|_| decode_error("a session last_ms is not an integer"))?,
        turn: turn.parse().map_err(|_| decode_error("a session turn is not an integer"))?,
    })
}

type LookupRaw = (Vec<Vec<Option<String>>>, Vec<Option<String>>, Vec<Vec<String>>);

#[async_trait::async_trait]
impl SessionStore for RedisSessionStore {
    /// `req.now_ms` is ignored here: the server TTL decides what has expired (ruling T7-e).
    async fn lookup(&self, req: &LookupRequest) -> Result<LookupReply, HistoryStoreError> {
        let nc = req.call_ids.len() + req.result_call_ids.len();
        if nc == 0 && req.refs.is_empty() {
            return Ok(LookupReply {
                calls: Vec::new(),
                results: Vec::new(),
                refs: Vec::new(),
                sessions: Vec::new(),
            });
        }
        let mut inv = self.lookup.prepare_invoke();
        for id in req.call_ids.iter().chain(req.result_call_ids.iter()) {
            inv.key(call_link_key(req.org, &req.principal, id));
        }
        for id in &req.refs {
            inv.key(decision_link_key(req.org, &req.principal, *id));
        }
        inv.arg(nc).arg(req.refs.len()).arg(format!("{KEY_PREFIX}s:{}:", req.org));
        let mut con = self.handle.clone();
        let (links, refs, sessions): LookupRaw = inv.invoke_async(&mut con).await.map_err(|err| unavailable(&err))?;
        let links = links.iter().map(|f| decode_link(f)).collect::<Result<Vec<_>, _>>()?;
        let refs = refs
            .iter()
            .map(|s| s.as_deref().map(|s| Uuid::try_parse(s).map_err(|_| decode_error("a decision link session is not a uuid"))).transpose())
            .collect::<Result<Vec<_>, _>>()?;
        let sessions = sessions.iter().map(|row| decode_session(row)).collect::<Result<Vec<_>, _>>()?;
        // A short reply is left short on purpose: `link_session` refuses a reply of the wrong length.
        let split = req.call_ids.len().min(links.len());
        let mut calls = links;
        let results = calls.split_off(split);
        Ok(LookupReply { calls, results, refs, sessions })
    }

    async fn write(&self, req: &WriteRequest) -> Result<AppendReply, HistoryStoreError> {
        let h = &req.histories;
        let events = req
            .results
            .iter()
            .map(|r| NewEvent {
                id: r.id,
                tool: r.tool.clone(),
                guard: Some(call_link_key(h.org, &h.principal, &r.call_id)),
                link_keys: Vec::new(),
            })
            .collect();
        self.run_append(&Batch {
            histories: h,
            turn: req.turn,
            kind: EventKind::Result,
            verdict: EventVerdict::Allow,
            link: LinkKind::None,
            events,
            now_ms: req.now_ms,
            window_from_ms: None,
        })
        .await
    }

    async fn reserve(&self, req: &ReserveRequest) -> Result<AppendReply, HistoryStoreError> {
        let h = &req.histories;
        let events = req
            .calls
            .iter()
            .map(|c| NewEvent {
                id: c.id,
                tool: c.tool.clone(),
                guard: None,
                link_keys: vec![decision_link_key(h.org, &h.principal, c.id)],
            })
            .collect();
        self.run_append(&Batch {
            histories: h,
            turn: Some(req.turn),
            kind: EventKind::Call,
            verdict: EventVerdict::Pending,
            link: LinkKind::Decision,
            events,
            now_ms: req.now_ms,
            window_from_ms: req.window_from_ms,
        })
        .await
    }

    async fn settle(&self, req: &SettleRequest) -> Result<(), HistoryStoreError> {
        if req.items.is_empty() {
            return Ok(());
        }
        let mut inv = self.settle.prepare_invoke();
        inv.arg(KEY_TTL_SECS).arg(req.session.to_string()).arg(req.items.len());
        for item in &req.items {
            let links: Vec<String> = item.call_id.iter().chain(item.item_id.iter()).map(|id| call_link_key(req.org, &req.principal, id)).collect();
            inv.key(event_key(req.org, item.event_id));
            for key in &links {
                inv.key(key);
            }
            inv.arg(item.event_id.to_string()).arg(item.verdict.as_str()).arg(&item.tool).arg(links.len());
        }
        let mut con = self.handle.clone();
        inv.invoke_async::<i64>(&mut con).await.map(|_| ()).map_err(|err| unavailable(&err))
    }

    async fn append(&self, req: &AppendRequest) -> Result<AppendReply, HistoryStoreError> {
        let h = &req.histories;
        let events = req
            .calls
            .iter()
            .map(|c| NewEvent {
                id: c.id,
                tool: c.tool.clone(),
                guard: None,
                link_keys: c.call_id.iter().chain(c.item_id.iter()).map(|id| call_link_key(h.org, &h.principal, id)).collect(),
            })
            .collect();
        self.run_append(&Batch {
            histories: h,
            turn: Some(req.turn),
            kind: EventKind::Call,
            verdict: EventVerdict::Allow,
            link: LinkKind::Call,
            events,
            now_ms: req.now_ms,
            window_from_ms: None,
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_check_classifies_each_reply() {
        let reply = |policy: &str| Ok(vec!["maxmemory-policy".to_string(), policy.to_string()]);
        assert_eq!(eviction_check(&reply("noeviction")), EvictionCheck::Safe);
        for policy in ["volatile-ttl", "volatile-lru", "allkeys-random", "allkeys-lfu"] {
            assert_eq!(eviction_check(&reply(policy)), EvictionCheck::Evicts(policy.to_string()));
        }
        let noperm = redis::RedisError::from((redis::ErrorKind::Server(redis::ServerErrorKind::NoPerm), "this user has no permissions to run the 'config|get' command"));
        assert_eq!(eviction_check(&Err(noperm)), EvictionCheck::Unknown);
        let unknown = redis::RedisError::from((redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError), "unknown command 'CONFIG'"));
        assert_eq!(eviction_check(&Err(unknown)), EvictionCheck::Unknown);
        assert_eq!(eviction_check(&Ok(vec![])), EvictionCheck::Unknown);
    }

    #[test]
    fn key_names_hold_no_client_string() {
        let org = Uuid::from_u128(1);
        let client = "call:id*with?[glob]";
        let principal = "prn:pgs:iam:::principal/p*1";
        let keys = [
            history_key(org, Scope::Principal, principal, client),
            history_key(org, Scope::Key, client, "send"),
            tools_key(org, Scope::Key, client),
            call_link_key(org, principal, client),
            decision_link_key(org, principal, Uuid::from_u128(2)),
        ];
        for key in &keys {
            let rest = key.strip_prefix(KEY_PREFIX).expect("the prefix");
            for raw in [client, principal, "*", "?", "["] {
                assert!(!rest.contains(raw), "{key} holds {raw:?}");
            }
        }
        assert_eq!(
            call_link_key(org, principal, client),
            format!("{KEY_PREFIX}l:{org}:{}:c:{}", link_digest(principal), link_digest(client))
        );
        assert_eq!(session_key(org, Uuid::from_u128(3)), format!("{KEY_PREFIX}s:{org}:{}", Uuid::from_u128(3)));
        assert_eq!(event_key(org, Uuid::from_u128(3)), format!("{KEY_PREFIX}e:{org}:{}", Uuid::from_u128(3)));
    }

    fn fields(body: &str, verdict: &str) -> Vec<(String, String)> {
        vec![("b".into(), body.into()), ("v".into(), verdict.into()), ("x".into(), "later".into())]
    }

    #[test]
    fn a_body_with_an_unknown_field_still_decodes() {
        let id = Uuid::from_u128(9);
        let body = format!(r#"{{"v":1,"id":"{id}","ts":42,"k":"response","t":"send","fields":{{"a":1}},"bad":true}}"#);
        let event = decode_event(&fields(&body, "deny")).expect("decodes");
        assert_eq!(
            event,
            StoredEvent {
                id,
                ts_ms: 42,
                kind: EventKind::Result,
                tool: "send".into(),
                verdict: EventVerdict::Deny
            }
        );
        // The encoder and the decoder agree.
        let own = encode_body(id, 7, EventKind::Call, "read");
        assert_eq!(decode_event(&fields(&own, "pending")).expect("decodes").ts_ms, 7);
        assert!(decode_event(&fields("not json", "allow")).is_err());
        assert!(decode_event(&fields(&own, "maybe")).is_err());
    }

    #[test]
    fn errors_map_to_their_kinds() {
        let io = redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused"));
        let short_circuit = redis::RedisError::from((redis::ErrorKind::Io, paigasus_redis::BREAKER_OPEN_MESSAGE));
        let server = redis::RedisError::from((
            redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError),
            "OOM command not allowed when used memory > 'maxmemory'",
        ));
        let decode = redis::RedisError::from((redis::ErrorKind::UnexpectedReturnType, "not an array"));
        let parse = redis::RedisError::from((redis::ErrorKind::Parse, "bad reply"));
        let kinds = [io, short_circuit, server, decode, parse].map(|err| unavailable_kind(&err));
        assert_eq!(
            kinds,
            [UnavailableKind::Io, UnavailableKind::Io, UnavailableKind::Server, UnavailableKind::Decode, UnavailableKind::Decode]
        );
        let url = "redis://:s3cret-pw@redis.internal.example:6379/0";
        let err = redis::RedisError::from((redis::ErrorKind::Parse, "bad reply", url.to_string()));
        let HistoryStoreError::Unavailable { detail, .. } = unavailable(&err);
        assert!(!detail.contains("s3cret-pw") && !detail.contains("redis.internal.example"), "{detail}");
    }

    #[test]
    fn the_breaker_series_carry_the_tool_history_role() {
        let m = breaker_metrics();
        assert_eq!(
            (m.state, m.transitions, m.role),
            (names::GATEWAY_REDIS_BREAKER_STATE, names::GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL, "tool_history")
        );
    }
}
