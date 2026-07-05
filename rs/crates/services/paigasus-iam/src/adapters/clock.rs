// SPDX-License-Identifier: Apache-2.0

//! `SystemClock` — wall-clock time truncated to microseconds so timestamps round-trip
//! through Postgres `TIMESTAMPTZ` (µs resolution) without truncation-on-store mismatch.

// Nothing in `main.rs` wires this into a use case yet — the composition root lands in
// Task 11. Until then it's exercised only via the `#[cfg(test)]` test below; same
// reasoning as `application::create_user` (Task 6).
#![allow(dead_code)]

use chrono::{DateTime, SubsecRound, Utc};
use paigasus_iam_core::Clock;

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now().trunc_subsecs(6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_has_no_sub_microsecond_digits() {
        let t = SystemClock.now();
        assert_eq!(t.timestamp_subsec_nanos() % 1_000, 0);
    }
}
