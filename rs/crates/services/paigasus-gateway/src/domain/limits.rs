// SPDX-License-Identifier: Apache-2.0

//! SMA-677: the pure rules of the gateway's rate limit and token budget, and the store port.
//!
//! No dependency on `redis`, `tokio`, `metrics` or the config types (spec § 4.2). Both store
//! adapters compute every decision with the functions in this file, so they agree to the request
//! (D3). No function here reads a clock: every instant is a parameter.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Days, Months, NaiveDate, NaiveTime, Utc, Weekday};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The D3 rate window, in milliseconds.
pub const RATE_WINDOW_MS: u64 = 60_000;
/// D18: a rate key lives 180 s after its last increment: two windows plus one window of margin
/// for clock skew between replicas.
pub const RATE_KEY_TTL_SECS: u32 = 180;
/// D23: the largest `principal_requests_per_minute` and `org_requests_per_minute`.
pub const MAX_REQUESTS_PER_MINUTE: u64 = 1_000_000_000;
/// D23: the largest `tokens_per_period`.
pub const MAX_TOKENS_PER_PERIOD: u64 = 1_000_000_000_000;
/// D23: one charge is clamped to this many tokens. A larger usage record is a bad record.
pub const MAX_TOKENS_PER_CHARGE: u64 = 1_000_000_000;

/// Milliseconds since the epoch. A clock before the epoch reads as 0, never a panic (D10).
fn unix_ms(now: SystemTime) -> u64 {
    let ms = now.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_millis();
    u64::try_from(ms).unwrap_or(u64::MAX)
}

/// Where `now` falls in the D3 windows: `index = floor(unix_ms / 60000)`, and the milliseconds
/// elapsed in that window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowIndex {
    pub index: u64,
    pub elapsed_ms: u64,
}

impl WindowIndex {
    pub fn at(now: SystemTime) -> Self {
        let ms = unix_ms(now);
        WindowIndex {
            index: ms / RATE_WINDOW_MS,
            elapsed_ms: ms % RATE_WINDOW_MS,
        }
    }
}

/// D3: `previous × (60000 − elapsed_ms) + current × 60000 + 60000 ≤ limit × 60000`. The Lua
/// script (Task 11) evaluates the same form. Saturating, so no input can panic (D10).
pub fn admits(previous: u64, current: u64, elapsed_ms: u64, limit: u64) -> bool {
    let elapsed = elapsed_ms.min(RATE_WINDOW_MS - 1);
    let weighted = previous
        .saturating_mul(RATE_WINDOW_MS - elapsed)
        .saturating_add(current.saturating_mul(RATE_WINDOW_MS))
        .saturating_add(RATE_WINDOW_MS);
    weighted <= limit.saturating_mul(RATE_WINDOW_MS)
}

/// Milliseconds until `admits` holds, if no other request arrives in between.
fn retry_after_ms(previous: u64, current: u64, elapsed_ms: u64, limit: u64) -> u64 {
    let elapsed = elapsed_ms.min(RATE_WINDOW_MS - 1);
    if admits(previous, current, elapsed, limit) {
        return 0;
    }
    // Same window: the previous count's weight falls as `elapsed` grows. With
    // budget = (limit − current − 1) × 60000, admission needs elapsed ≥ 60000 − ⌊budget / previous⌋.
    if previous > 0 && current < limit {
        let budget = (limit - current - 1).saturating_mul(RATE_WINDOW_MS);
        let needed = RATE_WINDOW_MS.saturating_sub(budget / previous);
        if needed < RATE_WINDOW_MS {
            return needed.saturating_sub(elapsed);
        }
    }
    // Next window: `current` becomes the previous count and the new current count is 0. A needed
    // offset of a full window means the window after next, where both counts read 0.
    let to_next = RATE_WINDOW_MS - elapsed;
    if current == 0 {
        return to_next;
    }
    let budget = limit.saturating_sub(1).saturating_mul(RATE_WINDOW_MS);
    to_next.saturating_add(RATE_WINDOW_MS.saturating_sub(budget / current))
}

/// The `Retry-After` for a refused rate check, in whole seconds, at least 1 (D8).
pub fn retry_after(previous: u64, current: u64, elapsed_ms: u64, limit: u64) -> u32 {
    let ms = retry_after_ms(previous, current, elapsed_ms, limit);
    u32::try_from(ms.div_ceil(1000).max(1)).unwrap_or(u32::MAX)
}

/// The counts one rate check saw. A refusal carries them, so `Retry-After` and the D8 precedence
/// are computed in Rust for both adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateCounts {
    pub previous: u64,
    pub current: u64,
    pub elapsed_ms: u64,
    pub limit: u64,
}

impl RateCounts {
    pub fn admits(&self) -> bool {
        admits(self.previous, self.current, self.elapsed_ms, self.limit)
    }

    pub fn retry_after_secs(&self) -> u32 {
        retry_after(self.previous, self.current, self.elapsed_ms, self.limit)
    }
}

