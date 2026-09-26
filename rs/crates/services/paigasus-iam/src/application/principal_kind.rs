// SPDX-License-Identifier: Apache-2.0

//! The principal-kind filter of `ListRoleGrants` and `ListMemberships` (SMA-676 D7, D8).
//!
//! Each transport maps its raw value into [`PrincipalKindFilter`]; the service calls
//! [`PrincipalKindFilter::resolve`]. An unknown value is its own variant, so no adapter can
//! turn it into "any kind": the refusal lives in one place for both transports (D10).

use crate::application::error::TenancyError;
use paigasus_iam_core::PrincipalKind;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrincipalKindFilter {
    /// No filter (gRPC `PRINCIPAL_KIND_UNSPECIFIED`, or no HTTP parameter).
    #[default]
    Any,
    Only(PrincipalKind),
    /// A value that names no kind. [`PrincipalKindFilter::resolve`] refuses it.
    Unknown,
}

impl PrincipalKindFilter {
    /// The HTTP query value: absent is `Any`; `user` and `service_account` (the strings of
    /// `PrincipalKind::as_str`) name a kind; anything else, the empty string included, is
    /// `Unknown`.
    #[must_use]
    pub fn from_query(raw: Option<&str>) -> Self {
        match raw {
            None => Self::Any,
            Some(s) => PrincipalKind::parse(s).map_or(Self::Unknown, Self::Only),
        }
    }

    /// `None` = any kind. `Unknown` is refused (D7).
    pub fn resolve(self) -> Result<Option<PrincipalKind>, TenancyError> {
        match self {
            Self::Any => Ok(None),
            Self::Only(kind) => Ok(Some(kind)),
            Self::Unknown => Err(TenancyError::InvalidPrincipalKind("principal_kind")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SMA-676 D7 and Review Focus 3: every string that is not exactly `user` or
    /// `service_account` refuses. An empty value and a different case refuse too.
    #[test]
    fn principal_kind_filter_refuses_every_unknown_value() {
        assert_eq!(PrincipalKindFilter::from_query(None), PrincipalKindFilter::Any);
        assert_eq!(PrincipalKindFilter::from_query(Some("user")), PrincipalKindFilter::Only(PrincipalKind::User));
        assert_eq!(PrincipalKindFilter::from_query(Some("service_account")), PrincipalKindFilter::Only(PrincipalKind::ServiceAccount));
        for raw in ["", "users", "USER", " user", "any"] {
            assert_eq!(PrincipalKindFilter::from_query(Some(raw)), PrincipalKindFilter::Unknown, "{raw:?}");
        }
        assert_eq!(PrincipalKindFilter::Unknown.resolve(), Err(TenancyError::InvalidPrincipalKind("principal_kind")));
        assert_eq!(PrincipalKindFilter::Any.resolve(), Ok(None));
        assert_eq!(PrincipalKindFilter::Only(PrincipalKind::User).resolve(), Ok(Some(PrincipalKind::User)));
    }
}
