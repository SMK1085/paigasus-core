// SPDX-License-Identifier: Apache-2.0

//! Content-keyed [`DecisionCache`] (spec §7/D12; SMA-470 D4): caches a previously-computed
//! [`Decision`] under a key that folds in the compiled policy set's content hash, the entity
//! generation (Task 10), AND the full request (principal/action/resource/context), so a
//! cached decision can only ever be served back for the exact same question asked against
//! the exact same policy/entity state — any change to any of those six inputs
//! (policy_content, entity_gen, principal, action, resource, context) mints a different key,
//! never a stale hit. The policy component is deliberately the compiled set's content hash
//! rather than the `policy_gen` counter: that counter is Redis-sourced and can move
//! NON-monotonically (a swallowed bump, a reset-to-0 key), which would let a key space that
//! was live before a revoke be re-entered and replay a pre-revoke `Allow`. See
//! [`decision_key`]'s doc for the full rationale.
//!
//! Two implementations: [`MemoryDecisionCache`] (single-replica, process lifetime only —
//! same posture as `Generations::memory`) and [`RedisDecisionCache`] (cross-replica,
//! `ConnectionManager`, mirroring `adapters::oidc::redis_cache::RedisJwksCache`'s
//! connect/clone-per-call pattern). **Both fail OPEN (D12):** a decision cache is a pure
//! accelerator over `PolicySnapshot`/Cedar — never the system of record — so a `get` that
//! can't be served cleanly (a Redis error, or a payload that fails to deserialize) is
//! reported as a plain cache miss, `None`, never an error; a `put` that can't be written is
//! logged and swallowed, never surfaced to the caller. A Redis outage bypasses the
//! accelerator; it must never fail a decision.

use async_trait::async_trait;
use paigasus_iam_core::{AccessRequest, AuthzError, Decision, DecisionCache};
use redis::AsyncCommands;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::adapters::redis_conn::{RedisHandle, RedisRole};

/// Redis/in-proc key prefix (spec §7): `iam:authz:dec:<policy_content>:<entity_gen>:<hash>`.
const KEY_PREFIX: &str = "iam:authz:dec:";

/// The cache key for one [`AccessRequest`] decided against a given compiled-policy content
/// hash and `entity_gen`: `iam:authz:dec:<policy_content>:<entity_gen>:<blake3 hex digest>`.
///
/// The policy component is
/// [`CompiledPolicies::content_hash`](paigasus_iam_core::authz::engine::CompiledPolicies::content_hash),
/// NOT the `policy_gen` counter
/// (SMA-470 D4). The counter is Redis-sourced: it can stall behind a swallowed bump, reset to
/// 0 when Redis loses its data, and therefore move NON-monotonically — which would let a key
/// space that was live earlier be re-entered, replaying a pre-revoke `Allow` from before the
/// change. A content hash cannot: it is a pure function of the compiled policy set, identical
/// across replicas that compiled the same set (so the cache stays shared fleet-wide) and
/// always different when the set differs.
///
/// **Scope caveat:** that content hash covers the STORED inputs — the policy/template
/// documents and the role-grant rows — and nothing else. The Cedar `schema()` and the
/// `Action`-to-Cedar-UID mapping are compile-time constants outside it, so a release that
/// changes evaluation semantics WITHOUT touching a stored policy or grant hashes identically
/// on old and new replicas, and the two share decision-cache keys across a rolling deploy.
/// Not a regression (the `policy_gen` counter this replaced was equally content-independent),
/// but a semantics-changing deploy should flush `iam:authz:dec:*` or accept up to
/// `authz.decision_cache_ttl_secs` of mixed-semantics hits — see the RUNBOOK's "Authz
/// availability posture".
///
/// The digest is over a canonical (deterministic) serialization of `(principal.canonical(),
/// action.as_wire(), resource.canonical(), context)` — a `serde_json` encoding of that
/// tuple, which is deterministic here because
/// [`RequestContext`](paigasus_iam_core::authz::model::RequestContext) wraps a `BTreeMap`
/// (always iterated in sorted key order, never hashmap-random order).
///
/// The key changes if ANY of the six inputs changes: `policy_content`/`entity_gen` are
/// folded in verbatim (not hashed) so a content or generation change always mints a disjoint
/// key space, and the request's four fields all feed the digest.
#[must_use]
pub fn decision_key(policy_content: &str, entity_gen: u64, req: &AccessRequest) -> String {
    let canonical = (req.principal.canonical(), req.action.as_wire(), req.resource.canonical(), &req.context);
    // `RequestContext` derives `Serialize` and every field feeding this tuple does too —
    // encoding a value built entirely from those cannot fail.
    let bytes = serde_json::to_vec(&canonical).expect("decision_key's canonical tuple is always serializable");
    let digest = blake3::hash(&bytes);
    format!("{KEY_PREFIX}{policy_content}:{entity_gen}:{}", digest.to_hex())
}

