// SPDX-License-Identifier: Apache-2.0

//! `RedisLimitStore` (SMA-677 § 4.3, D16, D18, D19, D20): the limit counts on a shared Redis, so
//! every replica that points at it shares one set of counts, and a gateway restart keeps them.
//!
//! One Lua script per request reads every count, decides, and increments the rate keys only when
//! every check passes (D4); Redis runs a script atomically, so two replicas cannot both take the
//! last slot. A charge is `MULTI INCRBY EXPIRE EXEC`, spawned from `charge` on a `TaskTracker`.
//! Every instant comes from the gateway's clock; the script never calls `TIME` (D18 "Clock").
//! The connection is `paigasus_redis::connect`, with the SMA-473 budget and the SMA-476 breaker;
//! its series carry the gateway's names and `role="limits"`. A failure maps to
//! `LimitStoreError::Unavailable` and the request is admitted (fail-open, D10).

use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::SystemTime;

use metrics::counter;
use paigasus_observability::names;
use paigasus_redis::{BreakerMetrics, RedisHandle};
use tokio_util::task::TaskTracker;
use uuid::Uuid;

use crate::application::limits::{LIMITS_BREAKER_ROLE, StoreOp, report_store_unavailable};
use crate::domain::limits::{
    Budget, BudgetPeriod, ChargeDropReason, FailedCheck, LimitDecision, LimitPolicy, LimitStore, LimitStoreError, LimitTicket, MAX_TOKENS_PER_CHARGE, PeriodKey, RATE_KEY_TTL_SECS, RateCounts,
    UnavailableKind, WindowIndex, budget_refusal,
};

/// D18: every key starts with this. `v1` lets a later format use new keys with no migration.
pub const KEY_PREFIX: &str = "paigasus:gateway:limits:v1:";

/// D18: the admission script. Redis 5.0 commands only (`GET`, `INCR`, `EXPIRE`; no `EXPIRE NX`).
/// It evaluates the same D3 integer form as `domain::limits::admits`, including its
/// `elapsed.min(59_999)` clamp; the contract suite catches a difference. D23 bounds every product below 2^53, so Lua's doubles hold it exactly.
const ADMISSION_SCRIPT: &str = r#"
-- KEYS: principal current, principal previous, org current, org previous, budget.
-- ARGV: elapsed_ms, principal limit, org limit, budget limit ('' = no limit), rate TTL.
-- SOURCE OF TRUTH: `domain::limits::admits` (Rust). Keep this the same integer form, exactly:
--   elapsed  = min(elapsed_ms, 59999)
--   admit iff previous * (60000 - elapsed) + current * 60000 + 60000 <= limit * 60000
-- A gap of two or more windows needs no code: the previous key is a different, absent key, so it
-- reads 0 (the same reset the Rust form gets from an empty previous count).
local elapsed = tonumber(ARGV[1])
if elapsed > 59999 then elapsed = 59999 end
-- A stored count that is not an integer raises a script error HERE, in the read phase, before any
-- INCR: otherwise an INCR on it would fail after an earlier key was already incremented (D4).
local function count(key)
  local value = redis.call('GET', key)
  if not value then return 0 end
  local n = tonumber(value)
  if n == nil or n ~= math.floor(n) then
    error('limit key holds a non-integer count: ' .. key)
  end
  return n
end
local function rate(current_key, previous_key, limit_arg)
  if limit_arg == '' then return 1, 0, 0 end
  local limit = tonumber(limit_arg)
  local current = count(current_key)
  local previous = count(previous_key)
  if previous * (60000 - elapsed) + current * 60000 + 60000 <= limit * 60000 then
    return 1, previous, current
  end
  return 0, previous, current
end
local p_ok, p_prev, p_cur = rate(KEYS[1], KEYS[2], ARGV[2])
local o_ok, o_prev, o_cur = rate(KEYS[3], KEYS[4], ARGV[3])
local b_ok, b_used = 1, 0
if ARGV[4] ~= '' then
  b_used = count(KEYS[5])
  if b_used >= tonumber(ARGV[4]) then b_ok = 0 end
