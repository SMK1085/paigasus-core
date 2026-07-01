// SPDX-License-Identifier: Apache-2.0
//! Property + example coverage for the `Prn` value type: parse↔canonical round-trip, canonical
//! idempotence, and representative rejection paths (SMA-448).

use paigasus_kernel::Prn;
use proptest::prelude::*;
use uuid::Uuid;

fn any_uuid() -> impl Strategy<Value = Uuid> {
    any::<[u8; 16]>().prop_map(Uuid::from_bytes)
}

proptest! {
    #[test]
    fn build_parse_roundtrip(
        service in "[a-z][a-z0-9]{0,8}",
        rtype in "[a-z][a-z0-9]{0,8}",
        org_present: bool,
        org in any_uuid(),
        rid in any_uuid(),
    ) {
        let org_opt = if org_present { Some(org) } else { None };
        let p = Prn::build(&service, "", org_opt, &rtype, rid).expect("valid build");
        let canon = p.canonical();
        let parsed = Prn::parse(&canon).expect("canonical reparses");
        prop_assert_eq!(&parsed, &p);
        prop_assert_eq!(parsed.canonical(), canon);
    }
}

#[test]
fn canonicalizes_mixed_case_uuid() {
    let upper = "prn:pgs:iam:::user/0190A1E5-0000-7000-8000-00000000ABCD";
    let lower = "prn:pgs:iam:::user/0190a1e5-0000-7000-8000-00000000abcd";
    assert_eq!(Prn::parse(upper).unwrap().canonical(), lower);
}

#[test]
fn rejects_with_expected_kind() {
    let cases = [
        ("", "empty"),
        ("xrn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004", "bad-scheme"),
        ("prn:pgz:iam:::user/0190a1e5-0000-7000-8000-000000000004", "bad-partition"),
        ("prn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004:x", "wrong-field-count"),
        ("prn:pgs:IAM:::user/0190a1e5-0000-7000-8000-000000000004", "bad-service"),
        ("prn:pgs:api--key:::user/0190a1e5-0000-7000-8000-000000000004", "bad-service"),
        ("prn:pgs:iam::not-a-uuid:team/0190a1e5-0000-7000-8000-000000000004", "bad-org"),
        ("prn:pgs:iam:::user", "bad-resource-path"),
        ("prn:pgs:iam:::user/a/b", "bad-resource-path"),
        ("prn:pgs:iam:::/0190a1e5-0000-7000-8000-000000000004", "bad-resource-type"),
        ("prn:pgs:iam:::team-1/0190a1e5-0000-7000-8000-000000000004", "bad-resource-type"),
        ("prn:pgs:iam:::user/not-a-uuid", "bad-resource-id"),
    ];
    for (input, kind) in cases {
        assert_eq!(Prn::parse(input).unwrap_err().kind(), kind, "input={input}");
    }
}