/// In-process `DecisionCache`: a plain `Mutex<HashMap<..>>` — single-replica, process
/// lifetime only (same posture as `Generations::memory`). No eviction/TTL (M3 scope): a
/// deployment that needs bounded memory or cross-replica sharing reaches for
/// [`RedisDecisionCache`] instead.
#[derive(Default)]
pub struct MemoryDecisionCache {
    entries: Mutex<HashMap<String, Decision>>,
}

impl MemoryDecisionCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl DecisionCache for MemoryDecisionCache {
    async fn get(&self, key: &str) -> Option<Decision> {
        self.entries.lock().unwrap().get(key).cloned()
    }

    async fn put(&self, key: &str, decision: &Decision) {
        self.entries.lock().unwrap().insert(key.to_string(), decision.clone());
    }
}

/// `DecisionCache` backed by Redis via an auto-reconnecting `ConnectionManager` (spec §7),
/// mirroring `adapters::oidc::redis_cache::RedisJwksCache`. Cheap to clone the connection
/// per call — `ConnectionManager` is itself `Arc`-backed and designed for concurrent
/// callers.
///
/// **Fail-open (D12):** unlike `RedisJwksCache` (which fails CLOSED on a Redis error — a
/// stale/unreachable JWKS cache is an auth-availability concern), this cache fails OPEN:
/// every error path (connect, I/O, or (de)serialize) on `get` returns `None` — a plain
/// miss, indistinguishable from "never cached" — and every error on `put` is logged and
/// swallowed. The caller (`CedarAuthorizer`, a later task) always falls through to
/// `PolicySnapshot`/Cedar on a miss, so a Redis outage only costs the accelerator, never a
/// decision.
pub struct RedisDecisionCache {
    conn: RedisHandle,
    ttl_secs: u64,
}

impl RedisDecisionCache {
    /// Opens `redis_url` and wraps it in a `ConnectionManager`. `ttl_secs` is applied to
    /// every `put` as Redis's own `EX` expiry — this cache is a fail-open accelerator, so an
    /// entry disappearing after `ttl_secs` (or on eviction) never surfaces as anything
    /// other than a subsequent miss.
    pub async fn connect(redis_url: &str, ttl_secs: u64) -> Result<Self, AuthzError> {
        let conn = crate::adapters::redis_conn::connect(redis_url, RedisRole::Authz).await.map_err(redis_connect_err)?;
        Ok(Self { conn, ttl_secs })
    }

    /// Builds a cache over an ALREADY-CONNECTED `ConnectionManager` (SMA-444 Task 21):
    /// `AppState::new` shares ONE redis connection across the redis-backed `Generations` +
    /// `RedisDecisionCache` + `SliceCache` rather than each opening its own — `connect` above
    /// stays the standalone-caller/test entry point.
    pub(crate) fn from_connection(conn: RedisHandle, ttl_secs: u64) -> Self {
        Self { conn, ttl_secs }
    }
}

#[async_trait]
impl DecisionCache for RedisDecisionCache {
    async fn get(&self, key: &str) -> Option<Decision> {
        let mut conn = self.conn.clone();
        let raw: Result<Option<Vec<u8>>, redis::RedisError> = conn.get(key).await;
        match raw {
            Ok(Some(bytes)) => match serde_json::from_slice::<Decision>(&bytes) {
                Ok(decision) => Some(decision),
                Err(_) => {
                    log_deserialize_miss();
                    None
                }
            },
            Ok(None) => None,
            Err(err) => {
                log_get_miss(err.kind());
                None
            }
        }
    }

    async fn put(&self, key: &str, decision: &Decision) {
        let payload = match serde_json::to_vec(decision) {
            Ok(payload) => payload,
            Err(_) => {
                log_serialize_swallow();
                return;
            }
        };
        let mut conn = self.conn.clone();
        let result: Result<(), redis::RedisError> = conn.set_ex(key, payload, self.ttl_secs).await;
        if let Err(err) = result {
            log_put_swallow(err.kind());
        }
    }
}

fn redis_connect_err(e: redis::RedisError) -> AuthzError {
    AuthzError::Backend(Box::new(e))
}

