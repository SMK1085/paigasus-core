// SPDX-License-Identifier: Apache-2.0

//! Authorization cache/adapter composition (ADR-0013, spec §7): the two generation
//! counters (Task 10). Later M3 tasks add the policy snapshot, entity-slice cache,
//! decision cache, and the `CedarAuthorizer` composition to this module.

pub mod generation;

pub use generation::Generations;
