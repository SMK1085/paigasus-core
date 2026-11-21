// SPDX-License-Identifier: Apache-2.0

//! Conversion between `jiff::Timestamp` and the microseconds that Postgres stores.
//! `timestamptz` has a precision of one microsecond.

use jiff::Timestamp;
use paigasus_agent_host_core::ports::StoreError;

/// The microseconds since the Unix epoch. An instant before the epoch gives a negative number.
#[must_use]
pub fn to_micros(t: Timestamp) -> i64 {
    t.as_microsecond()
}

/// The timestamp for a count of microseconds since the Unix epoch.
pub fn from_micros(us: i64) -> Result<Timestamp, StoreError> {
    Timestamp::from_microsecond(us).map_err(|e| StoreError(format!("timestamp out of range: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_round_trips_through_microseconds() {
        let t = Timestamp::from_microsecond(1_700_000_000_123_456).expect("in range");
        assert_eq!(from_micros(to_micros(t)).expect("in range"), t);
    }

    #[test]
    fn an_instant_before_the_epoch_round_trips() {
        let t = Timestamp::from_microsecond(-5).expect("in range");
        assert_eq!(to_micros(t), -5);
        assert_eq!(from_micros(-5).expect("in range"), t);
    }

    #[test]
    fn a_value_out_of_range_is_a_store_error() {
        assert!(from_micros(i64::MAX).is_err());
    }
}
