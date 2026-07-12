// SPDX-License-Identifier: Apache-2.0

//! Authorization cache/adapter composition (ADR-0013, spec §7): the two generation
//! counters (Task 10), the compiled-policy snapshot (Task 13), the generation-keyed
//! decision + entity-slice caches (Task 14), and the `CedarAuthorizer` + `TracingAuditSink`
//! composition wiring all of these together into the `Authorizer`/`AuditSink` ports (Task
//! 15). `CedarAuthorizer` is wired into `AppState`/`main.rs` (SMA-446 Slice A).

pub mod audit;
pub mod cedar_authorizer;
pub mod decision_cache;
pub mod denial_audit;
pub mod entity_cache;
pub mod generation;
pub mod policy_snapshot;

pub use audit::{FanOutAuditSink, TracingAuditSink};
pub use cedar_authorizer::{CedarAuthorizer, GenerationsReader};
pub use decision_cache::{MemoryDecisionCache, RedisDecisionCache, decision_key};
pub use denial_audit::{BufferedDenialAuditSink, DenialAuditBuffer, DenialAuditDrain};
pub use entity_cache::SliceCache;
pub use generation::Generations;
pub use policy_snapshot::PolicySnapshot;
