// SPDX-License-Identifier: Apache-2.0

//! `RedisSessionStore` (SMA-753 spec § 7, D4, D5, D8, D11): the session history on a shared Redis,
//! so every replica that points at it sees the same histories and links.
//!
//! Each operation is ONE Lua script call. Redis runs a script atomically, so two concurrent
//! requests on two replicas see each other: a result guard, a trim or a session touch never
//! interleaves with another. Every instant comes from the gateway's clock; no script calls `TIME`.
//! Rust builds every key name and passes it in `KEYS`. The exceptions are the `s:` key of a
//! session and the `e:` key of an event that a link names, which `LOOKUP_SCRIPT` builds from a
//! prefix in `ARGV` and only reads, and the keys of the window read in `APPEND_SCRIPT`: the `e:`
//! key of each member that it reads, and the `h:` and `x:` keys of each tool of an every-tool read
//! (the `t:` index holds tool digests, so only the script knows them). All of these are only read.
//! That is allowed because Redis Cluster is not supported (N4).
//!
//! Layout. A client string in a key name is a digest (`link_digest`), never the raw string:
//! - `h:<org>:<s|p|k>:<sha256(scope id)>:<sha256(tool)>`: a sorted set, score `ts_ms`, member the event id.
//! - `t:<org>:<s|p|k>:<sha256(scope id)>`: the set of tool digests of a history.
//! - `e:<org>:<event id>`: a hash, `b` = the JSON body, `v` = the verdict, and on a call event
//!   `r` = the id of its result event once that exists (the result guard).
//! - `l:<org>:<sha256(principal)>:c:<sha256(call id)>`: a hash, `s`, `t`, `e`. No state field, so a
//!   PR 1 replica can read it after a rollback.
//! - `l:<org>:<sha256(principal)>:d:<decision id>`: a string, the session.
//! - `s:<org>:<session>`: a hash, `last_ms` and `turn`.
//! - `x:<org>:<s|p|k>:<sha256(scope id)>:<sha256(tool)>`: a string, the truncation mark of an `h:`
//!   key: the largest score that a cap trim removed from it (spec P4). It keeps the larger value.
//! - `x:t:<org>:<s|p|k>:<sha256(scope id)>`: a string, the every-tool mark of a history: the time of
//!   a write whose tool did not fit in the `t:` index (`MAX_INDEXED_TOOLS`). It keeps the larger
//!   value.
//!
//! The window read (spec P4) runs at the end of a reserve, after its writes, so the reply holds the
//! pending events of the reserve. It reads at most `MAX_READ_SETS` sets and `MAX_READ_EVENTS`
//! events: the set that passes a limit and every later set come back truncated, with no event. A
//! member whose `e:` body is gone is skipped and counted (spec P10).
//!
//! The body `b` (spec P3, `body.rs`) keeps `v: 1` and has these members: `id`, `ts`, `k`, `t`, and
//! then the stored fields of the call: `f` (path to value) and `kid` (the hex key id), or `b: 1`
//! (unparsed arguments). A result event gets the `f`, `kid` and `b` of its call: the write script
//! copies them by text from the call body that the result guard reads (design D5).
//!
//! The `c:` links (spec P8, P9):
//! - The reserve writes the `c:` link of each call next to its `d:` link, under the call id and
//!   under the item id. It keeps a link that already names another event (a reused call id).
//! - An allow settle writes the link, and replaces a link of another event (PR 1).
//! - A deny settle deletes a link only when its `e` is this event and the stored verdict is `deny`.
//! - An append writes the verdict of the call. Only an allowed call writes (and replaces) links. A
//!   denied call deletes each link that still names its event (a late reserve wrote it). An
//!   append under an id that a reserve wrote replaces the body, the verdict and the score of that
//!   one member: one event (plan decision D-E).
//!
//! Every key gets `EXPIRE KEY_TTL_SECS` on each write. A trim never deletes an `e:` body: another
//! history can still hold the event, and the body expires by its TTL.
//!
//! The connection is `paigasus_redis::connect` with the SMA-476 breaker; its series carry the
//! gateway's names and `role="tool_history"`.

use std::collections::HashSet;
use std::sync::Arc;

use paigasus_kernel::tool_policy::history::{Fields, KeyId, SetTool};
use paigasus_observability::names;
use paigasus_redis::{BreakerMetrics, RedisHandle};
use uuid::Uuid;

use super::body::{decode_body, decode_error, encode_body};
use crate::domain::limits::UnavailableKind;
use crate::domain::tool_history::{
    AppendReply, AppendRequest, CallLink, EventKind, EventVerdict, Histories, HistoryStoreError, KEY_TTL_SECS, LookupReply, LookupRequest, MAX_EVENTS_PER_TOOL, MAX_INDEXED_TOOLS, MAX_READ_EVENTS,
    MAX_READ_SETS, RETENTION_MS, ReadRequest, ReserveRequest, Scope, SessionState, SessionStore, SetWindow, SettleRequest, StoredEvent, WriteRequest, check_window, link_digest,
};

