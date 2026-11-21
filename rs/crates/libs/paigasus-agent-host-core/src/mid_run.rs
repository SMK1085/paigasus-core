// SPDX-License-Identifier: Apache-2.0

//! The policy for an event that arrives while a run is active.

use serde::{Deserialize, Serialize};

/// What the host does with an event that arrives during a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidRunPolicy {
    /// Keep the event for the next run.
    Queue,
    /// Fold the event into the active run.
    Fold,
    /// Interrupt the active run.
    Interrupt,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mid_run_policy_reads_snake_case() {
        assert_eq!(serde_json::from_str::<MidRunPolicy>("\"queue\"").ok(), Some(MidRunPolicy::Queue));
        assert_eq!(serde_json::from_str::<MidRunPolicy>("\"fold\"").ok(), Some(MidRunPolicy::Fold));
        assert_eq!(serde_json::from_str::<MidRunPolicy>("\"interrupt\"").ok(), Some(MidRunPolicy::Interrupt));
    }
}
