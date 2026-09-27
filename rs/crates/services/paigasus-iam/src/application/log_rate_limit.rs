// SPDX-License-Identifier: Apache-2.0

//! One rate-limit policy for diagnostic log lines (SMA-686 D14, SMA-698 D2).

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// At most one log line per (issuer, kind) in this interval (SMA-686 D14).
pub(crate) const LOG_RATE_LIMIT_INTERVAL: Duration = Duration::from_secs(10);

/// Rate limit for diagnostic log lines (SMA-686 D14). One signed token can otherwise write one
/// line per request. Keyed by (issuer, kind), so the map holds at most `issuers × kinds` entries.
/// The validator uses it with `TokenDefect`, and `AuthenticateToken` with the JIT `defect` label
/// (SMA-698 D2). A suppressed event is counted, and the next admitted line reports the count.
pub(crate) struct LogRateLimiter<K> {
    interval: Duration,
    last: Mutex<HashMap<(String, K), (Instant, u64)>>,
}

impl<K: Copy + Eq + Hash> LogRateLimiter<K> {
    pub(crate) fn new(interval: Duration) -> Self {
        LogRateLimiter {
            interval,
            last: Mutex::new(HashMap::new()),
        }
    }

    /// `Some(suppressed)` when a line may be written at `now` (with the count of events
    /// suppressed since the last line); `None` when this event is suppressed and counted.
    pub(crate) fn admit_at(&self, issuer: &str, kind: K, now: Instant) -> Option<u64> {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        match last.get_mut(&(issuer.to_owned(), kind)) {
            Some((at, suppressed)) if now.saturating_duration_since(*at) < self.interval => {
                *suppressed += 1;
                None
            }
            Some((at, suppressed)) => {
                let count = *suppressed;
                *at = now;
                *suppressed = 0;
                Some(count)
            }
            None => {
                last.insert((issuer.to_owned(), kind), (now, 0));
                Some(0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_iam_core::TokenDefect;

    #[test]
    fn refusal_log_counts_suppressed_refusals() {
        // D14 unit: admit, suppress twice within the interval, then admit with the count.
        let log = LogRateLimiter::new(Duration::from_secs(10));
        let t0 = Instant::now();
        assert_eq!(log.admit_at("iss", TokenDefect::NotAnAccessToken, t0), Some(0));
        assert_eq!(log.admit_at("iss", TokenDefect::NotAnAccessToken, t0 + Duration::from_secs(1)), None);
        assert_eq!(log.admit_at("iss", TokenDefect::NotAnAccessToken, t0 + Duration::from_secs(2)), None);
        // Independent keys: another defect and another issuer are admitted at once.
        assert_eq!(log.admit_at("iss", TokenDefect::AudienceMismatch, t0 + Duration::from_secs(2)), Some(0));
        assert_eq!(log.admit_at("other", TokenDefect::NotAnAccessToken, t0 + Duration::from_secs(2)), Some(0));
        assert_eq!(log.admit_at("iss", TokenDefect::NotAnAccessToken, t0 + Duration::from_secs(11)), Some(2));
        assert_eq!(log.admit_at("iss", TokenDefect::NotAnAccessToken, t0 + Duration::from_secs(12)), None);
    }
}
