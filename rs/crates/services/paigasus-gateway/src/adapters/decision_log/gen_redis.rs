// SPDX-License-Identifier: Apache-2.0

//! The Redis scripts of the generation record (SMA-821 spec § 5.1, § 5.3, § 5.3b). Three scripts, each atomic:
//! the group script makes the consumer group and a new generation together, the sample script reads the gauge,
//! the generation and the epoch (read only, so it works at the memory limit), and the detection script repairs a
//! missing generation and counts one epoch step.
//!
//! The keys are `<key>` (the stream), `<key>:gen` (the generation id) and `<key>:epoch` (a counter). The scripts
//! that need "the stream has this group" share one Lua fragment, `HAS_GROUP_LUA`.
//!
//! Logs name error kinds only. They never hold a stream entry or a payload.

use std::sync::LazyLock;

use super::generation::RedisView;

/// The Redis key of the generation id of the stream `stream_key`.
#[must_use]
pub fn gen_key(stream_key: &str) -> String {
    format!("{stream_key}:gen")
}

/// The Redis key of the epoch counter of the stream `stream_key`.
#[must_use]
pub fn epoch_key(stream_key: &str) -> String {
    format!("{stream_key}:epoch")
}

/// The Lua function `has_group(key, name)`: true when the stream `key` has the consumer group `name`. It is written
/// once and put in front of each script that needs it. `XINFO GROUPS` fails on a missing stream, so the function
/// checks `EXISTS` first.
const HAS_GROUP_LUA: &str = "local function has_group(key, name) \
     if redis.call('EXISTS', key) == 0 then return false end \
     for _, g in ipairs(redis.call('XINFO', 'GROUPS', key)) do \
       for i = 1, #g, 2 do \
         if g[i] == 'name' and g[i + 1] == name then return true end \
       end \
     end \
     return false \
   end ";

/// Make the consumer group, and a new generation with it.
///
/// KEYS: stream, gen. ARGV: group, fresh id. Returns the string `created` (this call made the group and set the
/// generation to the fresh id) or `exists` (`BUSYGROUP`; the generation is kept, or set to the fresh id when it is
/// missing). Another error reply is returned as it is.
const GROUP_SCRIPT: &str = "local r = redis.pcall('XGROUP', 'CREATE', KEYS[1], ARGV[1], '0', 'MKSTREAM') \
     if type(r) == 'table' and r.err then \
       if string.sub(r.err, 1, 9) == 'BUSYGROUP' then \
         redis.call('SET', KEYS[2], ARGV[2], 'NX') \
         return 'exists' \
       end \
       return r \
     end \
     redis.call('SET', KEYS[2], ARGV[2]) \
     return 'created'";

/// Read the stream length, the group flag, the generation and the epoch. It writes nothing.
///
/// KEYS: stream, gen, epoch. ARGV: group. Returns `{xlen, has_group (0 or 1), generation or nil, epoch or nil}`.
const SAMPLE_BODY: &str = "local n = redis.call('XLEN', KEYS[1]) \
     local has = 0 \
     if has_group(KEYS[1], ARGV[1]) then has = 1 end \
     return {n, has, redis.call('GET', KEYS[2]), redis.call('GET', KEYS[3])}";

/// The detection step. Without the group it writes nothing and returns `{0, false, false, xlen}`. With the group
/// it sets the generation to the fresh id when it is missing (`NX`), adds 1 to the epoch, and returns
/// `{1, generation, epoch, xlen}`. A write error (for example at the memory limit) is returned as it is.
///
/// KEYS: stream, gen, epoch. ARGV: group, fresh id.
const DETECT_BODY: &str = "local n = redis.call('XLEN', KEYS[1]) \
     if not has_group(KEYS[1], ARGV[1]) then return {0, false, false, n} end \
     local s = redis.pcall('SET', KEYS[2], ARGV[2], 'NX') \
     if type(s) == 'table' and s.err then return s end \
     local e = redis.pcall('INCR', KEYS[3]) \
     if type(e) == 'table' and e.err then return e end \
     return {1, redis.call('GET', KEYS[2]), e, n}";

static GROUP: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(GROUP_SCRIPT));
static SAMPLE: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(&format!("{HAS_GROUP_LUA}{SAMPLE_BODY}")));
static DETECT: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(&format!("{HAS_GROUP_LUA}{DETECT_BODY}")));

/// What the group script did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupOutcome {
    /// This call made the group, and set the generation to the fresh id.
    Created,
    /// The group was there (`BUSYGROUP`).
    Exists,
}

/// Make the group and a new generation in one step.
///
/// # Errors
/// Any Redis error other than `BUSYGROUP`.
pub async fn ensure_group(conn: &mut paigasus_redis::RedisHandle, key: &str, group: &str, fresh_id: &str) -> Result<GroupOutcome, redis::RedisError> {
    let r: String = GROUP.key(key).key(gen_key(key)).arg(group).arg(fresh_id).invoke_async(conn).await?;
    match r.as_str() {
        "created" => Ok(GroupOutcome::Created),
        "exists" => Ok(GroupOutcome::Exists),
        _ => Err(redis::RedisError::from((redis::ErrorKind::Parse, "the group script returned an unknown answer"))),
    }
}

/// What the sample script read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    pub xlen: u64,
    pub has_group: bool,
    pub generation: Option<String>,
    pub epoch: Option<u64>,
}

/// Read the sample. The script is read only.
///
/// # Errors
/// A Redis error, or an epoch value that is not a number.
pub async fn sample(conn: &mut paigasus_redis::RedisHandle, key: &str, group: &str) -> Result<Sample, redis::RedisError> {
    let (xlen, has, generation, epoch): (u64, u64, Option<String>, Option<String>) = SAMPLE.key(key).key(gen_key(key)).key(epoch_key(key)).arg(group).invoke_async(conn).await?;
    Ok(Sample {
        xlen,
        has_group: has == 1,
        generation,
        epoch: epoch.map(|e| parse_epoch(&e)).transpose()?,
    })
}

fn parse_epoch(text: &str) -> Result<u64, redis::RedisError> {
    text.parse().map_err(|_| redis::RedisError::from((redis::ErrorKind::Parse, "the epoch is not a number")))
}

/// Run the detection step (spec § 5.3 step 2).
///
/// # Errors
/// A Redis error, including a write error that the script returned.
pub async fn detect(conn: &mut paigasus_redis::RedisHandle, key: &str, group: &str, fresh_id: &str) -> Result<RedisView, redis::RedisError> {
    let (live, generation, epoch, xlen): (u64, Option<String>, Option<u64>, u64) = DETECT.key(key).key(gen_key(key)).key(epoch_key(key)).arg(group).arg(fresh_id).invoke_async(conn).await?;
    match (live, generation, epoch) {
        (1, Some(generation), Some(epoch)) => Ok(RedisView::Live { generation, epoch, xlen }),
        (0, _, _) => Ok(RedisView::NoGroup),
        _ => Err(redis::RedisError::from((redis::ErrorKind::Parse, "the detection script returned a bad tuple"))),
    }
}
