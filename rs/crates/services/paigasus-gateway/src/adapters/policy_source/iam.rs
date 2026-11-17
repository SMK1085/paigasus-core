// SPDX-License-Identifier: Apache-2.0

//! The IAM policy source (SMA-775, spec 9.4): the compiled floor and tenant sets, and the tenant
//! settings, fetched from paigasus-iam over gRPC.
//!
//! **The service credential.** The gateway sends its own service credential (an IAM API key that
//! holds `tool_policy_reader`) on the two fetch calls of this module and on no other call. The
//! per-request `IsAuthorized` self-query in `adapters::iam::client` keeps the caller's own key
//! (SMA-446 D9). The credential file is read on every fetch, so a rotation needs no restart. The
//! key, the file content and the policy text are never logged and never put in an error message.

use std::path::PathBuf;

use async_trait::async_trait;
use paigasus_gateway_core::{OnUnavailable, TenantMode};
use paigasus_proto::paigasus::iam::v1::tool_policy_service_client::ToolPolicyServiceClient;
use paigasus_proto::paigasus::iam::v1::{
    CompiledToolPolicySet, GetCompiledFloorPoliciesRequest, GetCompiledFloorPoliciesResponse, GetCompiledToolPoliciesRequest, GetCompiledToolPoliciesResponse, ToolPolicyFailMode, ToolPolicyMode,
    ToolPolicySetKind,
};
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Code, Request};
use uuid::Uuid;

use crate::adapters::iam::client::with_correlation;
use crate::domain::tool_policy::{PolicySource, PolicyText, SourceError, TenantPolicies, TenantSettings};

/// Fetches the tool policies from IAM.
#[derive(Debug, Clone)]
pub struct IamPolicySource {
    client: ToolPolicyServiceClient<Channel>,
    credential_file: PathBuf,
}

impl IamPolicySource {
    /// A source over `channel`. `credential_file` holds the service credential.
    pub fn new(channel: Channel, credential_file: impl Into<PathBuf>) -> Self {
        Self {
            client: ToolPolicyServiceClient::new(channel),
            credential_file: credential_file.into(),
        }
    }

    /// Read the credential file now (R8: on every fetch). The error text holds the error kind only.
    async fn bearer(&self) -> Result<MetadataValue<tonic::metadata::Ascii>, SourceError> {
        let raw = tokio::fs::read(&self.credential_file).await.map_err(|e| credential_error(&e.kind().to_string()))?;
        let key = std::str::from_utf8(&raw).map_err(|_| credential_error("invalid data"))?.trim_matches(|c: char| c.is_ascii_whitespace());
        if key.is_empty() {
            return Err(credential_error("empty"));
        }
        let mut value = MetadataValue::try_from(format!("Bearer {key}")).map_err(|_| credential_error("invalid data"))?;
        value.set_sensitive(true);
        Ok(value)
    }

    /// `message` as a request that carries the service credential and the correlation id.
    async fn request<T>(&self, message: T) -> Result<Request<T>, SourceError> {
        let bearer = self.bearer().await?;
        let mut req = with_correlation(Request::new(message));
        req.metadata_mut().insert("authorization", bearer);
        Ok(req)
    }
}

fn credential_error(detail: &str) -> SourceError {
    SourceError::Io(format!("service credential file: {detail}"))
}

/// A failed call: `NOT_FOUND` is "not found", every other status is an I/O error with its code.
fn status_error(status: &tonic::Status, what: &str) -> SourceError {
    if status.code() == Code::NotFound {
        SourceError::NotFound(what.to_owned())
    } else {
        SourceError::Io(format!("iam {:?}", status.code()))
    }
}

#[async_trait]
impl PolicySource for IamPolicySource {
    async fn floor(&self) -> Result<PolicyText, SourceError> {
        let req = self.request(GetCompiledFloorPoliciesRequest { version: None }).await?;
        let resp = self.client.clone().get_compiled_floor_policies(req).await.map_err(|s| status_error(&s, "floor"))?;
        floor_from_response(resp.into_inner())
    }

    async fn tenant(&self, org: Uuid) -> Result<TenantPolicies, SourceError> {
        let org_prn = format!("prn:pgs:iam:::organization/{}", org.hyphenated());
        let req = self.request(GetCompiledToolPoliciesRequest { org_prn, version: None }).await?;
        let resp = self.client.clone().get_compiled_tool_policies(req).await.map_err(|s| status_error(&s, "tenant"))?;
        tenant_from_response(org, resp.into_inner())
    }
}

fn malformed(why: &str) -> SourceError {
    SourceError::Malformed(why.to_owned())
}

