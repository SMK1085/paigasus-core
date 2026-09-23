// SPDX-License-Identifier: Apache-2.0

//! The authenticated caller identity a request carries after the G5 auth middleware
//! validates its bearer credential; consumed by the chat handler (G7) to authorize the
//! request and to log request metadata (never the prompt/response body or any other PII).

/// The caller identity resolved from a request's bearer credential (an IAM API key or
/// service-account token — G5 populates this via the IAM introspect call). Carried through
/// the request extensions so downstream handlers (G7) never need to re-authenticate.
#[derive(Debug, Clone)]
pub struct CallerContext {
    /// The authenticated principal's PRN (Paigasus Resource Name), e.g. a service account.
    pub principal_prn: String,
    /// The scope PRN the caller's credential was issued under (org/team/project), used to
    /// authorize the request against the target resource.
    pub scope_prn: String,
    /// The credential's own identifier (the API key's `key_id`, not the secret itself) —
    /// safe to log; identifies which key authenticated the request without leaking it.
    pub key_id: String,
}

use paigasus_kernel::Prn;
use uuid::Uuid;

/// What the `paigasus-org` request header held (SMA-635 D2). The adapter builds it; the domain
/// function below takes no transport type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgHeader<'a> {
    /// No `paigasus-org` header.
    Absent,
    /// Exactly one header, whose value is visible ASCII.
    One(&'a str),
    /// Two or more headers.
    Many,
}

/// Why no organization could be resolved. The HTTP adapter maps each case to a 400 with
/// `param: "paigasus-org"` (`adapters::http::error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgResolutionError {
    /// The header is not ONE UUID in the kernel's 36-character hyphenated form, or it repeats.
    InvalidOrgHeader,
    /// No header, and the user reaches zero or several organizations (D3).
    OrgRequired,
}

const IAM_SERVICE: &str = "iam";
const ORGANIZATION: &str = "organization";

/// Resolve the ONE organization an OIDC caller acts in (SMA-635 spec §4.2).
///
/// - `One(value)`: `value` must be a UUID in the 36-character hyphenated form (the kernel rule,
///   `resource_name.rs:101-107`). The header is NOT checked against `node_prns`: IAM decides that
///   in the self-query.
/// - `Many`: always an error.
/// - `Absent`: every PRN in `node_prns` (memberships and role-grant scopes) names at most one org:
///   the resource id of an `organization`, or the org slot of a `team` or `project`. A PRN that
///   does not parse, of another service, or of another type (a Root grant) is ignored. Exactly
///   one distinct org is the answer.
///
/// The answer is built with the kernel builder, so its canonical form is lower case
/// (`prn:pgs:iam:::organization/<uuid>`) whatever case the input used.
pub fn resolve_org(header: OrgHeader<'_>, node_prns: &[&str]) -> Result<Prn, OrgResolutionError> {
    match header {
        OrgHeader::Many => Err(OrgResolutionError::InvalidOrgHeader),
        OrgHeader::One(value) => parse_org_uuid(value).map(org_prn).ok_or(OrgResolutionError::InvalidOrgHeader),
        OrgHeader::Absent => {
            let mut found: Option<Uuid> = None;
            for raw in node_prns {
                let Some(org) = org_of(raw) else { continue };
                match found {
                    None => found = Some(org),
                    Some(existing) if existing == org => {}
                    Some(_) => return Err(OrgResolutionError::OrgRequired),
                }
            }
            found.map(org_prn).ok_or(OrgResolutionError::OrgRequired)
        }
    }
}

/// The kernel's UUID rule: exactly 36 characters, and `Uuid::try_parse` accepts it. The length
/// check rejects the simple (32), braced (38) and `urn:uuid:` (45) forms that `try_parse` would
/// otherwise accept.
fn parse_org_uuid(value: &str) -> Option<Uuid> {
    if value.len() != 36 {
        return None;
    }
    Uuid::try_parse(value).ok()
}

fn org_prn(id: Uuid) -> Prn {
    Prn::build(IAM_SERVICE, "", None, ORGANIZATION, id).expect("static organization PRN parts are valid")
}