/// The D3 state of one key: the counts of the last window that saw a request and of the window
/// before it. Two integers, O(1) memory per key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlidingWindow {
    window: u64,
    previous: u64,
    current: u64,
}

impl SlidingWindow {
    /// The counts as seen at `now`. A backward clock step (an earlier window) reads the stored
    /// window at 0 ms elapsed (D10).
    pub fn counts(&self, now: SystemTime, limit: NonZeroU64) -> RateCounts {
        let at = WindowIndex::at(now);
        let (previous, current, elapsed_ms) = if at.index == self.window {
            (self.previous, self.current, at.elapsed_ms)
        } else if at.index == self.window.saturating_add(1) {
            (self.current, 0, at.elapsed_ms)
        } else if at.index > self.window {
            (0, 0, at.elapsed_ms)
        } else {
            (self.previous, self.current, 0)
        };
        RateCounts {
            previous,
            current,
            elapsed_ms,
            limit: limit.get(),
        }
    }

    /// D4: pure. `Err` carries the counts that refused the request.
    pub fn check(&self, now: SystemTime, limit: NonZeroU64) -> Result<(), RateCounts> {
        let counts = self.counts(now, limit);
        if counts.admits() { Ok(()) } else { Err(counts) }
    }

    /// D4: count one admitted request at `now`. Never moves the window backward.
    pub fn commit(&mut self, now: SystemTime) {
        let at = WindowIndex::at(now);
        if at.index > self.window {
            self.previous = if at.index == self.window.saturating_add(1) { self.current } else { 0 };
            self.current = 0;
            self.window = at.index;
        }
        self.current = self.current.saturating_add(1);
    }
}

/// D7/D18: a budget period accepts a late charge for one day after it ends.
pub const LATE_CHARGE_GRACE_SECS: i64 = 86_400;

/// D6: the UTC calendar period of a token budget.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BudgetPeriod {
    Daily,
    Weekly,
    #[default]
    Monthly,
}

/// One concrete budget period. Carried as `Copy` values; formatted only for the Redis key and the
/// refusal message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeriodKey {
    Day(NaiveDate),
    /// The ISO 8601 week-year and week (`iso_week()`), not the calendar year.
    Week {
        year: i32,
        week: u32,
    },
    Month {
        year: i32,
        month: u32,
    },
}

impl BudgetPeriod {
    /// The period that holds `now`. Every replica derives the same key from the instant alone.
    pub fn key_at(self, now: SystemTime) -> PeriodKey {
        let secs = match now.duration_since(UNIX_EPOCH) {
            Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
            Err(before) => i64::try_from(before.duration().as_secs()).map_or(i64::MIN, |s| -s),
        };
        self.key_at_unix(secs)
    }

    /// Total for every `i64`: an instant outside chrono's range clamps to its first or last date,
    /// so no caller can panic on a hostile or corrupt timestamp.
    fn key_at_unix(self, secs: i64) -> PeriodKey {
        let date = match DateTime::<Utc>::from_timestamp(secs, 0) {
            Some(instant) => instant.date_naive(),
            None if secs < 0 => NaiveDate::MIN,
            None => NaiveDate::MAX,
        };
        match self {
            BudgetPeriod::Daily => PeriodKey::Day(date),
            BudgetPeriod::Weekly => {
                let week = date.iso_week();
                PeriodKey::Week { year: week.year(), week: week.week() }
            }
            BudgetPeriod::Monthly => PeriodKey::Month {
                year: date.year(),
                month: date.month(),
            },
        }
    }

    /// D18: the relative TTL of the budget key for a charge at `now`: `period_end + 86400 − now`
    /// in whole seconds. `None` when that is below 1 s: the charge arrives more than one day after
    /// its period ended, and is dropped and counted (`period_expired`).
    pub fn charge_ttl(key: PeriodKey, now: SystemTime) -> Option<u32> {
        let now_secs = i64::try_from(now.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs()).unwrap_or(i64::MAX);
        let ttl = key.resets_at_unix().saturating_add(LATE_CHARGE_GRACE_SECS).saturating_sub(now_secs);
        if ttl < 1 { None } else { u32::try_from(ttl).ok() }
    }

    /// D8: the label of the period that resets at `resets_at_unix`, from the instant one second
    /// before the reset ("2026-10", "2026-W40", "2026-10-02").
    pub fn label_at_reset(self, resets_at_unix: i64) -> String {
        self.key_at_unix(resets_at_unix.saturating_sub(1).max(0)).label()
    }
}

impl PeriodKey {
    fn start_date(self) -> NaiveDate {
        match self {
            PeriodKey::Day(date) => Some(date),
            PeriodKey::Week { year, week } => NaiveDate::from_isoywd_opt(year, week, Weekday::Mon),
            PeriodKey::Month { year, month } => NaiveDate::from_ymd_opt(year, month, 1),
        }
        .unwrap_or(NaiveDate::MIN)
    }

    fn end_date(self) -> NaiveDate {
        let start = self.start_date();
        match self {
            PeriodKey::Day(_) => start.checked_add_days(Days::new(1)),
            PeriodKey::Week { .. } => start.checked_add_days(Days::new(7)),
            PeriodKey::Month { .. } => start.checked_add_months(Months::new(1)),
        }
        .unwrap_or(NaiveDate::MAX)
    }