/// The checks that the floor set and the tenant set share. R5: a set with a history condition is
/// refused until SMA-753 (IAM refuses to save such a set; this is defence in depth).
fn check_set(set: &CompiledToolPolicySet, kind: ToolPolicySetKind) -> Result<(), SourceError> {
    if ToolPolicySetKind::try_from(set.kind) != Ok(kind) {
        return Err(malformed("the set has the wrong kind"));
    }
    if !set.conditions.is_empty() {
        return Err(malformed("history conditions need SMA-753"));
    }
    Ok(())
}

/// The tenant part of a `GetCompiledToolPolicies` answer. Version 0 is a valid empty set.
pub(crate) fn tenant_from_response(org: Uuid, r: GetCompiledToolPoliciesResponse) -> Result<TenantPolicies, SourceError> {
    let set = r.set.ok_or_else(|| malformed("the answer has no set"))?;
    let settings = r.settings.ok_or_else(|| malformed("the answer has no settings"))?;
    check_set(&set, ToolPolicySetKind::Tenant)?;
    if set.version > 0 && Uuid::try_parse(&set.org_id).ok() != Some(org) {
        return Err(malformed("the set belongs to another organization"));
    }
    let mode = match ToolPolicyMode::try_from(settings.mode) {
        Ok(ToolPolicyMode::Off) => TenantMode::Off,
        Ok(ToolPolicyMode::LogOnly) => TenantMode::LogOnly,
        Ok(ToolPolicyMode::Enforce) => TenantMode::Enforce,
        _ => return Err(malformed("the tenant mode is unspecified or unknown")),
    };
    let (on_unavailable, fail) = match ToolPolicyFailMode::try_from(settings.fail_mode) {
        Ok(ToolPolicyFailMode::Open) => (OnUnavailable::Open, "open"),
        Ok(ToolPolicyFailMode::Closed) => (OnUnavailable::Closed, "closed"),
        _ => return Err(malformed("the fail mode is unspecified or unknown")),
    };
    // R7: a change of the set version, the mode or the fail mode changes the token. The catalog
    // and the text change only with the set version.
    Ok(TenantPolicies {
        text: set.cedar_policies,
        catalog: set.catalog_names,
        version: format!("t{}:{}:{}", set.version, mode.as_str(), fail),
        settings: Some(TenantSettings { mode, on_unavailable }),
    })
}

/// The floor of a `GetCompiledFloorPolicies` answer. Version 0 is a valid empty floor (R6).
pub(crate) fn floor_from_response(r: GetCompiledFloorPoliciesResponse) -> Result<PolicyText, SourceError> {
    let set = r.set.ok_or_else(|| malformed("the answer has no set"))?;
    check_set(&set, ToolPolicySetKind::Floor)?;
    Ok(PolicyText {
        text: set.cedar_policies,
        version: format!("f{}", set.version),
    })
}

#[cfg(test)]
mod tests {
    use paigasus_proto::paigasus::iam::v1::{HistoryCondition, ToolPolicySettings};

    use super::*;

    const ORG: &str = "0190a100-0000-7000-8000-0000000000a1";

    fn org() -> Uuid {
        Uuid::try_parse(ORG).expect("uuid")
    }

    fn set(version: u64) -> CompiledToolPolicySet {
        CompiledToolPolicySet {
            kind: ToolPolicySetKind::Tenant as i32,
            org_id: ORG.to_owned(),
            version,
            cedar_policies: "permit(principal, action, resource);".to_owned(),
            catalog_names: vec!["read_file".to_owned()],
            ..Default::default()
        }
    }

    fn settings(mode: ToolPolicyMode, fail: ToolPolicyFailMode) -> ToolPolicySettings {
        ToolPolicySettings {
            mode: mode as i32,
            fail_mode: fail as i32,
            ..Default::default()
        }
    }

    fn response(set: CompiledToolPolicySet, settings: ToolPolicySettings) -> GetCompiledToolPoliciesResponse {
        GetCompiledToolPoliciesResponse {
            set: Some(set),
            settings: Some(settings),
            floor_version: 3,
        }
    }

    fn condition() -> HistoryCondition {
        HistoryCondition {
            slot: "h0".to_owned(),
            policy_id: "p".to_owned(),
            ..Default::default()
        }
    }

    fn is_malformed<T: std::fmt::Debug>(r: Result<T, SourceError>) -> bool {
        matches!(r, Err(SourceError::Malformed(_)))
    }

    #[test]
    fn a_tenant_response_maps_text_catalog_settings_and_token() {
        let t = tenant_from_response(org(), response(set(7), settings(ToolPolicyMode::Enforce, ToolPolicyFailMode::Open))).expect("maps");
        assert_eq!(t.text, "permit(principal, action, resource);");
        assert_eq!(t.catalog, ["read_file"]);
        assert_eq!(t.version, "t7:enforce:open");
        assert_eq!(
            t.settings,
            Some(TenantSettings {
                mode: TenantMode::Enforce,
                on_unavailable: OnUnavailable::Open
            })
        );
    }