/// Every key starts with this. `v1` lets a later format use new keys with no migration.
pub const KEY_PREFIX: &str = "paigasus:gateway:history:v1:";

/// The `role` label of the breaker series of this connection.
pub const HISTORY_BREAKER_ROLE: &str = "tool_history";

/// The `c:` link rules, shared by `APPEND_SCRIPT` and `SETTLE_SCRIPT`.
macro_rules! call_link_lua {
    () => {
        r#"
-- Write the c: link of a call. The result guard `r` is on the e: key of the event, not here: a
-- relink to a new event reads the fresh guard of that event (ruling T7-b), and a settle retry for
-- the same event keeps the written guard (ruling T7-f).
local function put_call_link(key, session, tool, id, ttl)
  redis.call('HSET', key, 's', session, 't', tool, 'e', id)
  redis.call('EXPIRE', key, ttl)
end
-- Delete the c: link only when it still names the event `id` (spec P8).
local function drop_own_call_link(key, id)
  if redis.call('HGET', key, 'e') == id then
    redis.call('DEL', key)
  end
end
"#
    };
}

/// Shared by `write`, `reserve` and `append`. Redis 6.2 commands only.
const APPEND_SCRIPT: &str = concat!(
    call_link_lua!(),
    r#"
-- ARGV: now_ms, retention_ms, cap, ttl_secs, window_from ('' = none), turn ('' = no touch),
--       session, c-link mode ('keep'|'replace'), guard ('0'|'1'), H, T, N,
--       max indexed tools, max read sets, max read events, the e: key prefix,
--       then per tool (2 values): tool digest, tool name,
--       then per event (6 values): id, body, tool index (1..T), verdict, d-link count, c-link count,
--       then R (the number of read sets; only read when window_from is set),
--       then per read set (3 values): 't' (one tool), 'e' (every tool) or another value (a set
--       that the adapter does not know: truncated), the h: key prefix and the x: key prefix of
--       the history ('' unless 'e').
-- KEYS: s-key, H t-keys, H x:t keys (the every-tool marks), T * H h-keys (tool t, history i at
--       1 + 2H + (t - 1) * H + i), T * H x-keys (at 1 + 2H + T * H + (t - 1) * H + i),
--       then per event: e-key, the guard key when guard = '1' (the e: key of the CALL event
--       that the result belongs to), then the d: keys, then the c: keys (a local shell call has
--       two c: links, F7),
--       then per read set (2 keys): the h: key and its x: key ('t'), or the t: key and the x:t key
--       of the history ('e').
-- c-link mode 'keep' (reserve): a c: link that names another event stays. 'replace' (append): the
-- link is written. An event with the verdict 'deny' (an append) writes no c: link: it deletes each
-- one that still names it.
-- A history key and a tool go once per distinct tool, not once per event: a reserve of 10,000
-- events must stay small on the wire.
local now = tonumber(ARGV[1])
local cutoff = now - tonumber(ARGV[2])
local cap = tonumber(ARGV[3])
local ttl = tonumber(ARGV[4])
local window_from = ARGV[5]
local turn = ARGV[6]
local session = ARGV[7]
local cmode = ARGV[8]
local guard = ARGV[9]
local H = tonumber(ARGV[10])
local T = tonumber(ARGV[11])
local N = tonumber(ARGV[12])
local max_tools = tonumber(ARGV[13])
local max_sets = tonumber(ARGV[14])
local max_events = tonumber(ARGV[15])
local eprefix = ARGV[16]
local A0 = 16

local digests, names, members = {}, {}, {}
for t = 1, T do
  digests[t] = ARGV[A0 + 2 * t - 1]
  names[t] = ARGV[A0 + 2 * t]
  members[t] = {}
end
local function hkey(t, i)
  return KEYS[1 + 2 * H + (t - 1) * H + i]
end
local function xkey(t, i)
  return KEYS[1 + 2 * H + T * H + (t - 1) * H + i]
end

-- A truncation mark keeps the larger value, and gets the TTL of its set.
local function raise_mark(key, value)
  local old = redis.call('GET', key)
  if not old or tonumber(old) < value then
    redis.call('SET', key, string.format('%.0f', value))
  end
  redis.call('EXPIRE', key, ttl)
end

-- The stored fields of a call body are its last members (`,"f":` or `,"b":` up to the closing
-- brace, see body.rs). Copy them into the result body by text: Lua reads a JSON number as a
-- double, so a decode and an encode would change a large Long. A call body with neither member
-- (PR 1) gives none.
local function copy_fields(body, call_body)
  if not call_body then
    return body
  end
  local at = string.find(call_body, ',"f":', 1, true) or string.find(call_body, ',"b":', 1, true)
  if not at then
    return body
  end
  return string.sub(body, 1, -2) .. string.sub(call_body, at, -2) .. '}'
end

local written = 0
local seen_guard, guard_sets = {}, {}
local k, a = 1 + 2 * H + 2 * T * H, A0 + 2 * T
for j = 1, N do
  local id, body, t, verdict = ARGV[a + 1], ARGV[a + 2], tonumber(ARGV[a + 3]), ARGV[a + 4]
  local nd, nc = tonumber(ARGV[a + 5]), tonumber(ARGV[a + 6])
  a = a + 6
  k = k + 1
  local ekey = KEYS[k]
  local skip = false
  local gkey = nil
  if guard == '1' then
    k = k + 1
    gkey = KEYS[k]
    -- F5: a call event that is gone gets no result and no new key.
    if redis.call('EXISTS', gkey) == 0 then
      skip = true
    -- The result is already written: write it once per call event. Both aliases of a local
    -- shell call name the same event, and a relink after the lookup does not move the guard.
    -- The guard is only READ here and is set LAST (below), so a script that stops half way
    -- (an ACL error on a later write) leaves no guard and a retry writes the result.
    elseif seen_guard[gkey] or redis.call('HEXISTS', gkey, 'r') == 1 then
      skip = true
    end
  end
  local dbase = k
  local cbase = k + nd
  k = k + nd + nc
  if not skip then
    if gkey then
      body = copy_fields(body, redis.call('HGET', gkey, 'b'))
    end
    redis.call('HSET', ekey, 'b', body, 'v', verdict)
    redis.call('EXPIRE', ekey, ttl)
    table.insert(members[t], id)
    for l = 1, nd do
      redis.call('SET', KEYS[dbase + l], session, 'EX', ttl)
    end
    for l = 1, nc do
      local lkey = KEYS[cbase + l]
      if verdict == 'deny' then
        -- A denied call was not forwarded: it deletes the links that still name it (a reserve
        -- that returned too late for the report site wrote them), and writes none.
        drop_own_call_link(lkey, id)
      else
        local held = cmode == 'keep' and redis.call('HGET', lkey, 'e') or false
        if not held or held == id then
          put_call_link(lkey, session, names[t], id, ttl)
        end
      end
    end
    if gkey then
      seen_guard[gkey] = true
      guard_sets[#guard_sets + 1] = {gkey, id}
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
      -- The t: index holds at most max_tools tools. A tool past it keeps its own set, and the
      -- history gets the every-tool mark (spec P4).
      local tkey = KEYS[1 + i]
      if redis.call('SISMEMBER', tkey, digests[t]) == 0 then
        if redis.call('SCARD', tkey) < max_tools then
          redis.call('SADD', tkey, digests[t])
        else
          raise_mark(KEYS[1 + H + i], now)
        end
      end
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
        -- ZPOPMIN answers in ascending score order: the last score is the largest removed one.
        raise_mark(xkey(t, i), tonumber(popped[#popped]))
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

-- The result guards go LAST, after the bodies, the links and the history writes.
for _, g in ipairs(guard_sets) do
  redis.call('HSET', g[1], 'r', g[2])
  redis.call('EXPIRE', g[1], ttl)
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

-- The window read (spec P4), after every write. A set past a read limit and every later set come
-- back truncated with no event and no mark. An entry is {set index (1..R), truncated, mark ('' =
-- none), then 2 values per event: body, verdict}. The member and the score are not sent: every
-- write puts the member into the body `id` and the score into the body `ts`, so the adapter takes
-- both from the body (Task 19b: 20 % less server time, 32 % less reply). The adapter checks the
-- index against the request order. A member whose e: body is gone is skipped and counted.
local window, missing = {}, 0
if window_from ~= '' then
  local R = tonumber(ARGV[a + 1])
  local read_events, stopped = 0, false
  for r = 1, R do
    local kind, hprefix, xprefix = ARGV[a + 3 * r - 1], ARGV[a + 3 * r], ARGV[a + 3 * r + 1]
    local k1, k2 = KEYS[k + 2 * r - 1], KEYS[k + 2 * r]
    if r > max_sets or (kind ~= 't' and kind ~= 'e') then
      stopped = true
    end
    local entry = {tostring(r), '1', ''}
    if not stopped then
      local hkeys, mark = {}, redis.call('GET', k2)
      mark = mark and tonumber(mark) or nil
      if kind == 't' then
        hkeys[1] = k1
      else
        for _, d in ipairs(redis.call('SMEMBERS', k1)) do
          hkeys[#hkeys + 1] = hprefix .. d
          local m = redis.call('GET', xprefix .. d)
          if m and (not mark or tonumber(m) > mark) then
            mark = tonumber(m)
          end
        end
      end
      local count = 0
      for _, h in ipairs(hkeys) do
        count = count + redis.call('ZCOUNT', h, window_from, '+inf')
      end
      if read_events + count > max_events then
        stopped = true
      else
        read_events = read_events + count
        entry = {tostring(r), '0', mark and string.format('%.0f', mark) or ''}
        for _, h in ipairs(hkeys) do
          local m = redis.call('ZRANGEBYSCORE', h, window_from, '+inf')
          for p = 1, #m do
            local f = redis.call('HMGET', eprefix .. m[p], 'b', 'v')
            if f[1] and f[2] then
              entry[#entry + 1] = f[1]
              entry[#entry + 1] = f[2]
            else
              missing = missing + 1
            end
          end
        end
      end
    end
    window[r] = entry
  end
end

local out = {tostring(written), tostring(trimmed_retention), tostring(trimmed_cap), tostring(missing)}
for _, v in ipairs(flagged) do
  table.insert(out, v)
end
return {out, window}
"#
);

/// Read only.
const LOOKUP_SCRIPT: &str = r#"
-- ARGV: the number of c: keys (calls, then results), the number of d: keys, the s: key prefix,
--       the e: key prefix.
-- KEYS: the c: keys, then the d: keys.
-- A link reply is {s, t, e, r}: `r` is '1' when the linked EVENT has its result guard.
local nc, nd, prefix, eprefix = tonumber(ARGV[1]), tonumber(ARGV[2]), ARGV[3], ARGV[4]
local links, refs, sessions, named, order = {}, {}, {}, {}, {}
local function name(s)
  if s and not named[s] then
    named[s] = true
    table.insert(order, s)
  end
end
for i = 1, nc do
  local f = redis.call('HMGET', KEYS[i], 's', 't', 'e')
  local r = false
  if f[3] and redis.call('HEXISTS', eprefix .. f[3], 'r') == 1 then
    r = '1'
  end
  links[i] = {f[1], f[2], f[3], r}
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
    local stored = redis.call('HGET', ekey, 'v')
    if stored == 'pending' then
      redis.call('HSET', ekey, 'v', verdict)
      stored = verdict
    end
    redis.call('EXPIRE', ekey, ttl)
    -- The link follows the STORED verdict: a later allow settle of a denied event writes no link,
    -- and a later deny settle of an allowed event deletes none.
    if verdict == 'allow' and stored == 'allow' then
      for l = 1, nlinks do
        put_call_link(KEYS[lbase + l], session, tool, id, ttl)
      end
    elseif verdict == 'deny' and stored == 'deny' then
      for l = 1, nlinks do
        drop_own_call_link(KEYS[lbase + l], id)
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

/// The truncation mark of `history_key(org, scope, scope_id, tool)`.
pub fn mark_key(org: Uuid, scope: Scope, scope_id: &str, tool: &str) -> String {
    format!("{KEY_PREFIX}x:{org}:{}:{}:{}", scope.as_str(), link_digest(scope_id), link_digest(tool))
}

/// The every-tool mark of a history.
fn tools_mark_key(org: Uuid, scope: Scope, scope_id: &str) -> String {
    format!("{KEY_PREFIX}x:t:{org}:{}:{}", scope.as_str(), link_digest(scope_id))
}

/// The key prefixes of the sets of one history: `h:` and `x:`. The script adds a tool digest.
fn set_prefixes(org: Uuid, scope: Scope, scope_id: &str) -> (String, String) {
    let tail = format!("{org}:{}:{}:", scope.as_str(), link_digest(scope_id));
    (format!("{KEY_PREFIX}h:{tail}"), format!("{KEY_PREFIX}x:{tail}"))
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

/// Decode the fields of an `e:` hash (`b` = the JSON body, `v` = the verdict). Unknown fields of
/// the hash and unknown keys of the body are ignored (rolling upgrade).
///
/// # Errors
///
/// A hash with no `b` or no `v`, a body that `decode_body` refuses, or an unknown verdict.
pub fn decode_event(fields: &[(String, String)]) -> Result<StoredEvent, HistoryStoreError> {
    let field = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
    let body = decode_body(field("b").ok_or_else(|| decode_error("event body has no `b` field"))?)?;
    let verdict = parse_verdict(field("v").ok_or_else(|| decode_error("event has no `v` field"))?)?;
    Ok(StoredEvent {
        id: body.id,
        ts_ms: body.ts_ms,
        kind: body.kind,
        tool: body.tool,
        verdict,
        fields: body.fields,
        key: body.key,
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
///
/// RESP2 answers with a flat array `[name, policy]`; RESP3 answers with a map `{name: policy}`.
/// Both shapes give the real policy, so an evicting policy fails the boot on either protocol.
pub fn eviction_check(reply: &redis::RedisResult<redis::Value>) -> EvictionCheck {
    let Ok(value) = reply else {
        return EvictionCheck::Unknown;
    };
    match policy_of(value) {
        Some(policy) if policy == "noeviction" => EvictionCheck::Safe,
        Some(policy) => EvictionCheck::Evicts(policy),
        None => EvictionCheck::Unknown,
    }
}

/// The `maxmemory-policy` value of a `CONFIG GET` reply, in the RESP2 array or the RESP3 map shape.
fn policy_of(value: &redis::Value) -> Option<String> {
    const NAME: &str = "maxmemory-policy";
    match value {
        redis::Value::Array(items) => match items.as_slice() {
            [name, policy] if text_of(name).as_deref() == Some(NAME) => text_of(policy),
            _ => None,
        },
        redis::Value::Map(pairs) => match pairs.as_slice() {
            [(name, policy)] if text_of(name).as_deref() == Some(NAME) => text_of(policy),
            _ => None,
        },
        _ => None,
    }
}

fn text_of(value: &redis::Value) -> Option<String> {
    match value {
        redis::Value::BulkString(bytes) => String::from_utf8(bytes.clone()).ok(),
        redis::Value::SimpleString(text) | redis::Value::VerbatimString { text, .. } => Some(text.clone()),
        _ => None,
    }
}

/// How `APPEND_SCRIPT` writes a `c:` link that already exists.
#[derive(Clone, Copy)]
enum CallLinkMode {
    /// The reserve: a link that names another event stays (spec P8).
    Keep,
    /// The append: the link is written.
    Replace,
}

impl CallLinkMode {
    fn as_arg(self) -> &'static str {
        match self {
            CallLinkMode::Keep => "keep",
            CallLinkMode::Replace => "replace",
        }
    }
}

/// One event for `APPEND_SCRIPT`.
struct NewEvent {
    id: Uuid,
    tool: String,
    verdict: EventVerdict,
    /// The stored fields and their key id. `None` for a result event: the script copies them from
    /// the call body.
    stored: Option<(Fields, Option<KeyId>)>,
    /// The `e:` key of the call event whose `r` guard decides if the result is written. `write` only.
    guard: Option<String>,
    decision_keys: Vec<String>,
    call_keys: Vec<String>,
}

/// One `APPEND_SCRIPT` call. Every event of a call has the same kind and the same link mode.
struct Batch<'a> {
    histories: &'a Histories,
    turn: Option<i64>,
    kind: EventKind,
    call_links: CallLinkMode,
    events: Vec<NewEvent>,
    now_ms: u64,
    read: Option<&'a ReadRequest>,
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
        let reply = redis::cmd("CONFIG").arg("GET").arg("maxmemory-policy").query_async::<redis::Value>(&mut con).await;
        eviction_check(&reply)
    }

    async fn run_append(&self, batch: &Batch<'_>) -> Result<AppendReply, HistoryStoreError> {
        let h = batch.histories;
        if batch.events.is_empty() && batch.turn.is_none() && batch.read.is_none() {
            return Ok(AppendReply::empty());
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
        for (scope, id) in &scopes {
            inv.key(tools_mark_key(h.org, *scope, id));
        }
        for tool in &tools {
            for (scope, id) in &scopes {
                inv.key(history_key(h.org, *scope, id, tool));
            }
        }
        for tool in &tools {
            for (scope, id) in &scopes {
                inv.key(mark_key(h.org, *scope, id, tool));
            }
        }
        inv.arg(batch.now_ms)
            .arg(RETENTION_MS)
            .arg(MAX_EVENTS_PER_TOOL)
            .arg(KEY_TTL_SECS)
            .arg(batch.read.map(|r| r.from_ms.to_string()).unwrap_or_default())
            .arg(batch.turn.map(|t| t.to_string()).unwrap_or_default())
            .arg(h.session.to_string())
            .arg(batch.call_links.as_arg())
            .arg(if guarded { "1" } else { "0" })
            .arg(scopes.len())
            .arg(tools.len())
            .arg(batch.events.len())
            .arg(MAX_INDEXED_TOOLS)
            .arg(MAX_READ_SETS)
            .arg(MAX_READ_EVENTS)
            .arg(format!("{KEY_PREFIX}e:{}:", h.org));
        for tool in &tools {
            inv.arg(link_digest(tool)).arg(*tool);
        }
        for e in &batch.events {
            inv.key(event_key(h.org, e.id));
            // `guarded` holds for every event of a `write` and for none of a reserve or append.
            if let Some(guard) = &e.guard {
                inv.key(guard);
            }
            for key in e.decision_keys.iter().chain(&e.call_keys) {
                inv.key(key);
            }
            let index = tools.iter().position(|t| *t == e.tool).map_or(0, |i| i + 1);
            let body = encode_body(e.id, batch.now_ms, batch.kind, &e.tool, e.stored.as_ref().map(|(f, k)| (f, *k)));
            inv.arg(e.id.to_string()).arg(body).arg(index).arg(e.verdict.as_str()).arg(e.decision_keys.len()).arg(e.call_keys.len());
        }
        if let Some(read) = batch.read {
            inv.arg(read.sets.len());
            for (scope, set) in &read.sets {
                let id = h.scope_id(*scope);
                match set {
                    SetTool::Tool(tool) => {
                        inv.key(history_key(h.org, *scope, &id, tool)).key(mark_key(h.org, *scope, &id, tool));
                        inv.arg("t").arg("").arg("");
                    }
                    SetTool::Every => {
                        let (hprefix, xprefix) = set_prefixes(h.org, *scope, &id);
                        inv.key(tools_key(h.org, *scope, &id)).key(tools_mark_key(h.org, *scope, &id));
                        inv.arg("e").arg(hprefix).arg(xprefix);
                    }
                    // A later kind of set that this adapter does not know: the script returns it
                    // truncated, so it fails closed.
                    _ => {
                        inv.key(tools_key(h.org, *scope, &id)).key(tools_mark_key(h.org, *scope, &id));
                        inv.arg("?").arg("").arg("");
                    }
                }
            }
        }
        let mut con = self.handle.clone();
        let (counts, window): (Vec<String>, Vec<Vec<String>>) = inv.invoke_async(&mut con).await.map_err(|err| unavailable(&err))?;
        let mut reply = append_reply(&counts, &scopes, &tools)?;
        if let Some(read) = batch.read {
            reply.window = window_reply(read, &window)?;
        }
        Ok(reply)
    }
}

/// Map the window part of the script reply: per set, the 1-based index of the requested set that
/// the script read, the truncated flag, the mark, then 2 values per event (body, verdict). The
/// reply holds no member and no score: every write puts the member into the body `id` and the score
/// into the body `ts` (the same `now_ms`), so the event takes both from its body. The events are
/// sorted by (`ts_ms`, id). The reply must hold one entry per requested set, in order: the echoed
/// index of each entry must be its position.
///
/// The script read only members with a score of at least `from_ms`, and the members of a set are
/// distinct. So a body whose `ts` is before `from_ms`, or a body id twice in one set, disagrees
/// with its member or score: a decode error, as a member that is not the id of its body was.
fn window_reply(req: &ReadRequest, raw: &[Vec<String>]) -> Result<Vec<SetWindow>, HistoryStoreError> {
    let mut window = Vec::with_capacity(raw.len());
    for (position, (entry, (scope, tool))) in raw.iter().zip(&req.sets).enumerate() {
        let [index, truncated, mark, events @ ..] = entry.as_slice() else {
            return Err(decode_error("a window entry has no set index, truncated flag or mark"));
        };
        if index.parse::<usize>().ok() != Some(position + 1) {
            return Err(decode_error("a window entry is not the set of its position in the request"));
        }
        if events.len() % 2 != 0 {
            return Err(decode_error("a window entry has an incomplete event"));
        }
        let mut stored = Vec::with_capacity(events.len() / 2);
        for e in events.chunks(2) {
            let event = decode_event(&[("b".into(), e[0].clone()), ("v".into(), e[1].clone())])?;
            if event.ts_ms < req.from_ms {
                return Err(decode_error("a window body has a time before the window"));
            }
            stored.push(event);
        }
        let mut ids = HashSet::with_capacity(stored.len());
        if !stored.iter().all(|e| ids.insert(e.id)) {
            return Err(decode_error("a window body id is in one set twice"));
        }
        stored.sort_by_key(|e| (e.ts_ms, e.id));
        window.push(SetWindow {
            scope: *scope,
            tool: tool.clone(),
            events: stored,
            mark_ms: match mark.as_str() {
                "" => None,
                m => Some(m.parse().map_err(|_| decode_error("a window mark is not an integer"))?),
            },
            truncated: match truncated.as_str() {
                "0" => false,
                "1" => true,
                _ => return Err(decode_error("a window truncated flag is not 0 or 1")),
            },
        });
    }
    // A short reply leaves a requested set out: a decode error, never an empty set.
    if raw.len() != req.sets.len() {
        return Err(decode_error(format!("the window reply holds {} sets for {} requested", raw.len(), req.sets.len())));
    }
    check_window(req, &window)?;
    Ok(window)
}

/// Map the script reply: four counts (the last one is the missing bodies), then (history index,
/// tool index) pairs. The key names use the tool digest; the tool index maps back to the tool NAME
/// of the request (F6).
fn append_reply(reply: &[String], scopes: &[(Scope, String)], tools: &[&str]) -> Result<AppendReply, HistoryStoreError> {
    let [written, retention, cap, missing, flagged @ ..] = reply else {
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
        window: Vec::new(),
        missing_bodies: count(missing)?,
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
        inv.arg(nc).arg(req.refs.len()).arg(format!("{KEY_PREFIX}s:{}:", req.org)).arg(format!("{KEY_PREFIX}e:{}:", req.org));
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
                verdict: EventVerdict::Allow,
                stored: None,
                guard: Some(event_key(h.org, r.event_id)),
                decision_keys: Vec::new(),
                call_keys: Vec::new(),
            })
            .collect();
        self.run_append(&Batch {
            histories: h,
            turn: req.turn,
            kind: EventKind::Result,
            call_links: CallLinkMode::Replace,
            events,
            now_ms: req.now_ms,
            read: None,
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
                verdict: EventVerdict::Pending,
                stored: Some((c.fields.clone(), c.key)),
                guard: None,
                decision_keys: vec![decision_link_key(h.org, &h.principal, c.id)],
                call_keys: c.call_id.iter().chain(c.item_id.iter()).map(|id| call_link_key(h.org, &h.principal, id)).collect(),
            })
            .collect();
        self.run_append(&Batch {
            histories: h,
            turn: Some(req.turn),
            kind: EventKind::Call,
            call_links: CallLinkMode::Keep,
            events,
            now_ms: req.now_ms,
            read: req.read.as_ref(),
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
                verdict: c.verdict,
                stored: Some((c.fields.clone(), c.key)),
                guard: None,
                decision_keys: Vec::new(),
                // The script links an allowed call, and deletes the own links of a denied call (spec
                // P8, P9).
                call_keys: c.call_id.iter().chain(c.item_id.iter()).map(|id| call_link_key(h.org, &h.principal, id)).collect(),
            })
            .collect();
        self.run_append(&Batch {
            histories: h,
            turn: Some(req.turn),
            kind: EventKind::Call,
            call_links: CallLinkMode::Replace,
            events,
            now_ms: req.now_ms,
            read: None,
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bulk(s: &str) -> redis::Value {
        redis::Value::BulkString(s.as_bytes().to_vec())
    }

    /// RESP2: `CONFIG GET` answers with a flat array.
    fn resp2(policy: &str) -> redis::RedisResult<redis::Value> {
        Ok(redis::Value::Array(vec![bulk("maxmemory-policy"), bulk(policy)]))
    }

    /// RESP3: `CONFIG GET` answers with a map.
    fn resp3(policy: &str) -> redis::RedisResult<redis::Value> {
        Ok(redis::Value::Map(vec![(bulk("maxmemory-policy"), bulk(policy))]))
    }

    /// A script reply that leaves out a requested set is a decode error, never an empty set
    /// (spec P4).
    #[test]
    fn a_window_reply_with_a_missing_entry_is_a_decode_error() {
        let req = ReadRequest {
            from_ms: 0,
            sets: vec![(Scope::Session, SetTool::Tool("a".into())), (Scope::Session, SetTool::Every)],
        };
        let entry = |i: usize| vec![i.to_string(), "0".to_string(), String::new()];
        assert_eq!(window_reply(&req, &[entry(1), entry(2)]).expect("a full reply").len(), 2);
        let short = window_reply(&req, &[entry(1)]).unwrap_err();
        assert!(matches!(short, HistoryStoreError::Unavailable { kind: UnavailableKind::Decode, .. }), "{short:?}");
        assert!(window_reply(&req, &[entry(1), entry(2), entry(3)]).is_err(), "a long reply");
        assert!(window_reply(&req, &[entry(2), entry(1)]).is_err(), "entries out of the request order");
        assert!(window_reply(&req, &[entry(1), entry(1)]).is_err(), "one set twice");
        assert!(window_reply(&req, &[entry(1), vec!["2".into(), "0".into()]]).is_err(), "an entry with no mark");
        assert!(window_reply(&req, &[entry(1), vec!["2".into(), "0".into(), String::new(), "x".into()]]).is_err(), "an incomplete event");
        assert!(window_reply(&req, &[entry(1), vec!["2".into(), "2".into(), String::new()]]).is_err(), "a bad flag");
    }

    /// A one-set read from `from_ms` of the tool `send`.
    fn send_read(from_ms: u64) -> ReadRequest {
        ReadRequest {
            from_ms,
            sets: vec![(Scope::Session, SetTool::Tool("send".into()))],
        }
    }

    /// One window entry of set 1: two values per event, the body and the verdict.
    fn send_entry(events: &[(String, &str)]) -> Vec<String> {
        let mut entry = vec!["1".to_string(), "0".to_string(), String::new()];
        for (body, verdict) in events {
            entry.push(body.clone());
            entry.push((*verdict).to_string());
        }
        entry
    }

    fn call_body(id: u128, ts_ms: u64) -> String {
        encode_body(Uuid::from_u128(id), ts_ms, EventKind::Call, "send", Some((&Fields::Unparsed, None)))
    }

    /// Task 19b: the reply holds no member and no score. Every write puts the same `now_ms` into
    /// the score and into the body `ts`, and the member is the body `id`, so the event takes both
    /// from its body. The events are sorted by (`ts_ms`, id).
    #[test]
    fn a_window_event_takes_its_id_and_time_from_its_body() {
        let entry = send_entry(&[(call_body(2, 20), "allow"), (call_body(1, 20), "pending"), (call_body(3, 10), "deny")]);
        let window = window_reply(&send_read(10), &[entry]).expect("decodes");
        let got: Vec<(Uuid, u64, EventVerdict)> = window[0].events.iter().map(|e| (e.id, e.ts_ms, e.verdict)).collect();
        assert_eq!(
            got,
            vec![
                (Uuid::from_u128(3), 10, EventVerdict::Deny),
                (Uuid::from_u128(1), 20, EventVerdict::Pending),
                (Uuid::from_u128(2), 20, EventVerdict::Allow),
            ]
        );
    }

    /// The script reads members with a score of at least `from_ms`. A body whose `ts` is before it
    /// disagrees with its score: the reply is corrupt, and the read fails closed (a decode error).
    #[test]
    fn a_window_body_from_before_the_window_is_a_decode_error() {
        let entry = send_entry(&[(call_body(1, 10), "allow"), (call_body(2, 9), "allow")]);
        let err = window_reply(&send_read(10), &[entry]).unwrap_err();
        assert!(matches!(err, HistoryStoreError::Unavailable { kind: UnavailableKind::Decode, .. }), "{err:?}");
    }

    /// The members of one set are distinct, and each body names its own member. Two bodies with
    /// one id in a set disagree with their members: a decode error.
    #[test]
    fn a_window_body_id_twice_in_one_set_is_a_decode_error() {
        let entry = send_entry(&[(call_body(1, 10), "allow"), (call_body(1, 11), "allow")]);
        let err = window_reply(&send_read(10), &[entry]).unwrap_err();
        assert!(matches!(err, HistoryStoreError::Unavailable { kind: UnavailableKind::Decode, .. }), "{err:?}");
    }

    #[test]
    fn a_mark_key_is_the_history_key_under_x() {
        let org = Uuid::from_u128(1);
        let h = history_key(org, Scope::Key, "k1", "send");
        let x = mark_key(org, Scope::Key, "k1", "send");
        assert_eq!(x.strip_prefix(&format!("{KEY_PREFIX}x:")), h.strip_prefix(&format!("{KEY_PREFIX}h:")));
        let (hp, xp) = set_prefixes(org, Scope::Key, "k1");
        assert_eq!((format!("{hp}{}", link_digest("send")), format!("{xp}{}", link_digest("send"))), (h, x));
        assert_eq!(tools_mark_key(org, Scope::Key, "k1"), format!("{KEY_PREFIX}x:t:{org}:k:{}", link_digest("k1")));
    }

    #[test]
    fn eviction_check_classifies_each_reply() {
        for reply in [resp2, resp3] {
            assert_eq!(eviction_check(&reply("noeviction")), EvictionCheck::Safe);
            for policy in ["volatile-ttl", "volatile-lru", "allkeys-random", "allkeys-lfu"] {
                assert_eq!(eviction_check(&reply(policy)), EvictionCheck::Evicts(policy.to_string()));
            }
        }
        let simple = Ok(redis::Value::Map(vec![(
            redis::Value::SimpleString("maxmemory-policy".into()),
            redis::Value::SimpleString("allkeys-lru".into()),
        )]));
        assert_eq!(eviction_check(&simple), EvictionCheck::Evicts("allkeys-lru".to_string()));
        let other_name = Ok(redis::Value::Map(vec![(bulk("maxmemory"), bulk("0"))]));
        assert_eq!(eviction_check(&other_name), EvictionCheck::Unknown);
        assert_eq!(eviction_check(&Ok(redis::Value::Map(vec![]))), EvictionCheck::Unknown);
        assert_eq!(eviction_check(&Ok(redis::Value::Nil)), EvictionCheck::Unknown);
        let noperm = redis::RedisError::from((redis::ErrorKind::Server(redis::ServerErrorKind::NoPerm), "this user has no permissions to run the 'config|get' command"));
        assert_eq!(eviction_check(&Err(noperm)), EvictionCheck::Unknown);
        let unknown = redis::RedisError::from((redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError), "unknown command 'CONFIG'"));
        assert_eq!(eviction_check(&Err(unknown)), EvictionCheck::Unknown);
        assert_eq!(eviction_check(&Ok(redis::Value::Array(vec![]))), EvictionCheck::Unknown);
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
                verdict: EventVerdict::Deny,
                fields: Fields::Values(std::collections::BTreeMap::new()),
                key: None,
            }
        );
        // The encoder and the decoder agree.
        let own = encode_body(id, 7, EventKind::Call, "read", Some((&Fields::Unparsed, None)));
        let decoded = decode_event(&fields(&own, "pending")).expect("decodes");
        assert_eq!((decoded.ts_ms, decoded.fields), (7, Fields::Unparsed));
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
