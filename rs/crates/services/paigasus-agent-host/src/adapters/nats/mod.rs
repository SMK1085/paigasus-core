// SPDX-License-Identifier: Apache-2.0

//! The NATS source (spec §9): the channels and the tenancy rule.

pub mod channels;
pub mod tenancy;

/// The inbox prefix of the `agent-host-consumer` identity. `ops/nats/subjects.env` holds the same value.
pub const INBOX_PREFIX: &str = "_INBOX_AGH";

/// The name of the durable consumer. `ops/nats/subjects.env` holds the same value.
pub const DURABLE_NAME: &str = "agent-host-inbox";

#[cfg(test)]
mod tests {
    use super::*;

    const SUBJECTS_ENV: &str = include_str!("../../../../../../../ops/nats/subjects.env");

    #[test]
    fn the_rust_constants_match_subjects_env() {
        assert!(SUBJECTS_ENV.contains(&format!("AGENT_HOST_INBOX_PREFIX=\"{INBOX_PREFIX}\"")));
        assert!(SUBJECTS_ENV.contains(&format!("AGENT_HOST_DURABLE_NAME=\"{DURABLE_NAME}\"")));
    }
}
