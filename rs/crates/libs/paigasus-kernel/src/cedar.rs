// SPDX-License-Identifier: Apache-2.0
//! PRN → Cedar entity-uid mapping (ADR-0013/ADR-0014), defined in the kernel so every service
//! agrees. Pure string mapping — no `cedar-policy` dependency. `service` + `resource-type` →
//! `Pgs::<Service>::<Type>`; `resource-id` → the entity id (SMA-448).

use crate::Prn;

/// A Cedar entity uid as plain strings: `entity_type` like `Pgs::Iam::Project`, `entity_id` the
/// resource-id UUID. Org/team parentage is conveyed separately as Cedar `parents`, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CedarUid {
    pub entity_type: String,
    pub entity_id: String,
}

/// Map a PRN to its Cedar entity uid.
#[must_use]
pub fn to_cedar_uid(prn: &Prn) -> CedarUid {
    CedarUid {
        entity_type: format!("Pgs::{}::{}", pascal(prn.service()), pascal(prn.resource_type())),
        entity_id: prn.resource_id().as_hyphenated().to_string(),
    }
}

/// PascalCase a validated kebab label: upper-case the first char of each `-` segment and join.
/// The label grammar (no empty segments) makes this injective.
fn pascal(label: &str) -> String {
    label
        .split('-')
        .map(|seg| {
            let mut chars = seg.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::to_cedar_uid;
    use crate::Prn;

    fn cedar(prn: &str) -> (String, String) {
        let c = to_cedar_uid(&Prn::parse(prn).unwrap());
        (c.entity_type, c.entity_id)
    }

    #[test]
    fn maps_namespace_type_and_id() {
        let (ty, id) = cedar("prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:service-account/0190a1d4-0000-7000-8000-000000000003");
        assert_eq!(ty, "Pgs::Iam::ServiceAccount");
        assert_eq!(id, "0190a1d4-0000-7000-8000-000000000003");
    }

    #[test]
    fn pascal_case_is_injective_over_multi_dash() {
        assert_eq!(
            cedar("prn:pgs:gateway::0190a100-0000-7000-8000-0000000000aa:api-key/0190a1f6-0000-7000-8000-000000000005").0,
            "Pgs::Gateway::ApiKey"
        );
        assert_eq!(cedar("prn:pgs:iam:::organization/0190a1e5-0000-7000-8000-000000000000").0, "Pgs::Iam::Organization");
        assert_eq!(cedar("prn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004").0, "Pgs::Iam::User");
    }
}