    pub fn start(self) -> DateTime<Utc> {
        self.start_date().and_time(NaiveTime::MIN).and_utc()
    }

    /// The reset instant: the first instant of the next period (exclusive end).
    pub fn end(self) -> DateTime<Utc> {
        self.end_date().and_time(NaiveTime::MIN).and_utc()
    }

    pub fn resets_at_unix(self) -> i64 {
        self.end().timestamp()
    }

    pub fn label(self) -> String {
        match self {
            PeriodKey::Day(date) => date.format("%Y-%m-%d").to_string(),
            PeriodKey::Week { year, week } => format!("{year:04}-W{week:02}"),
            PeriodKey::Month { year, month } => format!("{year:04}-{month:02}"),
        }
    }
}

/// An org's token budget for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub tokens: NonZeroU64,
    pub period: BudgetPeriod,
}

/// The resolved limits of one request. `None` on a dimension means no limit on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LimitPolicy {
    pub principal_requests_per_minute: Option<NonZeroU64>,
    pub org_requests_per_minute: Option<NonZeroU64>,
    pub budget: Option<Budget>,
}

impl LimitPolicy {
    /// D11: an empty policy skips the store.
    pub fn is_empty(&self) -> bool {
        self.principal_requests_per_minute.is_none() && !self.has_org_dimension()
    }

    /// D2: an org rate or a budget applies.
    pub fn has_org_dimension(&self) -> bool {
        self.org_requests_per_minute.is_some() || self.budget.is_some()
    }
}

/// One `[[limits.org]]` entry (D12). A field that is `None` keeps the table default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OrgOverride {
    pub org_requests_per_minute: Option<NonZeroU64>,
    pub tokens_per_period: Option<NonZeroU64>,
    /// D11: no org rate and no budget for this org; the principal rate still applies.
    pub exempt: bool,
}

/// The table defaults and the per-org overrides, read once at boot. Domain-owned, so the config
/// type stays out of the domain (`LimitsConfig::rules` converts).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LimitRules {
    pub principal_requests_per_minute: Option<NonZeroU64>,
    pub org_requests_per_minute: Option<NonZeroU64>,
    pub tokens_per_period: Option<NonZeroU64>,
    pub budget_period: BudgetPeriod,
    pub overrides: HashMap<Uuid, OrgOverride>,
}

impl LimitRules {
    pub fn policy_for(&self, org: Option<Uuid>) -> LimitPolicy {
        let entry = org.and_then(|id| self.overrides.get(&id)).copied().unwrap_or_default();
        if entry.exempt {
            return LimitPolicy {
                principal_requests_per_minute: self.principal_requests_per_minute,
                ..LimitPolicy::default()
            };
        }
        LimitPolicy {
            principal_requests_per_minute: self.principal_requests_per_minute,
            org_requests_per_minute: entry.org_requests_per_minute.or(self.org_requests_per_minute),
            budget: entry.tokens_per_period.or(self.tokens_per_period).map(|tokens| Budget { tokens, period: self.budget_period }),
        }
    }

    /// D11: true when no table default is set and no override sets a value, so no request can
    /// ever have a non-empty policy. `main.rs` then builds no store.
    pub fn every_policy_is_empty(&self) -> bool {
        self.principal_requests_per_minute.is_none()
            && self.org_requests_per_minute.is_none()
            && self.tokens_per_period.is_none()
            && self.overrides.values().all(|entry| entry.org_requests_per_minute.is_none() && entry.tokens_per_period.is_none())
    }
}

/// One check that refused a request. A refusal carries every failed check (D8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailedCheck {
    PrincipalRate(RateCounts),
    OrgRate(RateCounts),
    Budget { period: BudgetPeriod, resets_at_unix: i64 },
}

/// The failed budget check for `budget` in the period `key`.
pub fn budget_refusal(budget: Budget, key: PeriodKey) -> FailedCheck {
    FailedCheck::Budget {
        period: budget.period,
        resets_at_unix: key.resets_at_unix(),
    }
}

/// The right to charge one admitted request. It exists only when a budget applies, names the
/// period the request started in (D7), and is not `Clone`: the charge guard gives it back to
/// `LimitStore::charge` exactly once.
#[derive(Debug, PartialEq, Eq)]
pub struct LimitTicket {
    pub org: Uuid,
    pub period: PeriodKey,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LimitDecision {
    Admit(Option<LimitTicket>),
    Refused(Vec<FailedCheck>),
}

/// The `reason` label of `gateway_limit_refusals_total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    PrincipalRate,
    OrgRate,
    OrgBudget,
}

impl RefusalReason {
    pub const ALL: [RefusalReason; 3] = [RefusalReason::PrincipalRate, RefusalReason::OrgRate, RefusalReason::OrgBudget];

    pub fn as_label(self) -> &'static str {
        match self {
            RefusalReason::PrincipalRate => "principal_rate",
            RefusalReason::OrgRate => "org_rate",
            RefusalReason::OrgBudget => "org_budget",
        }
    }
}

