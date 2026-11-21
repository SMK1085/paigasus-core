// SPDX-License-Identifier: Apache-2.0

//! The three byte fixtures that `paigasus-iam`'s encoder test pins (SMA-787).

use paigasus_cloudevents::{CloudEvent, ExtensionValue, parse};
use serde_json::json;
use uuid::Uuid;

const CREATED: &[u8] = include_bytes!("fixtures/iam/principal_created_micros.json");
const ORG: &[u8] = include_bytes!("fixtures/iam/organization_created_trailing_zeros.json");
const ARCHIVED: &[u8] = include_bytes!("fixtures/iam/principal_archived_zero_fraction.json");

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

#[test]
fn the_crate_parses_every_iam_fixture() {
    let e = parse(CREATED).unwrap();
    assert_eq!(e.id(), "00000000-0000-0000-0000-000000001234");
    assert_eq!(e.ty(), "iam.principal.created");
    assert_eq!(e.schemaversion(), Some(1));
    assert!(e.actorprn().is_some());
    assert!(e.correlationid().is_some());
    assert_eq!(e.time(), Some("2026-08-07T12:30:45.123456Z".parse().unwrap()));
    assert_eq!(e.data(), Some(&json!({"kind": "user", "nested": {"a": 1}})));

    let o = parse(ORG).unwrap();
    assert!(o.subject().is_some());
    assert_eq!(o.time(), Some("2026-08-07T12:30:45.100000Z".parse().unwrap()));
    assert!(o.actorprn().is_none());

    let a = parse(ARCHIVED).unwrap();
    assert!(a.subject().is_none());
    assert_eq!(a.time(), Some("2026-08-07T12:30:45.000000Z".parse().unwrap()));
    assert_eq!(a.data(), Some(&json!({})));
}

struct Case {
    id: u128,
    ty: &'static str,
    subject: Option<String>,
    actor: Option<String>,
    correlation: Option<u128>,
    time: &'static str,
    data: serde_json::Value,
    expected: &'static [u8],
}

#[test]
fn the_crate_builds_the_iam_bytes_from_the_same_values() {
    let cases = [
        Case {
            id: 0x1234,
            ty: "iam.principal.created",
            subject: Some(format!("prn:pgs:iam:::principal/{}", u(0xbb))),
            actor: Some(format!("prn:pgs:iam:::principal/{}", u(0xaa))),
            correlation: Some(9),
            time: "2026-08-07T12:30:45.123456Z",
            data: json!({"kind": "user", "nested": {"a": 1}}),
            expected: CREATED,
        },
        Case {
            id: 0x2,
            ty: "iam.organization.created",
            subject: Some(format!("prn:pgs:iam:::organization/{}", u(0x500))),
            actor: None,
            correlation: None,
            time: "2026-08-07T12:30:45.100000Z",
            data: json!({"slug": "acme"}),
            expected: ORG,
        },
        Case {
            id: 0x3,
            ty: "iam.principal.archived",
            subject: None,
            actor: None,
            correlation: None,
            time: "2026-08-07T12:30:45.000000Z",
            data: json!({}),
            expected: ARCHIVED,
        },
    ];
    for c in cases {
        let mut e = CloudEvent::new(u(c.id), "urn:paigasus:iam", c.ty, c.time.parse().unwrap()).unwrap();
        if let Some(s) = &c.subject {
            e = e.with_subject(s).unwrap();
        }
        let mut e = e.with_datacontenttype("application/json").unwrap();
        e.set_extension("schemaversion", ExtensionValue::Integer(1)).unwrap();
        if let Some(a) = c.actor {
            e.set_extension("actorprn", ExtensionValue::String(a)).unwrap();
        }
        if let Some(n) = c.correlation {
            e.set_extension("correlationid", ExtensionValue::String(u(n).to_string())).unwrap();
        }
        let e = e.with_data(c.data);
        assert_eq!(String::from_utf8(e.to_bytes()).unwrap(), String::from_utf8(c.expected.to_vec()).unwrap());
    }
}
