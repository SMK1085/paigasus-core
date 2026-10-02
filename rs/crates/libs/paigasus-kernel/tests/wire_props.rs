// SPDX-License-Identifier: Apache-2.0
//! The wire form of `prn_parse_fields` holds for ANY input, not only for the corpus rows (SMA-673):
//! always six strings, the error kind agrees with `Prn::parse`, and an error row has five empty
//! fields.

use paigasus_kernel::Prn;
use paigasus_kernel::wire::prn_parse_fields;
use proptest::prelude::*;

fn check(s: &str) -> Result<(), TestCaseError> {
    let wire = prn_parse_fields(s);
    prop_assert_eq!(wire.len(), 6);
    match Prn::parse(s) {
        Ok(_) => {
            prop_assert_eq!(&wire[0], "");
            // service, resource type and resource id are never empty on a valid PRN.
            prop_assert!(!wire[1].is_empty() && !wire[4].is_empty() && !wire[5].is_empty());
        }
        Err(e) => {
            prop_assert_eq!(&wire[0], e.kind());
            prop_assert!(wire[1..].iter().all(String::is_empty), "error row with a non-empty field: {:?}", wire);
        }
    }
    Ok(())
}

proptest! {
    #[test]
    fn any_string_gives_six_consistent_strings(s in "\\PC{0,700}") {
        check(&s)?;
    }

    #[test]
    fn prn_shaped_strings_give_six_consistent_strings(
        service in "[a-zA-Z0-9-]{0,6}",
        region in "[a-zA-Z0-9-]{0,6}",
        org in "[0-9a-fA-F-]{0,36}",
        rtype in "[a-z0-9/-]{0,8}",
        rid in "[0-9a-fA-F-]{0,36}",
    ) {
        check(&format!("prn:pgs:{service}:{region}:{org}:{rtype}/{rid}"))?;
    }
}

#[test]
fn a_very_long_and_a_colon_only_input_give_six_strings() {
    check(&"x".repeat(10_000)).unwrap();
    check(":::::").unwrap();
    check("prn:pgs:iam:::user/\u{0}").unwrap();
}
