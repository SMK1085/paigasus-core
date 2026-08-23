// SPDX-License-Identifier: Apache-2.0

//! Wire-string helpers for the canonical error registry (ADR-0019, SMA-498).
//!
//! [`ErrorReason`] and [`ErrorDomain`] are REGISTRIES, never wire types: the wire carries
//! `google.rpc.ErrorInfo.reason` / `.domain` as strings. These helpers convert between the
//! generated enum and that string, deriving BOTH directions from prost's `as_str_name` /
//! `from_str_name` rather than a match table, so the kebab spellings exist in exactly one
//! place — `contracts/proto/paigasus/common/v1/error.proto`. A table here would be a second
//! copy of the registry, which is the "three unlinked places" drift ADR-0019 cites.

use std::sync::LazyLock;

use crate::paigasus::common::v1::{ErrorDomain, ErrorReason};

/// The canonical wire `domain` for IAM-produced errors, `"iam.paigasus.io"`.
///
/// Derived from the registry rather than written as a literal, and living here rather than in
/// either service, because BOTH sides need it: IAM to emit, the gateway to match on. A literal
/// in the gateway would be a second hand-maintained copy of the vocabulary (ADR-0019 D8).
/// `as_wire_domain` returns `None` only for the zero sentinel, which `Iam` is not.
pub static IAM_DOMAIN: LazyLock<String> = LazyLock::new(|| ErrorDomain::Iam.as_wire_domain().expect("ErrorDomain::Iam is not the Unspecified sentinel"));

/// The canonical wire `domain` for gateway-produced errors, `"gateway.paigasus.io"`. See
/// [`IAM_DOMAIN`].
///
/// Only [`IAM_DOMAIN`] is load-bearing today — the gateway never emits a `Status` carrying its
/// own domain, so this is referenced solely by its own test below. Defensible as symmetry:
/// SMA-507's two-way drift gate and SMA-508's TS SDK are both expected to consume it.
pub static GATEWAY_DOMAIN: LazyLock<String> = LazyLock::new(|| ErrorDomain::Gateway.as_wire_domain().expect("ErrorDomain::Gateway is not the Unspecified sentinel"));

/// The suffix every canonical error domain carries.
const DOMAIN_SUFFIX: &str = ".paigasus.io";

/// The proto-name prefix buf's `ENUM_VALUE_PREFIX` lint rule requires on every reason.
const REASON_PREFIX: &str = "ERROR_REASON_";

/// The proto-name prefix buf's `ENUM_VALUE_PREFIX` lint rule requires on every domain.
const DOMAIN_PREFIX: &str = "ERROR_DOMAIN_";

/// The bare proto-name suffix of the zero sentinel, which is never a code.
const UNSPECIFIED: &str = "UNSPECIFIED";

/// Does `s` match `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`?
///
/// Hand-written rather than pulled from a regex crate to keep `paigasus-proto` free of
/// non-codegen dependencies. Validation is POSITIVE (an allow-list of shapes) rather than a
/// deny-list of characters, and strictly ASCII: `str::to_uppercase` folds `ı` (U+0131) to `I`
/// and `ſ` (U+017F) to `S`, so a deny-list would let those reconstruct a valid proto name and
/// resolve to a real code.
fn is_wire_token(s: &str) -> bool {
    if !s.starts_with(|c: char| c.is_ascii_lowercase()) || s.ends_with('-') {
        return false;
    }
    let mut prev_hyphen = false;
    for c in s.chars() {
        match c {
            'a'..='z' | '0'..='9' => prev_hyphen = false,
            '-' if !prev_hyphen => prev_hyphen = true,
            _ => return false,
        }
    }
    true
}

