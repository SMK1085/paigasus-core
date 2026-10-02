// SPDX-License-Identifier: Apache-2.0
//! FFI wire forms of kernel values, FOR THE BINDINGS UNDER `rs/crates/bindings/` ONLY (SMA-673).
//!
//! A consumer of this crate must use [`crate::Prn`]. The shapes here are a binding convention, not
//! a domain API: a binding returns them across an FFI boundary, and each language wraps them in ONE
//! typed adapter. The module is `#[doc(hidden)]`, but it is `pub` in a published crate, so a change
//! to a shape here is a breaking change for the crate.
//!
//! This module has no FFI dependency and does no I/O. It fixes one marshalling convention in one
//! place, so that the wasm and napi bindings do not each copy it.

use crate::Prn;

/// Parse `s` ONCE and return `[error_kind, service, region, org, resource_type, resource_id]`.
///
/// The function is total: it never panics and always returns six strings. A valid PRN gives
/// `error_kind == ""` and the five canonical fields (`org` is `""` when the PRN has no org; the
/// UUIDs are lower-case and hyphenated). An invalid PRN gives the `PrnError::kind()` token and five
/// empty fields.
#[must_use]
pub fn prn_parse_fields(s: &str) -> Vec<String> {
    match Prn::parse(s) {
        Ok(p) => vec![
            String::new(),
            p.service().to_string(),
            p.region().to_string(),
            p.org().map(|u| u.as_hyphenated().to_string()).unwrap_or_default(),
            p.resource_type().to_string(),
            p.resource_id().as_hyphenated().to_string(),
        ],
        Err(e) => vec![e.kind().to_string(), String::new(), String::new(), String::new(), String::new(), String::new()],
    }
}

#[cfg(test)]
mod tests {
    use super::prn_parse_fields;

    const ORG: &str = "0190a100-0000-7000-8000-0000000000aa";
    const TEAM: &str = "0190a1b2-0000-7000-8000-000000000001";

    fn row(values: [&str; 6]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    #[test]
    fn a_valid_prn_with_an_org_gives_every_field() {
        let prn = format!("prn:pgs:iam::{ORG}:team/{TEAM}");
        assert_eq!(prn_parse_fields(&prn), row(["", "iam", "", ORG, "team", TEAM]));
    }

    #[test]
    fn a_valid_prn_without_an_org_gives_an_empty_org() {
        let id = "0190a1e5-0000-7000-8000-000000000000";
        let prn = format!("prn:pgs:iam:::organization/{id}");
        assert_eq!(prn_parse_fields(&prn), row(["", "iam", "", "", "organization", id]));
    }

    #[test]
    fn a_valid_prn_with_a_region_gives_the_region_in_position_two() {
        let prn = format!("prn:pgs:iam:eu-central-1:{ORG}:team/{TEAM}");
        assert_eq!(prn_parse_fields(&prn), row(["", "iam", "eu-central-1", ORG, "team", TEAM]));
    }

    #[test]
    fn an_upper_case_uuid_comes_back_lower_case() {
        let prn = "prn:pgs:iam:::user/0190A1E5-0000-7000-8000-00000000ABCD";
        assert_eq!(prn_parse_fields(prn), row(["", "iam", "", "", "user", "0190a1e5-0000-7000-8000-00000000abcd"]));
    }

    #[test]
    fn each_error_kind_gives_its_token_and_five_empty_fields() {
        let too_long = format!("prn:pgs:iam:::user/{}", "a".repeat(600));
        let cases: [(&str, &str); 11] = [
            ("", "empty"),
            (&too_long, "too-long"),
            ("xrn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004", "bad-scheme"),
            ("prn:pgz:iam:::user/0190a1e5-0000-7000-8000-000000000004", "bad-partition"),
            ("prn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004:extra", "wrong-field-count"),
            ("prn:pgs:IAM:::user/0190a1e5-0000-7000-8000-000000000004", "bad-service"),
            ("prn:pgs:iam:US-EAST:0190a100-0000-7000-8000-0000000000aa:team/0190a1b2-0000-7000-8000-000000000001", "bad-region"),
            ("prn:pgs:iam::not-a-uuid:team/0190a1b2-0000-7000-8000-000000000001", "bad-org"),
            ("prn:pgs:iam:::userwithoutslash", "bad-resource-path"),
            ("prn:pgs:iam:::/0190a1e5-0000-7000-8000-000000000004", "bad-resource-type"),
            ("prn:pgs:iam:::user/not-a-uuid", "bad-resource-id"),
        ];
        for (input, kind) in cases {
            assert_eq!(prn_parse_fields(input), row([kind, "", "", "", "", ""]), "input {input:?}");
        }
    }
}
