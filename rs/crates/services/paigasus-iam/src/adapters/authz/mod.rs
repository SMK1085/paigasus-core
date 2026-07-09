// SPDX-License-Identifier: Apache-2.0

//! Authorization cache/adapter composition (ADR-0013, spec §7): the two generation
//! counters (Task 10), the compiled-policy snapshot (Task 13), and the generation-keyed
//! decision + entity-slice caches (Task 14). A later M3 task adds the `CedarAuthorizer`
//! composition wiring all of these together.

pub mod decision_cache;
pub mod entity_cache;
pub mod generation;
pub mod policy_snapshot;

pub use decision_cache::{MemoryDecisionCache, RedisDecisionCache, decision_key};
pub use entity_cache::SliceCache;
pub use generation::Generations;
pub use policy_snapshot::PolicySnapshot;