/// Why the limits refused a request. The HTTP adapter maps it to a `GatewayError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitRefusal {
    RateLimited {
        retry_after_secs: u32,
        reason: RefusalReason,
    },
    BudgetExhausted {
        period: BudgetPeriod,
        resets_at_unix: i64,
    },
    /// D2: the scope names no org, and an org rate or a budget applies (fail-closed, Q11).
    Unscoped,
}

impl LimitRefusal {
    /// The refusal-metric label; `None` for `Unscoped`, which has its own counter.
    pub fn reason(&self) -> Option<RefusalReason> {
        match self {
            LimitRefusal::RateLimited { reason, .. } => Some(*reason),
            LimitRefusal::BudgetExhausted { .. } => Some(RefusalReason::OrgBudget),
            LimitRefusal::Unscoped => None,
        }
    }

    /// A short name for the refusal log line.
    pub fn label(&self) -> &'static str {
        self.reason().map_or("unscoped", RefusalReason::as_label)
    }
}

/// D8: the org budget first, then the principal rate, then the org rate. `Retry-After` is the
/// largest wait over every failed rate check, at least 1 s. `None` for an empty list.
pub fn choose_refusal(failed: &[FailedCheck]) -> Option<LimitRefusal> {
    if let Some((period, resets_at_unix)) = failed.iter().find_map(|check| match check {
        FailedCheck::Budget { period, resets_at_unix } => Some((*period, *resets_at_unix)),
        _ => None,
    }) {
        return Some(LimitRefusal::BudgetExhausted { period, resets_at_unix });
    }
    let retry_after_secs = failed
        .iter()
        .filter_map(|check| match check {
            FailedCheck::PrincipalRate(counts) | FailedCheck::OrgRate(counts) => Some(counts.retry_after_secs()),
            FailedCheck::Budget { .. } => None,
        })
        .max()?;
    let reason = if failed.iter().any(|check| matches!(check, FailedCheck::PrincipalRate(_))) {
        RefusalReason::PrincipalRate
    } else {
        RefusalReason::OrgRate
    };
    Some(LimitRefusal::RateLimited {
        retry_after_secs: retry_after_secs.max(1),
        reason,
    })
}

/// The `reason` label of `gateway_limit_charges_dropped_total` (D16, D18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargeDropReason {
    NoRuntime,
    Shutdown,
    PeriodExpired,
}

impl ChargeDropReason {
    pub const ALL: [ChargeDropReason; 3] = [ChargeDropReason::NoRuntime, ChargeDropReason::Shutdown, ChargeDropReason::PeriodExpired];

    pub fn as_label(self) -> &'static str {
        match self {
            ChargeDropReason::NoRuntime => "no_runtime",
            ChargeDropReason::Shutdown => "shutdown",
            ChargeDropReason::PeriodExpired => "period_expired",
        }
    }
}

/// D10: why a store call failed. `Copy`, and a bounded metric label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnavailableKind {
    /// Redis is down, the connection failed, or the breaker is open.
    Io,
    /// Redis answered with an error (for example `OOM` under `noeviction`, or a script error).
    Server,
    /// Redis answered with a reply of the wrong type or shape.
    Decode,
}

impl UnavailableKind {
    pub const ALL: [UnavailableKind; 3] = [UnavailableKind::Io, UnavailableKind::Server, UnavailableKind::Decode];

    pub fn as_label(self) -> &'static str {
        match self {
            UnavailableKind::Io => "io",
            UnavailableKind::Server => "server",
            UnavailableKind::Decode => "decode",
        }
    }
}

/// D10: the port's error. It names no `redis` type; the Redis adapter maps its errors here.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LimitStoreError {
    /// `detail` is the store error's text. It never contains the store URL.
    #[error("limit store unavailable ({kind:?}): {detail}")]
    Unavailable { kind: UnavailableKind, detail: String },
}

/// D1/D16: the limit store port. Consumed as `dyn LimitStore`, hence `async_trait`
/// (`rs/Cargo.toml:102-104`).
#[async_trait::async_trait]
pub trait LimitStore: Send + Sync {
    /// D4: check every dimension of `policy`, then count the request on every rate key only when
    /// every check passed. A ticket is issued only when a budget applies and `org` is `Some`.
    async fn check_and_admit(&self, principal: &str, org: Option<Uuid>, policy: &LimitPolicy, now: SystemTime) -> Result<LimitDecision, LimitStoreError>;

    /// D7/D16: add `tokens` to the ticket's period. Must not block and must not panic: the charge
    /// guard calls it from `Drop`. An adapter that needs I/O spawns it.
    fn charge(&self, ticket: LimitTicket, tokens: u64, now: SystemTime);
}

/// The injected clock (spec § 4.3a).
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

