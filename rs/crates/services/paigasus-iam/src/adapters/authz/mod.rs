// SPDX-License-Identifier: Apache-2.0

//! Authorization cache/adapter composition (ADR-0013, spec §7): the two generation
//! counters (Task 10) and the compiled-policy snapshot (Task 13). Later M3 tasks add the
//! entity-slice cache, decision cache, and the `CedarAuthorizer` composition to this module.

pub mod generation;
pub mod policy_snapshot;

pub use generation::Generations;
pub use policy_snapshot::PolicySnapshot;
