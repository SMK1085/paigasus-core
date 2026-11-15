// SPDX-License-Identifier: Apache-2.0

//! SMA-752 § 6.3: the tool-policy use cases of the gateway. `cache` holds the per-tenant policy
//! cache. The decider (Task 14) joins it here.

pub mod cache;

pub use cache::{CacheSettings, FloorEntry, Invalidate, Lookup, PolicyCache};
