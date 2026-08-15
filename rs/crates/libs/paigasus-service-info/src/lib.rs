// SPDX-License-Identifier: Apache-2.0

//! The `ServiceInfo` descriptor both Paigasus services serve (ADR-0020, SMA-505).
//!
//! This crate exists to hold the wire invariants in ONE tested place rather than in each
//! service: the `CAPABILITY_UNSPECIFIED` sentinel is never advertised, the list is
//! deterministic and de-duplicated, and `capabilities` is always emitted — as `[]` when empty.
//! That last one is not cosmetic: canonical protojson omits an empty repeated field, and a
//! console doing `info.capabilities.includes(k)` against a missing key throws `TypeError`
//! instead of rendering "feature off" (SMA-499 § 2.7).
//!
//! Each service owns its own config -> `Vec<Capability>` projection; nothing about that is
//! shared, because the two services read entirely different config types.

use paigasus_proto::paigasus::common::v1::{Capability, ServiceInfo};
use serde::Serialize;

/// The HTTP route both services serve the descriptor on, so the path literal cannot drift
/// between them. Specified normatively in `common/v1/service_info.proto`'s file comment.
pub const ROUTE: &str = "/v1/service-info";

/// Build the descriptor from the capabilities a service currently has ENABLED.
///
/// `version` is a parameter rather than this crate's own `CARGO_PKG_VERSION`: each SERVICE
/// must report its own build, and taking it as an argument is what lets the test above prove
/// the value flows through untouched (AC 4 — see that test's doc for why the obvious
/// service-side assertion is vacuous today).
///
/// The `UNSPECIFIED` sentinel is dropped, duplicates are removed, and the result is ordered by
/// enum discriminant. Ordering is an implementation detail for stable output, NOT a contract —
/// the proto states the list is unordered and that clients must build a set from it.
pub fn descriptor(service: &str, version: &str, capabilities: &[Capability]) -> ServiceInfo {
    let mut caps: Vec<Capability> = capabilities.iter().copied().filter(|c| *c != Capability::Unspecified).collect();
    caps.sort_by_key(|c| *c as i32);
    caps.dedup();
    ServiceInfo {
        service: service.to_owned(),
        version: version.to_owned(),
        // `as_wire_key` returns `None` only for the sentinel, already filtered above — so this
        // never silently drops a real capability. It stays the sole source of the mapping rule.
        capabilities: caps.into_iter().filter_map(Capability::as_wire_key).collect(),
    }
}

/// The JSON body of `GET /v1/service-info`: the BARE `ServiceInfo`, not the RPC response
/// wrapper (SMA-499 D3 — the wrapper exists only to satisfy buf lint).
///
/// `capabilities` is a plain `Vec<String>` with no `skip_serializing_if`, so serde emits `[]`
/// for an empty list. That is the MUST-emit-defaults rule holding by construction rather than
/// by anyone remembering it.
#[derive(Debug, Serialize)]
pub struct ServiceInfoDto {
    pub service: String,
    pub version: String,
    pub capabilities: Vec<String>,
}

impl From<&ServiceInfo> for ServiceInfoDto {
    fn from(info: &ServiceInfo) -> Self {
        ServiceInfoDto {
            service: info.service.clone(),
            version: info.version.clone(),
            capabilities: info.capabilities.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_proto::paigasus::common::v1::Capability;

    #[test]
    fn the_unspecified_sentinel_is_never_advertised() {
        let info = descriptor("iam", "1.2.3", &[Capability::Unspecified, Capability::IamAudit]);
        assert_eq!(info.capabilities, vec!["iam.audit".to_string()]);
    }

    #[test]
    fn duplicates_are_removed() {
        let info = descriptor("iam", "1.2.3", &[Capability::IamAudit, Capability::IamAudit]);
        assert_eq!(info.capabilities, vec!["iam.audit".to_string()]);
    }

    #[test]
    fn ordering_is_deterministic_regardless_of_input_order() {
        let forwards = descriptor("iam", "1.2.3", &[Capability::IamAuthzCedar, Capability::IamApikeys, Capability::IamAudit]);
        let backwards = descriptor("iam", "1.2.3", &[Capability::IamAudit, Capability::IamApikeys, Capability::IamAuthzCedar]);
        assert_eq!(forwards.capabilities, backwards.capabilities);
    }

    /// AC 4's only assertion that can actually fail while every crate is `version = "0.0.0"`:
    /// it pins that this crate neither rewrites nor substitutes the caller's version string.
    #[test]
    fn the_callers_version_flows_through_verbatim() {
        let info = descriptor("iam", "9.9.9-test-sentinel", &[]);
        assert_eq!(info.version, "9.9.9-test-sentinel");
        assert_eq!(info.service, "iam");
    }

    /// SMA-499 § 2.7: canonical protojson omits an empty repeated field, which would make a
    /// console doing `info.capabilities.includes(k)` throw instead of rendering "feature off".
    #[test]
    fn an_empty_capability_list_serializes_as_an_empty_array() {
        let dto = ServiceInfoDto::from(&descriptor("gateway", "0.0.0", &[]));
        let json = serde_json::to_string(&dto).expect("serialize");
        assert_eq!(json, r#"{"service":"gateway","version":"0.0.0","capabilities":[]}"#);
    }

    #[test]
    fn the_dto_field_names_match_the_protos_canonical_json_names() {
        let dto = ServiceInfoDto::from(&descriptor("iam", "0.0.0", &[Capability::IamAudit]));
        let json = serde_json::to_string(&dto).expect("serialize");
        assert_eq!(json, r#"{"service":"iam","version":"0.0.0","capabilities":["iam.audit"]}"#);
    }

    #[test]
    fn the_route_is_the_path_the_proto_specifies() {
        assert_eq!(ROUTE, "/v1/service-info");
    }
}
