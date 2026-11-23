// SPDX-License-Identifier: Apache-2.0

//! The retry delay: `min(base * 2^(n-1) * jitter[0.8, 1.2], max)` with saturating math (spec 6.9).

use std::time::Duration;

/// The base and the cap of the retry delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub base: Duration,
    pub max: Duration,
}

/// A jitter factor. The value is always in `[0.8, 1.2]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jitter(f64);

impl Jitter {
    /// Clamp `value` to `[0.8, 1.2]`. NaN gives 1.0.
    pub fn new(value: f64) -> Jitter {
        if value.is_nan() {
            return Jitter(1.0);
        }
        Jitter(value.clamp(0.8, 1.2))
    }
}

impl RetryPolicy {
    /// The delay before retry number `attempts`. The jitter applies before the cap.
    pub fn delay(&self, attempts: u32, jitter: Jitter, retry_after: Option<Duration>) -> Duration {
        let n = attempts.max(1);
        // Double the duration, not a `u32` factor: a factor that saturates at `u32::MAX` can stay
        // below the cap for a small base. Once `raw` reaches `2 * max`, a jitter of at least 0.8
        // still gives the cap, so the loop stops there. It also stops on a zero base.
        let limit = self.max.saturating_mul(2);
        let mut raw = self.base;
        for _ in 1..n {
            if raw.is_zero() || raw >= limit {
                break;
            }
            raw = raw.saturating_mul(2);
        }
        let jittered = Duration::try_from_secs_f64(raw.as_secs_f64() * jitter.0).unwrap_or(self.max);
        let delay = jittered.min(self.max);
        match retry_after {
            Some(after) => delay.max(after).min(self.max),
            None => delay,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn p() -> RetryPolicy {
        RetryPolicy {
            base: Duration::from_secs(30),
            max: Duration::from_secs(1800),
        }
    }
    fn one() -> Jitter {
        Jitter::new(1.0)
    }

    #[test]
    fn the_first_attempt_waits_the_base() {
        assert_eq!(p().delay(1, one(), None), Duration::from_secs(30));
    }
    #[test]
    fn the_delay_doubles() {
        assert_eq!(p().delay(3, one(), None), Duration::from_secs(120));
    }
    #[test]
    fn the_cap_holds() {
        assert_eq!(p().delay(7, one(), None), Duration::from_secs(1800));
    }
    #[test]
    fn the_jitter_comes_before_the_cap() {
        assert_eq!(p().delay(6, Jitter::new(1.2), None), Duration::from_secs(1152));
        assert_eq!(p().delay(7, Jitter::new(1.2), None), Duration::from_secs(1800));
    }
    #[test]
    fn retry_after_raises_the_delay_and_stays_capped() {
        assert_eq!(p().delay(1, one(), Some(Duration::from_secs(600))), Duration::from_secs(600));
        assert_eq!(p().delay(1, one(), Some(Duration::from_secs(99_999))), Duration::from_secs(1800));
    }
    #[test]
    fn attempt_100_does_not_overflow() {
        assert_eq!(p().delay(100, Jitter::new(1.2), None), Duration::from_secs(1800));
    }
    #[test]
    fn a_small_base_still_reaches_the_cap() {
        let tiny = RetryPolicy {
            base: Duration::from_nanos(1),
            max: Duration::from_secs(30),
        };
        assert_eq!(tiny.delay(36, one(), None), Duration::from_secs(30));
        assert_eq!(tiny.delay(u32::MAX, Jitter::new(0.8), None), Duration::from_secs(30));
        assert_eq!(tiny.delay(3, one(), None), Duration::from_nanos(4));
    }
    #[test]
    fn a_zero_base_gives_zero() {
        let zero = RetryPolicy {
            base: Duration::ZERO,
            max: Duration::from_secs(30),
        };
        assert_eq!(zero.delay(u32::MAX, Jitter::new(1.2), None), Duration::ZERO);
    }
    #[test]
    fn the_jitter_is_clamped() {
        assert_eq!(p().delay(1, Jitter::new(5.0), None), Duration::from_secs(36));
        assert_eq!(p().delay(1, Jitter::new(f64::NAN), None), Duration::from_secs(30));
    }
    #[test]
    fn four_retries_cover_seven_and_a_half_minutes() {
        let total: Duration = (1..=4).map(|n| p().delay(n, one(), None)).sum();
        assert_eq!(total, Duration::from_secs(450));
    }
}