/// Logs the Redis error's `ErrorKind` only — never `Display`/message, which can echo
/// connection details (same posture as `oidc::redis_cache::log_unavailable`) — then the
/// fail-open mapping: a get error degrades to a plain miss (D12).
fn log_get_miss(kind: redis::ErrorKind) {
    tracing::warn!(error_kind = ?kind, "redis decision cache get error — treating as a miss (fail-open, D12)");
}

fn log_deserialize_miss() {
    tracing::warn!(error_kind = "serde_json", "redis decision cache deserialize error — treating as a miss (fail-open, D12)");
}

fn log_put_swallow(kind: redis::ErrorKind) {
    tracing::warn!(error_kind = ?kind, "redis decision cache put error — swallowed (fail-open, D12)");
}

fn log_serialize_swallow() {
    tracing::warn!(error_kind = "serde_json", "redis decision cache serialize error — swallowed (fail-open, D12)");
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_iam_core::authz::model::ContextValue;
    use paigasus_iam_core::{Action, Effect, RequestContext};
    use paigasus_kernel::Prn;
    use uuid::Uuid;

    fn prn(resource_type: &str, n: u128) -> Prn {
        Prn::build("iam", "", None, resource_type, Uuid::from_u128(n)).expect("static test prn parts are valid")
    }

    fn base_request() -> AccessRequest {
        AccessRequest {
            principal: prn("principal", 1),
            action: Action::GetProject,
            resource: prn("project", 2),
            context: RequestContext::empty(),
        }
    }

    fn sample_decision() -> Decision {
        Decision {
            effect: Effect::Allow,
            determining_policies: vec!["policy-1".to_string()],
        }
    }

    /// Only `decision_key`'s own determinism. The CROSS-REPLICA property this underpins — that
    /// two replicas which compiled the same policy set derive the same `policy_content` string in
    /// the first place, and so share one key space — lives where that string is produced, not
    /// here: `authz::engine`'s `content_hash_is_stable_for_identical_inputs` and
    /// `content_hash_ignores_input_ordering` (paigasus-iam-core).
    #[test]
    fn decision_key_is_stable_for_identical_inputs() {
        let req = base_request();
        assert_eq!(decision_key("content-a", 2, &req), decision_key("content-a", 2, &req));
    }

    /// SMA-470 D4: the key's policy component is the compiled set's content hash, so any
    /// policy/grant change mints a disjoint key space — even when the generation counter did
    /// not move (a swallowed bump, a reset counter).
    #[test]
    fn decision_key_changes_when_policy_content_changes() {
        let req = base_request();
        assert_ne!(decision_key("content-a", 2, &req), decision_key("content-b", 2, &req));
    }

    #[test]
    fn decision_key_changes_when_entity_gen_changes() {
        let req = base_request();
        assert_ne!(decision_key("content-a", 2, &req), decision_key("content-a", 9, &req));
    }

    #[test]
    fn decision_key_changes_when_principal_changes() {
        let mut other = base_request();
        other.principal = prn("principal", 99);
        assert_ne!(decision_key("content-a", 2, &base_request()), decision_key("content-a", 2, &other));
    }

    #[test]
    fn decision_key_changes_when_action_changes() {
        let mut other = base_request();
        other.action = Action::ListProjects;
        assert_ne!(decision_key("content-a", 2, &base_request()), decision_key("content-a", 2, &other));
    }

    #[test]
    fn decision_key_changes_when_resource_changes() {
        let mut other = base_request();
        other.resource = prn("project", 99);
        assert_ne!(decision_key("content-a", 2, &base_request()), decision_key("content-a", 2, &other));
    }

    #[test]
    fn decision_key_changes_when_context_changes() {
        let mut other = base_request();
        other.context.0.insert("ip".to_string(), ContextValue::Str("10.0.0.1".to_string()));
        assert_ne!(decision_key("content-a", 2, &base_request()), decision_key("content-a", 2, &other));

        let mut different_value = base_request();
        different_value.context.0.insert("ip".to_string(), ContextValue::Str("10.0.0.2".to_string()));
        assert_ne!(decision_key("content-a", 2, &other), decision_key("content-a", 2, &different_value));
    }

    #[tokio::test]
    async fn memory_cache_get_put_round_trips() {
        let cache = MemoryDecisionCache::new();
        let key = decision_key("content-a", 2, &base_request());
        let decision = sample_decision();

        assert!(cache.get(&key).await.is_none(), "nothing cached yet");
        cache.put(&key, &decision).await;
        assert_eq!(cache.get(&key).await, Some(decision));
    }

    #[tokio::test]
    async fn memory_cache_get_of_missing_key_is_none() {
        let cache = MemoryDecisionCache::new();
        assert!(cache.get("iam:authz:dec:never:cached:x").await.is_none());
    }
}
