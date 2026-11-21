// SPDX-License-Identifier: Apache-2.0

//! The NATS tenancy rule (spec §9.4, D6 = T1): an org filter that fails closed.

use paigasus_kernel::resource_name::Prn;
use uuid::Uuid;

/// True only when the CloudEvents `subject` names the organization `org`.
#[must_use]
pub fn names_org(subject_attr: Option<&str>, org: Uuid) -> bool {
    let Some(subject) = subject_attr else {
        return false;
    };
    // A subject that does not parse is skipped. The PRN parser compares UUIDs as values, so case is no issue.
    let Ok(prn) = Prn::parse(subject) else {
        return false;
    };
    // An organization PRN names its own org by the resource id and has no org segment. One with an
    // org segment is no org PRN of this org, whatever the segment says, so rule 2 does not see it.
    if prn.service() == "iam" && prn.resource_type() == "organization" {
        return prn.org().is_none() && prn.resource_id() == org;
    }
    prn.org() == Some(org)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORG: u128 = 0x500;

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn org_prn(o: Uuid) -> String {
        format!("prn:pgs:iam:::organization/{o}")
    }

    /// The same PRN with the UUID in upper case.
    fn org_prn_upper(o: Uuid) -> String {
        format!("prn:pgs:iam:::organization/{}", o.to_string().to_uppercase())
    }

    #[test]
    fn the_org_prn_of_the_inbox_org_passes() {
        assert!(names_org(Some(&org_prn(u(ORG))), u(ORG)));
        assert!(names_org(Some(&org_prn_upper(u(ORG))), u(ORG)));
    }

    #[test]
    fn a_prn_with_the_org_segment_passes() {
        let x = u(0x77);
        assert!(names_org(Some(&format!("prn:pgs:iam::{}:team/{x}", u(ORG))), u(ORG)));
        assert!(names_org(Some(&format!("prn:pgs:agents::{}:inbox/{x}", u(ORG))), u(ORG)));
        assert!(names_org(Some(&format!("prn:pgs:agents::{}:inbox/{x}", u(ORG).to_string().to_uppercase())), u(ORG)));
    }

    #[test]
    fn another_org_fails() {
        let x = u(0x77);
        assert!(!names_org(Some(&org_prn(u(0x600))), u(ORG)));
        assert!(!names_org(Some(&org_prn_upper(u(0x600))), u(ORG)));
        assert!(!names_org(Some(&format!("prn:pgs:iam::{}:team/{x}", u(0x600))), u(ORG)));
    }

    #[test]
    fn an_org_prn_with_an_org_segment_is_checked_by_the_segment_only() {
        let (a, b) = (u(ORG), u(0x600));
        // The segment names another org, the resource id names the inbox org.
        assert!(!names_org(Some(&format!("prn:pgs:iam::{b}:organization/{a}")), a));
        // The segment names the inbox org, the resource id names another org.
        assert!(!names_org(Some(&format!("prn:pgs:iam::{a}:organization/{b}")), b));
        assert!(!names_org(Some(&format!("prn:pgs:iam::{a}:organization/{b}")), a));
    }

    #[test]
    fn no_subject_or_no_org_fails_closed() {
        let x = u(0x77);
        assert!(!names_org(None, u(ORG)));
        assert!(!names_org(Some(""), u(ORG)));
        assert!(!names_org(Some(&format!("prn:pgs:iam:::principal/{x}")), u(ORG)));
        assert!(!names_org(Some("not a prn"), u(ORG)));
        // The resource id equals the org, but the type or the service is wrong.
        assert!(!names_org(Some(&format!("prn:pgs:iam:::team/{}", u(ORG))), u(ORG)));
        assert!(!names_org(Some(&format!("prn:pgs:agents:::organization/{}", u(ORG))), u(ORG)));
    }
}