/// The org a tenancy PRN belongs to, or `None` when the PRN names no org.
fn org_of(raw: &str) -> Option<Uuid> {
    let prn = Prn::parse(raw).ok()?;
    if prn.service() != IAM_SERVICE {
        return None;
    }
    match prn.resource_type() {
        ORGANIZATION => Some(prn.resource_id()),
        "team" | "project" => prn.org(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0190a100-0000-7000-8000-0000000000a1";
    const A_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";
    const A_PRN_UPPER: &str = "prn:pgs:iam:::organization/0190A100-0000-7000-8000-0000000000A1";
    const B_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000b2";
    const TEAM_IN_A: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:team/0190a1b2-0000-7000-8000-0000000000c3";
    const PROJECT_IN_A: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:project/0190a1c3-0000-7000-8000-0000000000d4";
    const PROJECT_IN_B: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000b2:project/0190a1c3-0000-7000-8000-0000000000d5";
    const ROOT_GRANT: &str = "prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000";
    const OTHER_SERVICE_ORG: &str = "prn:pgs:gateway:::organization/0190a100-0000-7000-8000-0000000000a1";

    fn ok(prn: &str) -> Result<String, OrgResolutionError> {
        Ok(prn.to_owned())
    }

    /// SMA-635 spec §4.2: every `OrgHeader` arm, a Root grant, another service, a PRN that does
    /// not parse, and duplicates of one org. Review Focus 2: an upper-case UUID gives the
    /// lower-case canonical PRN, and two spellings of one org count as ONE org.
    #[test]
    #[allow(clippy::type_complexity)]
    fn resolve_org_table() {
        let upper = A.to_uppercase();
        let cases: Vec<(&str, OrgHeader<'_>, Vec<&str>, Result<String, OrgResolutionError>)> = vec![
            ("one_valid", OrgHeader::One(A), vec![], ok(A_PRN)),
            ("one_uppercase", OrgHeader::One(&upper), vec![], ok(A_PRN)),
            ("one_ignores_grants", OrgHeader::One(A), vec![B_PRN], ok(A_PRN)),
            (
                "one_simple_form",
                OrgHeader::One("0190a1000000700080000000000000a1"),
                vec![A_PRN],
                Err(OrgResolutionError::InvalidOrgHeader),
            ),
            (
                "one_braced",
                OrgHeader::One("{0190a100-0000-7000-8000-0000000000a1}"),
                vec![A_PRN],
                Err(OrgResolutionError::InvalidOrgHeader),
            ),
            (
                "one_urn",
                OrgHeader::One("urn:uuid:0190a100-0000-7000-8000-0000000000a1"),
                vec![A_PRN],
                Err(OrgResolutionError::InvalidOrgHeader),
            ),
            ("one_not_a_uuid", OrgHeader::One("acme"), vec![A_PRN], Err(OrgResolutionError::InvalidOrgHeader)),
            ("one_empty", OrgHeader::One(""), vec![A_PRN], Err(OrgResolutionError::InvalidOrgHeader)),
            ("many", OrgHeader::Many, vec![A_PRN], Err(OrgResolutionError::InvalidOrgHeader)),
            ("absent_zero_orgs", OrgHeader::Absent, vec![], Err(OrgResolutionError::OrgRequired)),
            ("absent_one_org", OrgHeader::Absent, vec![A_PRN], ok(A_PRN)),
            ("absent_org_team_project_one_org", OrgHeader::Absent, vec![A_PRN, TEAM_IN_A, PROJECT_IN_A], ok(A_PRN)),
            ("absent_team_only", OrgHeader::Absent, vec![TEAM_IN_A], ok(A_PRN)),
            ("absent_two_orgs", OrgHeader::Absent, vec![A_PRN, B_PRN], Err(OrgResolutionError::OrgRequired)),
            ("absent_two_orgs_via_project", OrgHeader::Absent, vec![A_PRN, PROJECT_IN_B], Err(OrgResolutionError::OrgRequired)),
            ("absent_root_grant_ignored", OrgHeader::Absent, vec![ROOT_GRANT, A_PRN], ok(A_PRN)),
            ("absent_root_grant_only", OrgHeader::Absent, vec![ROOT_GRANT], Err(OrgResolutionError::OrgRequired)),
            ("absent_other_service_ignored", OrgHeader::Absent, vec![OTHER_SERVICE_ORG], Err(OrgResolutionError::OrgRequired)),
            ("absent_unparseable_ignored", OrgHeader::Absent, vec!["not-a-prn", A_PRN], ok(A_PRN)),
            ("absent_same_org_two_spellings", OrgHeader::Absent, vec![A_PRN, A_PRN_UPPER], ok(A_PRN)),
            ("absent_duplicate_org", OrgHeader::Absent, vec![A_PRN, A_PRN, TEAM_IN_A], ok(A_PRN)),
        ];
        for (name, header, prns, want) in cases {
            let got = resolve_org(header, &prns).map(|prn| prn.canonical());
            assert_eq!(got, want, "{name}");
        }
    }
}