/// The production clock. The only place in the domain and application layers that reads the
/// system time.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-02T12:00:00Z, which is a window start (1_790_942_400 is a multiple of 60).
    const T0_MS: u64 = 1_790_942_400_000;

    fn at_ms(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    fn nz(n: u64) -> NonZeroU64 {
        NonZeroU64::new(n).expect("a non-zero test limit")
    }

    #[test]
    fn the_window_index_splits_unix_milliseconds() {
        assert_eq!(WindowIndex::at(at_ms(T0_MS)), WindowIndex { index: T0_MS / 60_000, elapsed_ms: 0 });
        assert_eq!(
            WindowIndex::at(at_ms(T0_MS + 59_999)),
            WindowIndex {
                index: T0_MS / 60_000,
                elapsed_ms: 59_999
            }
        );
        assert_eq!(
            WindowIndex::at(at_ms(T0_MS + 60_000)),
            WindowIndex {
                index: T0_MS / 60_000 + 1,
                elapsed_ms: 0
            }
        );
        // A clock before the epoch reads as the epoch, not a panic.
        assert_eq!(WindowIndex::at(UNIX_EPOCH - Duration::from_secs(5)), WindowIndex { index: 0, elapsed_ms: 0 });
    }

    /// (previous, current, elapsed_ms, limit, admitted) — the D3 integer form, row by row.
    const ROWS: &[(u64, u64, u64, u64, bool)] = &[
        (0, 0, 0, 1, true),        // the first request ever
        (0, 1, 0, 1, false),       // a limit of 1, second request
        (1, 0, 0, 1, false),       // window edge: the previous count weighs fully at 0 ms
        (1, 0, 59_999, 1, false),  // and still weighs 1/60000 at the last millisecond
        (2, 0, 30_000, 2, true),   // 2 × 30000 + 60000 = 120000 ≤ 120000
        (2, 0, 29_999, 2, false),  // one millisecond earlier it is over
        (0, 2, 59_999, 2, false),  // the current count alone is at the limit
        (10, 4, 30_000, 10, true), // 300000 + 240000 + 60000 = 600000
        (10, 5, 30_000, 10, false),
        (0, MAX_REQUESTS_PER_MINUTE - 1, 0, MAX_REQUESTS_PER_MINUTE, true), // the D23 maximum
        (0, MAX_REQUESTS_PER_MINUTE, 0, MAX_REQUESTS_PER_MINUTE, false),
        (u64::MAX, u64::MAX, 0, MAX_REQUESTS_PER_MINUTE, false), // saturates, no panic
    ];

    #[test]
    fn admits_follows_the_d3_integer_form() {
        for &(previous, current, elapsed, limit, want) in ROWS {
            assert_eq!(admits(previous, current, elapsed, limit), want, "row {:?}", (previous, current, elapsed, limit));
        }
    }

    /// Admission `wait_ms` after the row's instant, assuming no other request arrives: the counts
    /// shift by one window per 60 s, exactly as `SlidingWindow` does.
    fn admitted_after(previous: u64, current: u64, elapsed: u64, limit: u64, wait_ms: u64) -> bool {
        let t = elapsed + wait_ms;
        match t / RATE_WINDOW_MS {
            0 => admits(previous, current, t, limit),
            1 => admits(current, 0, t - RATE_WINDOW_MS, limit),
            _ => admits(0, 0, t % RATE_WINDOW_MS, limit),
        }
    }

    #[test]
    fn retry_after_is_the_first_whole_second_that_admits() {
        for &(previous, current, elapsed, limit, admitted) in ROWS {
            if admitted || previous == u64::MAX {
                continue;
            }
            let wait = retry_after(previous, current, elapsed, limit);
            assert!(wait >= 1, "Retry-After is at least 1 s");
            assert!(
                admitted_after(previous, current, elapsed, limit, u64::from(wait) * 1000),
                "admitted after {wait} s: {:?}",
                (previous, current, elapsed, limit)
            );
            if wait > 1 {
                assert!(
                    !admitted_after(previous, current, elapsed, limit, u64::from(wait - 1) * 1000),
                    "not admitted one second earlier: {:?}",
                    (previous, current, elapsed, limit)
                );
            }
        }
    }

    #[test]
    fn retry_after_is_one_second_even_for_an_admitted_request() {
        assert_eq!(retry_after(0, 0, 0, 1), 1);
    }

    #[test]
    fn a_limit_of_one_waits_for_the_window_after_next_when_both_windows_are_full() {
        // previous 0, current 1 at 0 ms, limit 1: the next window still weighs the 1 fully at
        // 0 ms and by 1/60000 at its end, so only the window after next admits: 120 s.
        assert_eq!(retry_after(0, 1, 0, 1), 120);
    }

    #[test]
    fn the_first_request_ever_is_admitted_and_counted() {
        let mut w = SlidingWindow::default();
        assert_eq!(w.check(at_ms(T0_MS), nz(1)), Ok(()));
        w.commit(at_ms(T0_MS));
        assert_eq!(
            w.counts(at_ms(T0_MS + 1), nz(1)),
            RateCounts {
                previous: 0,
                current: 1,
                elapsed_ms: 1,
                limit: 1
            }
        );
    }

    #[test]
    fn a_failed_check_does_not_change_the_state() {
        let mut w = SlidingWindow::default();
        w.commit(at_ms(T0_MS));
        let before = w;
        let refused = w.check(at_ms(T0_MS + 10), nz(1)).expect_err("a second request over a limit of 1");
        assert_eq!(
            refused,
            RateCounts {
                previous: 0,
                current: 1,
                elapsed_ms: 10,
                limit: 1
            }
        );
        assert_eq!(w, before, "check is pure (D4)");
    }

    #[test]
    fn the_next_window_reads_the_old_current_count_as_previous() {
        let mut w = SlidingWindow::default();
        for _ in 0..3 {
            w.commit(at_ms(T0_MS + 5_000));
        }
        assert_eq!(
            w.counts(at_ms(T0_MS + 60_000 + 15_000), nz(10)),
            RateCounts {
                previous: 3,
                current: 0,
                elapsed_ms: 15_000,
                limit: 10
            }
        );
        w.commit(at_ms(T0_MS + 60_000 + 15_000));
        assert_eq!(
            w.counts(at_ms(T0_MS + 60_000 + 16_000), nz(10)),
            RateCounts {
                previous: 3,
                current: 1,
                elapsed_ms: 16_000,
                limit: 10
            }
        );
    }

    #[test]
    fn a_gap_of_more_than_two_windows_reads_zero() {
        let mut w = SlidingWindow::default();
        w.commit(at_ms(T0_MS));
        assert_eq!(
            w.counts(at_ms(T0_MS + 2 * 60_000), nz(5)),
            RateCounts {
                previous: 0,
                current: 0,
                elapsed_ms: 0,
                limit: 5
            }
        );
        w.commit(at_ms(T0_MS + 2 * 60_000));
        assert_eq!(
            w.counts(at_ms(T0_MS + 2 * 60_000), nz(5)),
            RateCounts {
                previous: 0,
                current: 1,
                elapsed_ms: 0,
                limit: 5
            }
        );
    }

    #[test]
    fn a_backward_clock_step_clamps_the_elapsed_time_to_zero() {
        let mut w = SlidingWindow::default();
        w.commit(at_ms(T0_MS + 30_000));
        // Five minutes back: an earlier window. Read as the stored window at 0 ms; no panic.
        assert_eq!(
            w.counts(at_ms(T0_MS - 300_000), nz(5)),
            RateCounts {
                previous: 0,
                current: 1,
                elapsed_ms: 0,
                limit: 5
            }
        );
        w.commit(at_ms(T0_MS - 300_000));
        assert_eq!(
            w.counts(at_ms(T0_MS + 30_000), nz(5)),
            RateCounts {
                previous: 0,
                current: 2,
                elapsed_ms: 30_000,
                limit: 5
            }
        );
    }

    #[test]
    fn saturated_counts_never_panic() {
        let mut w = SlidingWindow {
            window: T0_MS / 60_000,
            previous: u64::MAX,
            current: u64::MAX,
        };
        w.commit(at_ms(T0_MS));
        assert!(w.check(at_ms(T0_MS), nz(MAX_REQUESTS_PER_MINUTE)).is_err());
        assert!(admits(u64::MAX, u64::MAX, 0, u64::MAX), "saturated sum equals the saturated limit (controller ruling D1)");
    }

    #[test]
    fn check_agrees_with_the_pure_functions_on_every_row() {
        for &(previous, current, elapsed, limit, want) in ROWS {
            if limit == 0 || previous == u64::MAX {
                continue;
            }
            let w = SlidingWindow {
                window: T0_MS / 60_000,
                previous,
                current,
            };
            let got = w.check(at_ms(T0_MS + elapsed), nz(limit));
            assert_eq!(got.is_ok(), want, "row {:?}", (previous, current, elapsed, limit));
            if let Err(counts) = got {
                assert_eq!(counts.retry_after_secs(), retry_after(previous, current, elapsed, limit));
            }
        }
    }

    // `HashMap` and `Uuid` come from `super::*` (the parent's own imports).

    fn instant(rfc3339: &str) -> SystemTime {
        SystemTime::from(chrono::DateTime::parse_from_rfc3339(rfc3339).expect("a fixture instant"))
    }

    fn unix(rfc3339: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(rfc3339).expect("a fixture instant").timestamp()
    }

    /// (period, instant, label, start, end) — spec § 5.1.
    #[test]
    fn period_keys_and_reset_instants() {
        let rows: &[(BudgetPeriod, &str, &str, &str, &str)] = &[
            (BudgetPeriod::Daily, "2026-10-02T23:59:59.999Z", "2026-10-02", "2026-10-02T00:00:00Z", "2026-10-03T00:00:00Z"),
            (BudgetPeriod::Daily, "2026-10-03T00:00:00Z", "2026-10-03", "2026-10-03T00:00:00Z", "2026-10-04T00:00:00Z"),
            (BudgetPeriod::Daily, "2028-02-29T12:00:00Z", "2028-02-29", "2028-02-29T00:00:00Z", "2028-03-01T00:00:00Z"),
            (BudgetPeriod::Weekly, "2026-10-04T23:59:59Z", "2026-W40", "2026-09-28T00:00:00Z", "2026-10-05T00:00:00Z"),
            (BudgetPeriod::Weekly, "2026-10-05T00:00:00Z", "2026-W41", "2026-10-05T00:00:00Z", "2026-10-12T00:00:00Z"),
            (BudgetPeriod::Weekly, "2026-12-31T12:00:00Z", "2026-W53", "2026-12-28T00:00:00Z", "2027-01-04T00:00:00Z"),
            (BudgetPeriod::Weekly, "2027-01-01T00:00:00Z", "2026-W53", "2026-12-28T00:00:00Z", "2027-01-04T00:00:00Z"),
            (BudgetPeriod::Monthly, "2026-10-31T23:59:59.999Z", "2026-10", "2026-10-01T00:00:00Z", "2026-11-01T00:00:00Z"),
            (BudgetPeriod::Monthly, "2026-12-31T23:59:59Z", "2026-12", "2026-12-01T00:00:00Z", "2027-01-01T00:00:00Z"),
            (BudgetPeriod::Monthly, "2027-01-01T00:00:00Z", "2027-01", "2027-01-01T00:00:00Z", "2027-02-01T00:00:00Z"),
            (BudgetPeriod::Monthly, "2028-02-29T00:00:00Z", "2028-02", "2028-02-01T00:00:00Z", "2028-03-01T00:00:00Z"),
        ];
        for &(period, now, label, start, end) in rows {
            let key = period.key_at(instant(now));
            assert_eq!(key.label(), label, "{period:?} at {now}");
            assert_eq!(key.start().timestamp(), unix(start), "{period:?} at {now}: start");
            assert_eq!(key.end().timestamp(), unix(end), "{period:?} at {now}: end");
            assert_eq!(key.resets_at_unix(), unix(end));
        }
    }

    #[test]
    fn the_default_period_is_monthly() {
        assert_eq!(BudgetPeriod::default(), BudgetPeriod::Monthly);
    }

    #[test]
    fn extreme_instants_never_panic() {
        for period in [BudgetPeriod::Daily, BudgetPeriod::Weekly, BudgetPeriod::Monthly] {
            for secs in [i64::MIN, i64::MIN + 1, -1, 0, i64::MAX - 1, i64::MAX] {
                let _ = period.label_at_reset(secs);
            }
            let _ = period.key_at(UNIX_EPOCH + Duration::from_secs(u64::MAX / 2)).label();
            let _ = period.key_at(UNIX_EPOCH - Duration::from_secs(u64::MAX / 2)).label();
        }
    }

    #[test]
    fn the_refusal_label_is_the_period_one_second_before_the_reset() {
        assert_eq!(BudgetPeriod::Monthly.label_at_reset(unix("2026-11-01T00:00:00Z")), "2026-10");
        assert_eq!(BudgetPeriod::Weekly.label_at_reset(unix("2026-10-05T00:00:00Z")), "2026-W40");
        assert_eq!(BudgetPeriod::Daily.label_at_reset(unix("2026-10-03T00:00:00Z")), "2026-10-02");
    }

    /// Spec § 5.1 `charge_ttl` rows. October 2026 has 31 days = 2_678_400 s.
    #[test]
    fn the_charge_ttl_is_relative_to_now() {
        let october = BudgetPeriod::Monthly.key_at(instant("2026-10-15T00:00:00Z"));
        assert_eq!(BudgetPeriod::charge_ttl(october, instant("2026-10-01T00:00:01Z")), Some(2_678_400 + 86_400 - 1));
        assert_eq!(BudgetPeriod::charge_ttl(october, instant("2026-11-01T00:00:00Z")), Some(86_400));
        assert_eq!(BudgetPeriod::charge_ttl(october, instant("2026-11-01T23:59:59Z")), Some(1));
        assert_eq!(BudgetPeriod::charge_ttl(october, instant("2026-11-02T00:00:00Z")), None);
    }

    const ORG_A: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000a1);
    const ORG_B: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000b2);
    const ORG_C: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000c3);

    fn rules() -> LimitRules {
        let mut overrides = HashMap::new();
        overrides.insert(
            ORG_A,
            OrgOverride {
                org_requests_per_minute: Some(nz(1200)),
                tokens_per_period: None,
                exempt: false,
            },
        );
        overrides.insert(
            ORG_B,
            OrgOverride {
                exempt: true,
                ..OrgOverride::default()
            },
        );
        LimitRules {
            principal_requests_per_minute: Some(nz(60)),
            org_requests_per_minute: Some(nz(600)),
            tokens_per_period: Some(nz(5_000_000)),
            budget_period: BudgetPeriod::Weekly,
            overrides,
        }
    }

    #[test]
    fn policy_for_applies_overrides_exemptions_and_defaults() {
        let budget = Some(Budget {
            tokens: nz(5_000_000),
            period: BudgetPeriod::Weekly,
        });
        // An override that sets only the org rate keeps the default budget.
        assert_eq!(
            rules().policy_for(Some(ORG_A)),
            LimitPolicy {
                principal_requests_per_minute: Some(nz(60)),
                org_requests_per_minute: Some(nz(1200)),
                budget
            }
        );
        // An exempt org keeps only the principal rate (D11).
        assert_eq!(
            rules().policy_for(Some(ORG_B)),
            LimitPolicy {
                principal_requests_per_minute: Some(nz(60)),
                org_requests_per_minute: None,
                budget: None
            }
        );
        // An unlisted org and a request with no org get the table defaults.
        let defaults = LimitPolicy {
            principal_requests_per_minute: Some(nz(60)),
            org_requests_per_minute: Some(nz(600)),
            budget,
        };
        assert_eq!(rules().policy_for(Some(ORG_C)), defaults);
        assert_eq!(rules().policy_for(None), defaults);
    }

    #[test]
    fn is_empty_and_every_policy_is_empty() {
        assert!(LimitPolicy::default().is_empty());
        assert!(!LimitPolicy::default().has_org_dimension());
        let exempt_only = LimitRules {
            overrides: HashMap::from([(
                ORG_B,
                OrgOverride {
                    exempt: true,
                    ..OrgOverride::default()
                },
            )]),
            ..LimitRules::default()
        };
        assert!(exempt_only.policy_for(Some(ORG_B)).is_empty(), "an exempt org with no principal rate gives an empty policy");
        assert!(exempt_only.every_policy_is_empty());
        let one_override = LimitRules {
            overrides: HashMap::from([(
                ORG_A,
                OrgOverride {
                    tokens_per_period: Some(nz(10)),
                    ..OrgOverride::default()
                },
            )]),
            ..LimitRules::default()
        };
        assert!(!one_override.every_policy_is_empty(), "an override with a value can give a non-empty policy");
        assert!(one_override.policy_for(None).is_empty());
        assert!(!rules().every_policy_is_empty());
    }

    fn rate(previous: u64, current: u64, elapsed_ms: u64, limit: u64) -> RateCounts {
        RateCounts { previous, current, elapsed_ms, limit }
    }

    /// D8 precedence: budget, then principal rate, then org rate; Retry-After = max, at least 1.
    #[test]
    fn the_refusal_follows_the_d8_precedence() {
        let budget = FailedCheck::Budget {
            period: BudgetPeriod::Monthly,
            resets_at_unix: unix("2026-11-01T00:00:00Z"),
        };
        let principal = FailedCheck::PrincipalRate(rate(0, 2, 59_999, 2)); // 31 s
        let org = FailedCheck::OrgRate(rate(10, 5, 30_000, 10)); // 6 s
        assert_eq!(
            choose_refusal(&[org, principal, budget]),
            Some(LimitRefusal::BudgetExhausted {
                period: BudgetPeriod::Monthly,
                resets_at_unix: unix("2026-11-01T00:00:00Z")
            })
        );
        assert_eq!(
            choose_refusal(&[org, principal]),
            Some(LimitRefusal::RateLimited {
                retry_after_secs: 31,
                reason: RefusalReason::PrincipalRate
            })
        );
        assert_eq!(
            choose_refusal(&[org]),
            Some(LimitRefusal::RateLimited {
                retry_after_secs: 6,
                reason: RefusalReason::OrgRate
            })
        );
        assert_eq!(choose_refusal(&[]), None);
        // The minimum is 1 s even when the counts would admit at once.
        assert_eq!(
            choose_refusal(&[FailedCheck::OrgRate(rate(0, 0, 0, 1))]),
            Some(LimitRefusal::RateLimited {
                retry_after_secs: 1,
                reason: RefusalReason::OrgRate
            })
        );
    }

    #[test]
    fn labels_are_the_bounded_metric_values() {
        assert_eq!(RefusalReason::ALL.map(RefusalReason::as_label), ["principal_rate", "org_rate", "org_budget"]);
        assert_eq!(UnavailableKind::ALL.map(UnavailableKind::as_label), ["io", "server", "decode"]);
        assert_eq!(ChargeDropReason::ALL.map(ChargeDropReason::as_label), ["no_runtime", "shutdown", "period_expired"]);
        assert_eq!(LimitRefusal::Unscoped.reason(), None);
        assert_eq!(LimitRefusal::Unscoped.label(), "unscoped");
        assert_eq!(
            budget_refusal(
                Budget {
                    tokens: nz(1),
                    period: BudgetPeriod::Daily
                },
                BudgetPeriod::Daily.key_at(instant("2026-10-02T12:00:00Z"))
            ),
            FailedCheck::Budget {
                period: BudgetPeriod::Daily,
                resets_at_unix: unix("2026-10-03T00:00:00Z")
            }
        );
    }

    #[test]
    fn the_period_serde_spelling_is_lowercase() {
        assert_eq!(serde_json::to_string(&BudgetPeriod::Weekly).expect("serializes"), "\"weekly\"");
        assert_eq!(serde_json::from_str::<BudgetPeriod>("\"daily\"").expect("deserializes"), BudgetPeriod::Daily);
        assert!(serde_json::from_str::<BudgetPeriod>("\"yearly\"").is_err());
    }
}
