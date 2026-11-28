// SPDX-License-Identifier: Apache-2.0

//! The page token of the agent API: plain text `v1.<scope uuid>.<0|1>.<last uuid>`, opaque by
//! contract. The scope is the org (agents) or the agent (inboxes). The flag is
//! `include_archived` (always `0` for inboxes). A token of another scope or another flag, or one
//! that does not parse, is [`AdminError::InvalidPageToken`].

use uuid::Uuid;

use super::error::AdminError;

/// The version prefix of the token.
const VERSION: &str = "v1";

/// The token that continues a list after the row `after`.
#[must_use]
pub fn encode(scope: Uuid, include_archived: bool, after: Uuid) -> String {
    let flag = if include_archived { '1' } else { '0' };
    format!("{VERSION}.{scope}.{flag}.{after}")
}

/// Read a token of the list of `scope` with the filter `include_archived`. An empty token is the
/// first page (`Ok(None)`).
pub fn decode(token: &str, scope: Uuid, include_archived: bool) -> Result<Option<Uuid>, AdminError> {
    if token.is_empty() {
        return Ok(None);
    }
    let mut parts = token.split('.');
    let (Some(VERSION), Some(_), Some(_), Some(after), None) = (parts.next(), parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(AdminError::InvalidPageToken);
    };
    let after = Uuid::parse_str(after).map_err(|_| AdminError::InvalidPageToken)?;
    // The token must be exactly the one this list writes: the same scope, the same flag, and the
    // canonical spelling of each uuid. `Uuid::parse_str` also reads other spellings.
    if encode(scope, include_archived, after) != token {
        return Err(AdminError::InvalidPageToken);
    }
    Ok(Some(after))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn the_token_has_the_planned_plain_form() {
        assert_eq!(encode(u(1), false, u(2)), format!("v1.{}.0.{}", u(1), u(2)));
        assert_eq!(encode(u(1), true, u(2)), format!("v1.{}.1.{}", u(1), u(2)));
    }

    #[test]
    fn an_empty_token_is_the_first_page() {
        assert_eq!(decode("", u(1), false), Ok(None));
        assert_eq!(decode("", u(1), true), Ok(None));
    }

    #[test]
    fn a_token_decodes_for_its_own_scope_and_filter_only() {
        for flag in [false, true] {
            let token = encode(u(1), flag, u(9));
            assert_eq!(decode(&token, u(1), flag), Ok(Some(u(9))));
            assert_eq!(decode(&token, u(1), !flag), Err(AdminError::InvalidPageToken));
            assert_eq!(decode(&token, u(2), flag), Err(AdminError::InvalidPageToken));
        }
    }

    #[test]
    fn a_token_in_another_spelling_is_invalid() {
        let scope = Uuid::from_u128(0xabcdef);
        let after = Uuid::from_u128(0xfedcba);
        for token in [
            "garbage".to_owned(),
            "v1".to_owned(),
            format!("v2.{scope}.0.{after}"),
            format!("v1.{scope}.2.{after}"),
            format!("v1.{scope}.0.not-a-uuid"),
            format!("v1.{scope}.0.{after}.x"),
            format!("v1.{scope}.0.{after}."),
            format!(" v1.{scope}.0.{after}"),
            format!("v1.{scope}.0.{}", after.to_string().to_uppercase()),
            format!("v1.{scope}.0.{}", after.simple()),
            format!("v1.{}.0.{after}", scope.simple()),
        ] {
            assert_eq!(decode(&token, scope, false), Err(AdminError::InvalidPageToken), "{token:?}");
        }
    }
}
