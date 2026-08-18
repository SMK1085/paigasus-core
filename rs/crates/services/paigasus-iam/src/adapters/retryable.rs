// SPDX-License-Identifier: Apache-2.0

//! IAM's error → [`Retryable`] mappings, in ONE place so the HTTP and gRPC surfaces cannot
//! disagree about whether the same error is worth retrying (spec D4).

use paigasus_iam_core::AuthnError;
use paigasus_observability::Retryable;

use crate::application::error::ErrorClass;

/// `TenancyError` has no transient variant: everything but `Internal` is a client-actionable
/// failure, and `Internal` absorbs `RepositoryError::Backend` alongside genuine bugs with the
/// source erased at conversion — so it is honestly `Unknown`, not a confident `No`.
pub(crate) fn tenancy_retryable(class: ErrorClass) -> Retryable {
    match class {
        ErrorClass::Internal => Retryable::Unknown,
        ErrorClass::Validation | ErrorClass::NotFound | ErrorClass::Conflict | ErrorClass::Precondition | ErrorClass::Forbidden => Retryable::No,
    }
}

/// `Unavailable` is the one authn error that names a transient dependency failure. `Backend`
/// is `Unknown` for the same reason `TenancyError::Internal` is.
pub(crate) fn authn_retryable(err: &AuthnError) -> Retryable {
    match err {
        AuthnError::Unavailable => Retryable::Yes,
        AuthnError::Backend(_) => Retryable::Unknown,
        AuthnError::InvalidToken(_) | AuthnError::IdentityNotProvisioned | AuthnError::ProvisioningFailed(_) | AuthnError::PrincipalInactive => Retryable::No,
    }
}

/// Test-only fixtures shared across this crate's test modules — `pub(crate)` (rather than
/// private inside a single `mod tests`) so both this file's own tests AND
/// `adapters::http::authn`'s test module can drive the exhaustive `AuthnError` list off the
/// SAME function rather than each keeping its own copy that could silently drift apart.
#[cfg(test)]
pub(crate) mod tests_support {
    use paigasus_iam_core::AuthnError;

    /// `AuthnError` lives in `paigasus-iam-core`, so a `cfg(test)` `EnumIter` derive there would
    /// NOT be visible when THIS crate's tests compile. The exhaustive `match` below is the
    /// dependency-free equivalent: adding a variant upstream fails this file to compile.
    pub(crate) fn all_authn_errors() -> Vec<AuthnError> {
        use paigasus_iam_core::{ProvisioningDefect, TokenDefect};
        let all = vec![
            AuthnError::InvalidToken(TokenDefect::Malformed),
            AuthnError::IdentityNotProvisioned,
            AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail),
            AuthnError::PrincipalInactive,
            AuthnError::Unavailable,
            AuthnError::Backend("x".into()),
        ];
        // Exhaustiveness guard: no wildcard arm, so a new variant is a compile error here.
        for e in &all {
            match e {
                AuthnError::InvalidToken(_)
                | AuthnError::IdentityNotProvisioned
                | AuthnError::ProvisioningFailed(_)
                | AuthnError::PrincipalInactive
                | AuthnError::Unavailable
                | AuthnError::Backend(_) => {}
            }
        }
        all
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::all_authn_errors;
    use super::*;
    use crate::application::error::{ErrorClass, TenancyError};
    use paigasus_iam_core::AuthnError;
    use paigasus_observability::Retryable;
    use strum::IntoEnumIterator;

    /// D4: `TenancyError` has NO transient variant — every one of its 26 codes is a client-actionable
    /// failure except `Internal`, which erases whether its source was a Postgres blip or a bug.
    #[test]
    fn every_tenancy_error_maps_to_no_except_internal() {
        for err in TenancyError::iter() {
            let want = if matches!(err.class(), ErrorClass::Internal) { Retryable::Unknown } else { Retryable::No };
            assert_eq!(tenancy_retryable(err.class()), want, "{err:?}");
        }
    }

    #[test]
    fn only_the_unavailable_authn_error_is_retryable() {
        for err in all_authn_errors() {
            let want = match &err {
                AuthnError::Unavailable => Retryable::Yes,
                AuthnError::Backend(_) => Retryable::Unknown,
                _ => Retryable::No,
            };
            assert_eq!(authn_retryable(&err), want, "{err:?}");
        }
    }
}