end
local admit = 0
if p_ok == 1 and o_ok == 1 and b_ok == 1 then
  admit = 1
  local ttl = tonumber(ARGV[5])
  if ARGV[2] ~= '' then
    redis.call('INCR', KEYS[1])
    redis.call('EXPIRE', KEYS[1], ttl)
  end
  if ARGV[3] ~= '' then
    redis.call('INCR', KEYS[3])
    redis.call('EXPIRE', KEYS[3], ttl)
  end
end
return {admit, p_ok, p_prev, p_cur, o_ok, o_prev, o_cur, b_ok, b_used}
"#;

pub fn principal_rate_key(principal: &str, window: u64) -> String {
    format!("{KEY_PREFIX}rate:p:{principal}:{window}")
}

pub fn org_rate_key(org: Uuid, window: u64) -> String {
    format!("{KEY_PREFIX}rate:o:{org}:{window}")
}

/// D6/D18: `…:budget:<org>:d2026-10-02`, `…:w2026-W40` or `…:m2026-10`.
pub fn budget_key(org: Uuid, period: PeriodKey) -> String {
    let kind = match period {
        PeriodKey::Day(_) => 'd',
        PeriodKey::Week { .. } => 'w',
        PeriodKey::Month { .. } => 'm',
    };
    format!("{KEY_PREFIX}budget:{org}:{kind}{}", period.label())
}

/// D15/§ 4.8: the breaker series carry the gateway's names, never IAM's.
pub fn breaker_metrics() -> BreakerMetrics {
    BreakerMetrics {
        state: names::GATEWAY_REDIS_BREAKER_STATE,
        transitions: names::GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL,
        role: LIMITS_BREAKER_ROLE,
    }
}

/// D10: `Io` covers every connection failure and the breaker's short-circuit (an `ErrorKind::Io`
/// error); `Decode` a reply of the wrong type; `Server` every other answer (an `OOM` refusal under
/// `noeviction`, a script error, `READONLY` from a replica).
fn unavailable_kind(err: &redis::RedisError) -> UnavailableKind {
    if err.is_io_error() {
        return UnavailableKind::Io;
    }
    match err.kind() {
        redis::ErrorKind::UnexpectedReturnType | redis::ErrorKind::Parse => UnavailableKind::Decode,
        _ => UnavailableKind::Server,
    }
}

/// The detail is the error kind and the server error code only. It never holds `err.to_string()`:
/// that text can carry the Redis URL, a host or a password, and the detail reaches the logs.
fn unavailable(err: &redis::RedisError) -> LimitStoreError {
    let detail = match err.code() {
        Some(code) => format!("redis error kind={:?} code={code}", err.kind()),
        None => format!("redis error kind={:?}", err.kind()),
    };
    LimitStoreError::Unavailable { kind: unavailable_kind(err), detail }
}

fn decode(detail: String) -> LimitStoreError {
    LimitStoreError::Unavailable {
        kind: UnavailableKind::Decode,
        detail,
    }
}

fn non_negative(value: i64) -> Result<u64, LimitStoreError> {
    u64::try_from(value).map_err(|_| decode(format!("the admission script returned a negative count {value}")))
}

fn limit_arg(limit: Option<NonZeroU64>) -> String {
    limit.map(|value| value.get().to_string()).unwrap_or_default()
}

/// Build the decision from the script's nine integers. The script decides; Rust only maps its
/// pass flags and counts to `FailedCheck`s, so the D8 precedence runs in one place for both
/// adapters.
fn decision_from_reply(
    reply: &[i64],
    at: WindowIndex,
    principal_limit: Option<NonZeroU64>,
    org_limit: Option<NonZeroU64>,
    budget: Option<(Uuid, Budget, PeriodKey)>,
) -> Result<LimitDecision, LimitStoreError> {
    let &[admit, p_ok, p_prev, p_cur, o_ok, o_prev, o_cur, b_ok, _b_used] = reply else {
        return Err(decode(format!("the admission script returned {} values, not 9", reply.len())));
    };
    if admit == 1 {
        return Ok(LimitDecision::Admit(budget.map(|(org, _, period)| LimitTicket { org, period })));
    }
    let counts = |previous: i64, current: i64, limit: NonZeroU64| -> Result<RateCounts, LimitStoreError> {
        Ok(RateCounts {
            previous: non_negative(previous)?,
            current: non_negative(current)?,
            elapsed_ms: at.elapsed_ms,
            limit: limit.get(),
        })
    };
    let mut failed = Vec::new();
    if let (0, Some(limit)) = (p_ok, principal_limit) {
        failed.push(FailedCheck::PrincipalRate(counts(p_prev, p_cur, limit)?));
    }
    if let (0, Some(limit)) = (o_ok, org_limit) {
        failed.push(FailedCheck::OrgRate(counts(o_prev, o_cur, limit)?));
    }
    if let (0, Some((_, budget, key))) = (b_ok, budget) {
        failed.push(budget_refusal(budget, key));
    }
    Ok(LimitDecision::Refused(failed))
}

