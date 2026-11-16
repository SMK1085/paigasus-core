// SPDX-License-Identifier: Apache-2.0

//! NATS adapters. `policy_events` is the core subscription on `iam.policy.>` that invalidates the
//! tool-policy cache (SMA-752 § 6.3.5, Q1-A).

pub mod policy_events;

pub use policy_events::{INBOX_PREFIX, NatsError, POLICY_SUBJECT, PolicyEvents, connect};