impl ErrorReason {
    /// The canonical wire `reason` for this value, e.g. `"slug-conflict"`.
    ///
    /// `None` for [`ErrorReason::Unspecified`]: that sentinel exists only to satisfy buf's
    /// `ENUM_ZERO_VALUE_SUFFIX` lint rule and is emitted by no surface. It is reachable because
    /// it is prost's `Default`, so returning `None` makes emitting it a caller-visible decision
    /// rather than a silent one.
    ///
    /// Returns an owned `String` deliberately: a borrowed `&'static str` would need a const
    /// table, i.e. the second copy of the registry this module exists to avoid. The allocation
    /// happens once per error response, and `tonic_types::ErrorDetails::with_error_info` takes
    /// `impl Into<String>` anyway.
    pub fn as_wire_reason(&self) -> Option<String> {
        let name = self.as_str_name().strip_prefix(REASON_PREFIX)?;
        if name == UNSPECIFIED {
            return None;
        }
        Some(name.to_ascii_lowercase().replace('_', "-"))
    }

    /// Parses a canonical wire `reason` back into a registry value; the exact inverse of
    /// [`ErrorReason::as_wire_reason`].
    ///
    /// Validates BEFORE transforming (see [`is_wire_token`]), so `"slug_conflict"`,
    /// `"SLUG-CONFLICT"` and Unicode look-alikes are rejected rather than folded into a valid
    /// name. A lenient parser would widen SMA-507's "emitted ⊆ registry" gate, which is the one
    /// thing that gate exists to prevent.
    pub fn from_wire_reason(reason: &str) -> Option<Self> {
        if !is_wire_token(reason) {
            return None;
        }
        let name = format!("{REASON_PREFIX}{}", reason.to_ascii_uppercase().replace('-', "_"));
        match Self::from_str_name(&name)? {
            Self::Unspecified => None,
            value => Some(value),
        }
    }
}

impl ErrorDomain {
    /// The canonical wire `domain` for this value, e.g. `"iam.paigasus.io"`. `None` for the
    /// zero sentinel, for the same reason as [`ErrorReason::as_wire_reason`].
    pub fn as_wire_domain(&self) -> Option<String> {
        let name = self.as_str_name().strip_prefix(DOMAIN_PREFIX)?;
        if name == UNSPECIFIED {
            return None;
        }
        Some(format!("{}{DOMAIN_SUFFIX}", name.to_ascii_lowercase().replace('_', "-")))
    }