/// The shared-Redis store. `Clone` shares the handle (one breaker), the script and the tracker.
#[derive(Clone)]
pub struct RedisLimitStore {
    handle: RedisHandle,
    script: Arc<redis::Script>,
    tracker: TaskTracker,
}

impl RedisLimitStore {
    /// D17: eager. A Redis that is down at boot fails the boot, so a wrong URL or password is
    /// found at deploy time (Q14). Call after `paigasus_observability::init`: the breaker sets its
    /// gauge in its constructor.
    pub async fn connect(redis_url: &str) -> redis::RedisResult<Self> {
        Ok(Self::from_handle(paigasus_redis::connect(redis_url, breaker_metrics()).await?))
    }

    pub fn from_handle(handle: RedisHandle) -> Self {
        RedisLimitStore {
            handle,
            script: Arc::new(redis::Script::new(ADMISSION_SCRIPT)),
            tracker: TaskTracker::new(),
        }
    }

    /// D16: the spawned charges, for the shutdown drain (`runtime::supervise`).
    pub fn tracker(&self) -> TaskTracker {
        self.tracker.clone()
    }

    /// D18: `MULTI INCRBY EXPIRE EXEC` through the breaker (`RedisHandle` implements
    /// `req_packed_commands`). The TTL is relative: `period_end + 86400 − now`. A charge more than
    /// a day late writes nothing and is counted (`period_expired`). Zero tokens send nothing.
    pub async fn apply_charge(&self, ticket: LimitTicket, tokens: u64, now: SystemTime) -> Result<(), LimitStoreError> {
        let tokens = tokens.min(MAX_TOKENS_PER_CHARGE);
        if tokens == 0 {
            return Ok(());
        }
        let Some(ttl) = BudgetPeriod::charge_ttl(ticket.period, now) else {
            counter!(names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, "reason" => ChargeDropReason::PeriodExpired.as_label()).increment(1);
            return Ok(());
        };
        let key = budget_key(ticket.org, ticket.period);
        let mut con = self.handle.clone();
        redis::pipe()
            .atomic()
            .incr(&key, tokens)
            .ignore()
            .expire(&key, i64::from(ttl))
            .ignore()
            .query_async::<()>(&mut con)
            .await
            .map_err(|err| unavailable(&err))
    }
}

#[async_trait::async_trait]
impl LimitStore for RedisLimitStore {
    async fn check_and_admit(&self, principal: &str, org: Option<Uuid>, policy: &LimitPolicy, now: SystemTime) -> Result<LimitDecision, LimitStoreError> {
        let at = WindowIndex::at(now);
        let previous = at.index.saturating_sub(1);
        let org_limit = org.and(policy.org_requests_per_minute);
        let budget = policy.budget.zip(org).map(|(budget, id)| (id, budget, budget.period.key_at(now)));
        // A dimension with no limit passes an empty limit; the script never touches its key.
        let (org_current, org_previous) = match org {
            Some(id) => (org_rate_key(id, at.index), org_rate_key(id, previous)),
            None => (format!("{KEY_PREFIX}rate:o:none:{}", at.index), format!("{KEY_PREFIX}rate:o:none:{previous}")),
        };
        let budget_name = budget.map_or_else(|| format!("{KEY_PREFIX}budget:none"), |(id, _, key)| budget_key(id, key));
        let mut invocation = self.script.prepare_invoke();
        invocation
            .key(principal_rate_key(principal, at.index))
            .key(principal_rate_key(principal, previous))
            .key(org_current)
            .key(org_previous)
            .key(budget_name)
            .arg(at.elapsed_ms)
            .arg(limit_arg(policy.principal_requests_per_minute))
            .arg(limit_arg(org_limit))
            .arg(limit_arg(budget.map(|(_, budget, _)| budget.tokens)))
            .arg(RATE_KEY_TTL_SECS);
        // `invoke_async` sends EVALSHA, and on NOSCRIPT sends SCRIPT LOAD and EVALSHA again (D19).
        let mut con = self.handle.clone();
        let reply: Vec<i64> = invocation.invoke_async(&mut con).await.map_err(|err| unavailable(&err))?;
        decision_from_reply(&reply, at, policy.principal_requests_per_minute, org_limit, budget)
    }

