// SPDX-License-Identifier: Apache-2.0

//! SMA-677: the pure rules of the gateway's rate limit and token budget, and the store port.
//!
//! No dependency on `redis`, `tokio`, `metrics` or the config types (spec § 4.2). Both store
//! adapters compute every decision with the functions in this file, so they agree to the request
//! (D3). No function here reads a clock: every instant is a parameter.

use std::num::NonZeroU64;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
}