    /// Parses a canonical wire `domain`; the exact inverse of [`ErrorDomain::as_wire_domain`].
    /// The `.paigasus.io` suffix is required, and the label is validated with the same positive
    /// ASCII check the reason parser uses.
    pub fn from_wire_domain(domain: &str) -> Option<Self> {
        let label = domain.strip_suffix(DOMAIN_SUFFIX)?;
        if !is_wire_token(label) {
            return None;
        }
        let name = format!("{DOMAIN_PREFIX}{}", label.to_ascii_uppercase().replace('-', "_"));
        match Self::from_str_name(&name)? {
            Self::Unspecified => None,
            value => Some(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::paigasus::common::v1::{ErrorDomain, ErrorReason};

    /// Every `ErrorReason` the registry declares. `::prost::Enumeration` provides
    /// `TryFrom<i32>`, so scanning the numbering ranges enumerates the enum without a
    /// hand-maintained list. The scan bound (9999) must stay comfortably above the highest
    /// declared number (999, the top of the shared range — see error.proto) or a value added
    /// above the bound goes invisible to every test derived from `all_reasons`/`all_domains`,
    /// including the `assert_eq!(actual.len(), 46)` anchor and the range-enforcement test.
    fn all_reasons() -> Vec<ErrorReason> {
        (0..=9999).filter_map(|i| ErrorReason::try_from(i).ok()).collect()
    }

    fn all_domains() -> Vec<ErrorDomain> {
        (0..=9999).filter_map(|i| ErrorDomain::try_from(i).ok()).collect()
    }

    /// The registry, spelled out. This DELIBERATELY duplicates error.proto — in a test,
    /// which is the right place for a redundant assertion. Without it every other test
    /// here is self-consistent by construction and a typo such as
    /// ERROR_REASON_UPSTREAM_TIMOUT would ship green.
    const EXPECTED_REASONS: &[&str] = &[
        // IAM: tenancy
        "slug-conflict",
        "duplicate-membership",
        "email-conflict",
        "service-account-name-conflict",
        "invalid-email",
        "invalid-slug",
        "invalid-name",
        "invalid-prn",
        "prn-mismatch",
        "invalid-pagination",
        "nothing-to-rename",
        "not-found",
        "parent-archived",
        "node-archived",
        "missing-org-membership",
        "forbidden",
        "unknown-role",
        "invalid-scope",
        "system-immutable",
        "policy-invalid",
        "policy-conflict",
        "invalid-action",
        "invalid-bulk-replay",
        "not-system-owned",
        "fleet-not-converged",
        // IAM: authn
        "invalid-token",
        "identity-not-provisioned",
        "provisioning-failed",
        "principal-inactive",
        "authn-unavailable",
        // IAM: system-row retirement
        "grants-survive",
        "decision-change-unacknowledged",
        // IAM: request validation (SMA-586)
        "invalid-timestamp",
        "invalid-uuid",
        "invalid-cursor",
        "invalid-audit-outcome",
        "missing-required-field",
        "mutually-exclusive-fields",
        // Gateway
        "missing-authorization",
        "invalid-api-key",
        "insufficient-permissions",
        "missing-scope",
        "iam-unavailable",
        "upstream-unavailable",
        "upstream-timeout",
        "upstream-error",
        "streaming-disabled",
        // Shared
        "internal",
        "invalid-request-body",
        "request-too-large",
        "missing-auth-context",
        "capability-disabled",
    ];

    #[test]
    fn the_registry_contains_exactly_the_expected_reasons() {
        let actual: std::collections::BTreeSet<String> = all_reasons().iter().filter_map(|r| r.as_wire_reason()).collect();
        let expected: std::collections::BTreeSet<String> = EXPECTED_REASONS.iter().map(|s| (*s).to_string()).collect();

        let missing: Vec<_> = expected.difference(&actual).collect();
        let unexpected: Vec<_> = actual.difference(&expected).collect();
        assert!(missing.is_empty(), "declared in the test but not in the registry: {missing:?}");
        assert!(unexpected.is_empty(), "in the registry but not declared in the test: {unexpected:?}");
        assert_eq!(actual.len(), 52, "the registry should hold 52 reasons");
    }

    #[test]
    fn the_registry_contains_exactly_the_expected_domains() {
        let actual: Vec<String> = all_domains().iter().filter_map(|d| d.as_wire_domain()).collect();
        assert_eq!(actual, vec!["iam.paigasus.io".to_string(), "gateway.paigasus.io".to_string()]);
    }

    #[test]
    fn every_reason_round_trips() {
        for reason in all_reasons() {
            let Some(wire) = reason.as_wire_reason() else {
                continue; // the Unspecified sentinel, covered by its own test
            };
            assert_eq!(ErrorReason::from_wire_reason(&wire), Some(reason), "round-trip failed for {wire}");
        }
    }

    #[test]
    fn every_domain_round_trips() {
        for domain in all_domains() {
            let Some(wire) = domain.as_wire_domain() else {
                continue;
            };
            assert_eq!(ErrorDomain::from_wire_domain(&wire), Some(domain), "round-trip failed for {wire}");
        }
    }

    /// The zero sentinel exists only to satisfy buf's ENUM_ZERO_VALUE_SUFFIX lint rule and is
    /// never emitted. It is reachable because it is prost's `Default`, so both directions
    /// refuse it rather than silently inventing an "unspecified" code.
    #[test]
    fn the_unspecified_sentinel_is_not_a_code() {
        assert_eq!(ErrorReason::Unspecified.as_wire_reason(), None);
        assert_eq!(ErrorDomain::Unspecified.as_wire_domain(), None);
        assert_eq!(ErrorReason::from_wire_reason("unspecified"), None);
        assert_eq!(ErrorDomain::from_wire_domain("unspecified.paigasus.io"), None);
    }

    #[test]
    fn every_wire_reason_is_a_well_formed_token() {
        for reason in all_reasons() {
            let Some(wire) = reason.as_wire_reason() else { continue };
            assert!(!wire.is_empty(), "empty wire string");
            assert!(!wire.contains('_'), "{wire} contains an underscore");
            assert!(wire.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{wire} has a non-token character");
            assert!(!wire.starts_with('-') && !wire.ends_with('-'), "{wire} has a leading or trailing hyphen");
            assert!(!wire.contains("--"), "{wire} has a doubled hyphen");
        }
    }

    /// Strictness matters: a lenient parser would widen SMA-507's "emitted subset of registry"
    /// gate, letting a misspelled emitted code pass. The Unicode cases are the sharp ones —
    /// `str::to_uppercase` folds U+0131 to 'I' and U+017F to 'S', so without an ASCII-only
    /// positive check these reconstruct a valid proto name and resolve.
    #[test]
    fn from_wire_reason_rejects_malformed_input() {
        for bad in [
            "slug_conflict",
            "SLUG-CONFLICT",
            "Slug-Conflict",
            "",
            "-slug",
            "slug-",
            "slug--conflict",
            "no-such-code",
            "ınternal",
            "ſlug-conflict",
        ] {
            assert_eq!(ErrorReason::from_wire_reason(bad), None, "{bad:?} must not resolve");
        }
    }

    #[test]
    fn from_wire_domain_requires_the_suffix() {
        assert_eq!(ErrorDomain::from_wire_domain("iam"), None);
        assert_eq!(ErrorDomain::from_wire_domain("iam.example.com"), None);
        assert_eq!(ErrorDomain::from_wire_domain("IAM.paigasus.io"), None);
        assert_eq!(ErrorDomain::from_wire_domain("iam.paigasus.io"), Some(ErrorDomain::Iam));
    }

    /// The numbering ranges are what lets SMA-507 decide which service may emit which code.
    #[test]
    fn every_reason_number_is_in_a_declared_range() {
        for reason in all_reasons() {
            let n = reason as i32;
            if n == 0 {
                continue; // the sentinel
            }
            assert!(
                (1..=299).contains(&n) || (300..=599).contains(&n) || (900..=999).contains(&n),
                "{reason:?} has number {n}, outside the IAM / gateway / shared ranges"
            );
        }
    }

    /// ADR-0019 quotes these spellings directly; they are the ones a reader will check first.
    #[test]
    fn the_adr_examples_are_spelled_as_documented() {
        assert_eq!(ErrorReason::SlugConflict.as_wire_reason().as_deref(), Some("slug-conflict"));
        assert_eq!(ErrorReason::ParentArchived.as_wire_reason().as_deref(), Some("parent-archived"));
        assert_eq!(ErrorReason::NothingToRename.as_wire_reason().as_deref(), Some("nothing-to-rename"));
        assert_eq!(ErrorDomain::Iam.as_wire_domain().as_deref(), Some("iam.paigasus.io"));
    }

    /// The domain strings live HERE, next to `as_wire_domain` they are derived from, because the
    /// gateway must compare `ErrorInfo.domain` against IAM's domain and cannot see a constant
    /// private to the IAM crate. A hardcoded copy in the gateway is exactly the second
    /// hand-maintained vocabulary ADR-0019's registry exists to prevent.
    #[test]
    fn the_domain_constants_match_the_registry() {
        assert_eq!(&*crate::error::IAM_DOMAIN, "iam.paigasus.io");
        assert_eq!(&*crate::error::GATEWAY_DOMAIN, "gateway.paigasus.io");
    }

    /// SMA-586: the six request-validation reasons that replace `invalid-prn`'s catch-all duty.
    /// Asserted by wire string rather than by enum variant so a renumbering or a rename that
    /// silently changes the kebab spelling fails here too.
    #[test]
    fn the_request_validation_reasons_resolve_both_ways() {
        for (variant, wire) in [
            (ErrorReason::InvalidTimestamp, "invalid-timestamp"),
            (ErrorReason::InvalidUuid, "invalid-uuid"),
            (ErrorReason::InvalidCursor, "invalid-cursor"),
            (ErrorReason::InvalidAuditOutcome, "invalid-audit-outcome"),
            (ErrorReason::MissingRequiredField, "missing-required-field"),
            (ErrorReason::MutuallyExclusiveFields, "mutually-exclusive-fields"),
        ] {
            assert_eq!(variant.as_wire_reason().as_deref(), Some(wire));
            assert_eq!(ErrorReason::from_wire_reason(wire), Some(variant));
        }
    }
}