    /// D16: never blocks, never panics. With a runtime, the charge runs on the tracker; the task
    /// holds a clone of the handle only, never request state. With no runtime (a drop on a plain
    /// thread), it is counted as `no_runtime` and dropped.
    fn charge(&self, ticket: LimitTicket, tokens: u64, now: SystemTime) {
        if tokens == 0 {
            return;
        }
        // The drain closed the tracker: the process is stopping, so count the charge instead of
        // spawning a task nobody waits for.
        if self.tracker.is_closed() {
            counter!(names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, "reason" => ChargeDropReason::Shutdown.as_label()).increment(1);
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            counter!(names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, "reason" => ChargeDropReason::NoRuntime.as_label()).increment(1);
            return;
        };
        let store = self.clone();
        self.tracker.spawn_on(
            async move {
                // No retry: a retry after an ambiguous failure would charge twice (D10).
                if let Err(LimitStoreError::Unavailable { kind, detail }) = store.apply_charge(ticket, tokens, now).await {
                    report_store_unavailable(StoreOp::Charge, kind, &detail);
                }
            },
            &runtime,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::charge_guard::{ChargeGuard, NoTicket};
    use crate::application::limits::Limits;
    use crate::domain::limits::{BudgetPeriod, LimitRules};
    use crate::test_support::{FixedClock, ORG_A, ORG_A_PRN, at, caller, counter};
    use metrics_util::debugging::DebuggingRecorder;

    fn nz(n: u64) -> NonZeroU64 {
        NonZeroU64::new(n).expect("non-zero")
    }

    #[test]
    fn errors_map_to_their_kinds() {
        let io = redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused"));
        let short_circuit = redis::RedisError::from((redis::ErrorKind::Io, paigasus_redis::BREAKER_OPEN_MESSAGE));
        let server = redis::RedisError::from((
            redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError),
            "OOM command not allowed when used memory > 'maxmemory'",
        ));
        let decode = redis::RedisError::from((redis::ErrorKind::UnexpectedReturnType, "not an array of integers"));
        let parse = redis::RedisError::from((redis::ErrorKind::Parse, "bad reply"));
        let kinds = [io, short_circuit, server, decode, parse].map(|err| unavailable_kind(&err));
        assert_eq!(
            kinds,
            [UnavailableKind::Io, UnavailableKind::Io, UnavailableKind::Server, UnavailableKind::Decode, UnavailableKind::Decode]
        );
    }

    #[test]
    fn the_keys_follow_the_v1_format() {
        let org = ORG_A;
        assert_eq!(principal_rate_key("prn:pgs:iam:::principal/p", 7), "paigasus:gateway:limits:v1:rate:p:prn:pgs:iam:::principal/p:7");
        assert_eq!(org_rate_key(org, 7), "paigasus:gateway:limits:v1:rate:o:0190a100-0000-7000-8000-0000000000a1:7");
        let noon = at("2026-10-02T12:00:00Z");
        assert_eq!(
            budget_key(org, BudgetPeriod::Daily.key_at(noon)),
            "paigasus:gateway:limits:v1:budget:0190a100-0000-7000-8000-0000000000a1:d2026-10-02"
        );
        assert_eq!(
            budget_key(org, BudgetPeriod::Weekly.key_at(noon)),
            "paigasus:gateway:limits:v1:budget:0190a100-0000-7000-8000-0000000000a1:w2026-W40"
        );
        assert_eq!(
            budget_key(org, BudgetPeriod::Monthly.key_at(noon)),
            "paigasus:gateway:limits:v1:budget:0190a100-0000-7000-8000-0000000000a1:m2026-10"
        );
    }

    #[test]
    fn a_script_reply_becomes_a_decision() {
        let at_noon = WindowIndex::at(at("2026-10-02T12:00:30Z"));
        let budget = Budget {
            tokens: nz(10),
            period: BudgetPeriod::Monthly,
        };
        let key = BudgetPeriod::Monthly.key_at(at("2026-10-02T12:00:30Z"));
        let admitted = decision_from_reply(&[1, 1, 0, 0, 1, 0, 0, 1, 3], at_noon, Some(nz(5)), Some(nz(5)), Some((ORG_A, budget, key))).expect("a valid reply");
        assert_eq!(admitted, LimitDecision::Admit(Some(LimitTicket { org: ORG_A, period: key })));
        let refused = decision_from_reply(&[0, 0, 2, 5, 1, 0, 0, 0, 12], at_noon, Some(nz(5)), Some(nz(5)), Some((ORG_A, budget, key))).expect("a valid reply");
        assert_eq!(
            refused,
            LimitDecision::Refused(vec![
                FailedCheck::PrincipalRate(RateCounts {
                    previous: 2,
                    current: 5,
                    elapsed_ms: 30_000,
                    limit: 5
                }),
                FailedCheck::Budget {
                    period: BudgetPeriod::Monthly,
                    resets_at_unix: 1_793_491_200
                },
            ])
        );
    }

    #[test]
    fn a_reply_of_the_wrong_shape_is_a_decode_failure() {
        let at_noon = WindowIndex::at(at("2026-10-02T12:00:00Z"));
        for reply in [&[1_i64, 1, 0][..], &[0, 0, -1, 5, 1, 0, 0, 1, 0][..]] {
            match decision_from_reply(reply, at_noon, Some(nz(5)), None, None) {
                Err(LimitStoreError::Unavailable { kind: UnavailableKind::Decode, .. }) => {}
                other => panic!("{reply:?} must be a decode failure, got {other:?}"),
            }
        }
    }

    /// Review ruling: the detail never holds a URL, host or password, whatever the error text says.
    #[test]
    fn the_detail_never_holds_a_url_or_a_password() {
        let url = "redis://:s3cret-pw@redis.internal.example:6379/0";
        let io = redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, format!("cannot reach {url}")));
        let parse = redis::RedisError::from((redis::ErrorKind::Parse, "bad reply", url.to_string()));
        for err in [io, parse] {
            let LimitStoreError::Unavailable { detail, .. } = unavailable(&err);
            for secret in ["s3cret-pw", "redis.internal.example", "redis://", "6379"] {
                assert!(!detail.contains(secret), "detail {detail:?} leaks {secret:?}");
            }
            assert!(!detail.is_empty());
        }
    }

    fn budget_rules() -> LimitRules {
        LimitRules {
            tokens_per_period: Some(nz(100)),
            ..LimitRules::default()
        }
    }

    /// A12 on an open breaker: admitted at once, no ticket, no dial, `op="check",kind="io"`.
    #[tokio::test]
    async fn an_open_breaker_admits_without_a_ticket_and_never_dials() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let blackhole = paigasus_redis::test_support::start().await;
        let store = RedisLimitStore::from_handle(paigasus_redis::with_open_breaker_for_tests(&blackhole.url, breaker_metrics()).expect("a lazy handle"));
        let limits = Arc::new(Limits::new(budget_rules(), Arc::new(store), Arc::new(FixedClock(at("2026-10-02T12:00:00Z")))));
        let guard = limits.admit(&caller(ORG_A_PRN)).await.expect("fail-open admits");
        assert!(!guard.has_ticket());
        assert_eq!(blackhole.accepted(), 0, "an open breaker short-circuits with no dial (SMA-702)");
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL, &[("op", "check"), ("kind", "io")]), Some(1));
    }

    /// D20: a blackhole with a closed breaker. Each of the first three checks waits for the dial
    /// budget (about 2.1 s) and fails open; then the breaker is open and the fourth dials nothing.
    #[tokio::test]
    async fn a_blackholed_redis_fails_open_until_the_breaker_stops_the_dials() {
        let blackhole = paigasus_redis::test_support::start().await;
        let store = RedisLimitStore::from_handle(paigasus_redis::new_lazy_for_tests(&blackhole.url, breaker_metrics()).expect("a lazy handle"));
        let policy = LimitPolicy {
            principal_requests_per_minute: Some(nz(5)),
            ..LimitPolicy::default()
        };
        let now = at("2026-10-02T12:00:00Z");
        for attempt in 1..=3 {
            match store.check_and_admit("p", None, &policy, now).await {
                Err(LimitStoreError::Unavailable { kind: UnavailableKind::Io, .. }) => {}
                other => panic!("attempt {attempt}: expected an io failure, got {other:?}"),
            }
        }
        let dialled = blackhole.accepted();
        assert!(dialled >= 3, "the closed breaker dialled on every attempt: {dialled}");
        assert!(matches!(
            store.check_and_admit("p", None, &policy, now).await,
            Err(LimitStoreError::Unavailable { kind: UnavailableKind::Io, .. })
        ));
        assert_eq!(blackhole.accepted(), dialled, "after three failures the breaker is open: no new dial");
    }

    /// D10: a charge against an open breaker is counted as `op="charge",kind="io"` and never panics.
    #[tokio::test]
    async fn a_charge_against_an_open_breaker_is_counted() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let blackhole = paigasus_redis::test_support::start().await;
        let store = RedisLimitStore::from_handle(paigasus_redis::with_open_breaker_for_tests(&blackhole.url, breaker_metrics()).expect("a lazy handle"));
        let now = at("2026-10-02T12:00:00Z");
        store.charge(
            LimitTicket {
                org: ORG_A,
                period: BudgetPeriod::Monthly.key_at(now),
            },
            5,
            now,
        );
        let tracker = store.tracker();
        tracker.close();
        tracker.wait().await;
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL, &[("op", "charge"), ("kind", "io")]), Some(1));
    }

    /// Carry (b): a charge that arrives after the drain closed the tracker is counted as
    /// `reason="shutdown"`, not spawned and not lost uncounted.
    #[tokio::test]
    async fn a_charge_after_the_drain_closed_the_tracker_is_counted_as_shutdown() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let blackhole = paigasus_redis::test_support::start().await;
        let store = RedisLimitStore::from_handle(paigasus_redis::with_open_breaker_for_tests(&blackhole.url, breaker_metrics()).expect("a lazy handle"));
        let now = at("2026-10-02T12:00:00Z");
        let tracker = store.tracker();
        tracker.close();
        store.charge(
            LimitTicket {
                org: ORG_A,
                period: BudgetPeriod::Monthly.key_at(now),
            },
            5,
            now,
        );
        assert_eq!(tracker.len(), 0, "nothing was spawned");
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, &[("reason", "shutdown")]), Some(1));
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL, &[("op", "charge"), ("kind", "io")]), None);
    }

    /// D16, spec § 5.8: a guard with a ticket dropped on a plain thread (no runtime) counts
    /// `reason="no_runtime"` and does not panic.
    #[tokio::test]
    async fn a_guard_dropped_without_a_runtime_counts_no_runtime() {
        let blackhole = paigasus_redis::test_support::start().await;
        let store = RedisLimitStore::from_handle(paigasus_redis::with_open_breaker_for_tests(&blackhole.url, breaker_metrics()).expect("a lazy handle"));
        let now = at("2026-10-02T12:00:00Z");
        let limits = Arc::new(Limits::new(budget_rules(), Arc::new(store), Arc::new(FixedClock(now))));
        let mut guard = ChargeGuard::new(
            limits,
            Some(LimitTicket {
                org: ORG_A,
                period: BudgetPeriod::Monthly.key_at(now),
            }),
            Some(ORG_A),
            NoTicket::NoBudget,
        );
        guard.mark_sent();
        guard.set_reported(5);
        let dropped = std::thread::spawn(move || {
            let recorder = DebuggingRecorder::new();
            let snapshotter = recorder.snapshotter();
            metrics::with_local_recorder(&recorder, || drop(guard));
            counter(&snapshotter, names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, &[("reason", "no_runtime")])
        })
        .join()
        .expect("Drop must not panic without a runtime");
        assert_eq!(dropped, Some(1));
    }
}
