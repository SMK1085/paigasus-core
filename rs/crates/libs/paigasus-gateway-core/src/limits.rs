// SPDX-License-Identifier: Apache-2.0
//! The interceptor's limits (spec D8, D13–D16). The gateway builds them from `[tool_interception]`.

use std::time::Duration;

/// What a failure does (D16): block every affected call (the default), or let it through and
/// report it (observe mode, § 5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnFailure {
    Block,
    Allow,
}

/// The bounds of one stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// D8: a record over this size is never skipped; it is a stream-level failure.
    pub max_record_bytes: usize,
    /// D13: the per-stream total of queued payload bytes.
    pub max_held_bytes: usize,
    /// D14: from a hold's first frame to its completion.
    pub max_hold: Duration,
    /// D14: how long the decider may take for one hold.
    pub decide_timeout: Duration,
    /// D15: the keep-alive interval while anything is held.
    pub keepalive: Duration,
    pub on_failure: OnFailure,
}

impl Limits {
    /// The spec's defaults (§ 5.2). The gateway config uses the same numbers.
    #[must_use]
    pub fn defaults() -> Self {
        Limits {
            max_record_bytes: 1_048_576,
            max_held_bytes: 4 * 1_048_576,
            max_hold: Duration::from_secs(300),
            decide_timeout: Duration::from_secs(5),
            keepalive: Duration::from_secs(10),
            on_failure: OnFailure::Block,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_match_the_spec() {
        let l = Limits::defaults();
        assert_eq!(l.max_record_bytes, 1 << 20);
        assert_eq!(l.max_held_bytes, 4 << 20);
        assert_eq!(l.max_hold, Duration::from_secs(300));
        assert_eq!(l.decide_timeout, Duration::from_secs(5));
        assert_eq!(l.keepalive, Duration::from_secs(10));
        assert_eq!(l.on_failure, OnFailure::Block);
    }
}
