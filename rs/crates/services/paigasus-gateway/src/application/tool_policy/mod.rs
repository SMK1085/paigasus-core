// SPDX-License-Identifier: Apache-2.0

//! SMA-752 § 6.3 and § 6.5: the tool-policy use cases of the gateway. `cache` holds the per-tenant
//! policy cache, `request` the request side (snapshot, plan, refusal, rewrite, report) and
//! `decider` the Cedar decider behind the `ToolCallDecider` port, and `slots` the history slots
//! of a hold (SMA-753 spec P7).

pub mod cache;
pub mod decider;
pub mod request;
pub mod slots;
#[cfg(test)]
pub(crate) mod testing;

pub use cache::{CacheSettings, CompiledTenant, FloorEntry, Invalidate, Lookup, PolicyCache};
pub use decider::{CedarDecider, DETECTOR_LABELS, FINDING_KINDS, FINDING_TRIPLES, finding_label, finding_labels, prime_tool_policy_metrics};
pub use request::{OverheadSample, Refusal, Refused, RequestOutcome, RequestToolPolicy};
pub use slots::{HoldSlots, StoreOutcome, hold_slots};