    #[test]
    fn a_settings_change_alone_changes_the_version_token() {
        let a = tenant_from_response(org(), response(set(7), settings(ToolPolicyMode::Enforce, ToolPolicyFailMode::Open))).expect("a");
        let b = tenant_from_response(org(), response(set(7), settings(ToolPolicyMode::LogOnly, ToolPolicyFailMode::Open))).expect("b");
        let c = tenant_from_response(org(), response(set(7), settings(ToolPolicyMode::Enforce, ToolPolicyFailMode::Closed))).expect("c");
        assert_eq!(
            [a.version.as_str(), b.version.as_str(), c.version.as_str()],
            ["t7:enforce:open", "t7:log_only:open", "t7:enforce:closed"]
        );
    }

    #[test]
    fn version_0_is_an_empty_tenant_set() {
        let empty = CompiledToolPolicySet {
            kind: ToolPolicySetKind::Tenant as i32,
            ..Default::default()
        };
        let t = tenant_from_response(org(), response(empty, settings(ToolPolicyMode::Off, ToolPolicyFailMode::Closed))).expect("valid");
        assert_eq!((t.text.as_str(), t.catalog.len(), t.version.as_str()), ("", 0, "t0:off:closed"));
    }

    #[test]
    fn a_set_with_a_history_condition_is_malformed() {
        let mut s = set(2);
        s.conditions.push(condition());
        let err = tenant_from_response(org(), response(s, settings(ToolPolicyMode::Enforce, ToolPolicyFailMode::Open))).expect_err("refused");
        assert!(err.to_string().contains("history conditions need SMA-753"), "{err}");
    }

    #[test]
    fn an_unspecified_mode_or_fail_mode_is_malformed() {
        assert!(is_malformed(tenant_from_response(
            org(),
            response(set(1), settings(ToolPolicyMode::Unspecified, ToolPolicyFailMode::Open))
        )));
        assert!(is_malformed(tenant_from_response(
            org(),
            response(set(1), settings(ToolPolicyMode::Off, ToolPolicyFailMode::Unspecified))
        )));
        let unknown = ToolPolicySettings {
            mode: 99,
            fail_mode: ToolPolicyFailMode::Open as i32,
            ..Default::default()
        };
        assert!(is_malformed(tenant_from_response(org(), response(set(1), unknown))));
        // An absent set or absent settings is malformed too, and so is a floor-kind set.
        assert!(is_malformed(tenant_from_response(org(), GetCompiledToolPoliciesResponse::default())));
        let no_settings = GetCompiledToolPoliciesResponse {
            set: Some(set(1)),
            settings: None,
            floor_version: 0,
        };
        assert!(is_malformed(tenant_from_response(org(), no_settings)));
        let mut floor_kind = set(1);
        floor_kind.kind = ToolPolicySetKind::Floor as i32;
        assert!(is_malformed(tenant_from_response(org(), response(floor_kind, settings(ToolPolicyMode::Off, ToolPolicyFailMode::Open)))));
    }

    #[test]
    fn a_set_for_another_org_is_malformed() {
        let mut s = set(2);
        s.org_id = "0190a100-0000-7000-8000-0000000000b2".to_owned();
        assert!(is_malformed(tenant_from_response(org(), response(s, settings(ToolPolicyMode::Off, ToolPolicyFailMode::Open)))));
        let mut s = set(2);
        s.org_id = String::new();
        assert!(is_malformed(tenant_from_response(org(), response(s, settings(ToolPolicyMode::Off, ToolPolicyFailMode::Open)))));
    }

    #[test]
    fn an_empty_floor_is_valid_and_a_floor_with_a_condition_is_malformed() {
        let floor = |version, conditions: Vec<HistoryCondition>| GetCompiledFloorPoliciesResponse {
            set: Some(CompiledToolPolicySet {
                kind: ToolPolicySetKind::Floor as i32,
                version,
                cedar_policies: "forbid(principal, action, resource);".to_owned(),
                conditions,
                ..Default::default()
            }),
        };
        let ok = floor_from_response(floor(4, vec![])).expect("valid");
        assert_eq!((ok.text.as_str(), ok.version.as_str()), ("forbid(principal, action, resource);", "f4"));
        assert_eq!(floor_from_response(floor(0, vec![])).expect("empty floor").version, "f0");
        assert!(is_malformed(floor_from_response(floor(4, vec![condition()]))));
        assert!(is_malformed(floor_from_response(GetCompiledFloorPoliciesResponse::default())));
        let tenant_kind = GetCompiledFloorPoliciesResponse { set: Some(set(1)) };
        assert!(is_malformed(floor_from_response(tenant_kind)));
    }
}
