// SPDX-License-Identifier: Apache-2.0

//! Wire-key transform for the `Capability` registry in
//! `paigasus/common/v1/service_info.proto`.

use crate::paigasus::common::v1::Capability;

/// The prefix every generated `Capability` value name carries.
const PREFIX: &str = "CAPABILITY_";

impl Capability {
    /// This capability's wire string, or `None` for the zero sentinel.
    ///
    /// Derived from prost's `as_str_name()` by the registry's mapping rule —
    /// strip `CAPABILITY_`, lowercase, `_` to `.` — never tabulated, so there is
    /// no second copy of the registry to drift against the proto.
    ///
    /// Returns `None` for [`Capability::Unspecified`] because that variant is
    /// prost's `Default`: a default-initialised or out-of-range-decoded value
    /// would otherwise silently advertise `"unspecified"` to every client.
    pub fn as_wire_key(self) -> Option<String> {
        let name = self.as_str_name().strip_prefix(PREFIX)?;
        if name == "UNSPECIFIED" {
            return None;
        }
        Some(name.to_ascii_lowercase().replace('_', "."))
    }

    /// The capability a wire string names, or `None` if it is not a registered key.
    ///
    /// The grammar is checked **positively, before** any transformation. A
    /// negative filter — rejecting `_` and ASCII uppercase — is not sufficient:
    /// `str::to_uppercase` folds U+0131 (dotless i) to `I`, so `"ıam.audit"`
    /// would otherwise resolve to a real capability.
    pub fn from_wire_key(key: &str) -> Option<Self> {
        if !is_wire_key(key) {
            return None;
        }
        // Belt-and-braces: `is_wire_key` has already restricted `key` to ASCII
        // lowercase/digits, so plain `to_uppercase` would behave identically
        // today. Keep `to_ascii_uppercase` anyway — if the grammar check is
        // ever loosened to admit non-ASCII, `to_uppercase` folds homoglyphs
        // (e.g. U+0131 dotless i -> 'I') into real capability names, while
        // `to_ascii_uppercase` cannot. Do not "simplify" this to `to_uppercase`.
        let name = format!("{PREFIX}{}", key.to_ascii_uppercase().replace('.', "_"));
        match Self::from_str_name(&name)? {
            // "unspecified" satisfies the grammar, so reject the sentinel here.
            Self::Unspecified => None,
            capability => Some(capability),
        }
    }
}

/// `^[a-z][a-z0-9]*(\.[a-z0-9]+)*$`, hand-rolled to avoid a `regex` dependency.
fn is_wire_key(key: &str) -> bool {
    key.starts_with(|c: char| c.is_ascii_lowercase())
        && key
            .split('.')
            .all(|segment| !segment.is_empty() && segment.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::Capability;

    /// Every registered capability. Deliberately explicit: prost generates no
    /// variant iterator. `adding_a_capability_forces_updating_these_tests`
    /// below is what stops this list going stale.
    const ALL: [Capability; 4] = [Capability::IamAuthzCedar, Capability::IamApikeys, Capability::IamAudit, Capability::GatewayChatStream];

    #[test]
    fn every_capability_round_trips() {
        for cap in ALL {
            let key = cap.as_wire_key().expect("a registered capability has a wire key");
            assert_eq!(Capability::from_wire_key(&key), Some(cap), "round-trip failed for {key}");
        }
    }

    #[test]
    fn the_registry_spells_the_adr_keys_exactly() {
        assert_eq!(Capability::IamAuthzCedar.as_wire_key().unwrap(), "iam.authz.cedar");
        assert_eq!(Capability::IamApikeys.as_wire_key().unwrap(), "iam.apikeys");
        assert_eq!(Capability::IamAudit.as_wire_key().unwrap(), "iam.audit");
        assert_eq!(Capability::GatewayChatStream.as_wire_key().unwrap(), "gateway.chat.stream");
    }

    #[test]
    fn the_zero_sentinel_has_no_wire_key_in_either_direction() {
        assert_eq!(Capability::Unspecified.as_wire_key(), None);
        assert_eq!(Capability::from_wire_key("unspecified"), None);
        // Unspecified is prost's Default, so this is the realistic hazard: a
        // default-initialised descriptor must not advertise "unspecified".
        assert_eq!(Capability::default().as_wire_key(), None);
    }

    #[test]
    fn wire_keys_match_the_documented_grammar() {
        for cap in ALL {
            let key = cap.as_wire_key().unwrap();
            assert!(key.starts_with(|c: char| c.is_ascii_lowercase()), "{key} must start with a letter");
            for segment in key.split('.') {
                assert!(!segment.is_empty(), "{key} has an empty segment");
                assert!(segment.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()), "segment {segment} of {key} is not [a-z0-9]");
            }
        }
    }

    #[test]
    fn from_wire_key_rejects_malformed_input() {
        for bad in [
            "iam_audit", // wrong separator: uppercases into a real proto name
            "IAM.AUDIT", // wrong casing
            "Iam.Audit",
            "unspecified", // the zero sentinel is not a key
            "",
            ".iam.audit",  // leading dot
            "iam.audit.",  // trailing dot
            "iam..audit",  // empty segment
            "iam.unknown", // well-formed but unregistered
            // Non-ASCII homoglyphs. Rejected by the grammar check
            // (`is_wire_key`) before any case-folding runs — see
            // `is_wire_key_rejects_non_ascii_homoglyphs` below for the test
            // that pins the guard actually doing this work.
            "ıam.audit", // U+0131 dotless i
            "ſervice.x", // U+017F long s
        ] {
            assert_eq!(Capability::from_wire_key(bad), None, "{bad:?} must not resolve");
        }
    }

    #[test]
    fn is_wire_key_rejects_non_ascii_homoglyphs() {
        // This is the guard that actually keeps `from_wire_key` safe from
        // Unicode case-folding surprises: `str::to_uppercase` (unlike
        // `to_ascii_uppercase`) folds U+0131 (dotless i) to `I` and U+017F
        // (long s) to `S`, which would let these resolve to real
        // capabilities if the grammar check ever stopped rejecting them
        // first. Pinning it here directly means loosening `is_wire_key` —
        // e.g. to a negative filter, or to checking only the first
        // character — fails this test, instead of silently reopening the
        // homoglyph hole while `from_wire_key_rejects_malformed_input` keeps
        // passing by accident.
        assert!(!super::is_wire_key("ıam.audit"), "U+0131 dotless i must be rejected");
        assert!(!super::is_wire_key("ſervice.x"), "U+017F long s must be rejected");
        assert!(super::is_wire_key("iam.audit"), "a valid key must be accepted");
    }

    #[test]
    fn adding_a_capability_forces_updating_these_tests() {
        // ALL covers discriminants 1..=4. Registering a fifth value fails here,
        // which is the signal to extend ALL and the literals test above.
        assert!(Capability::try_from(5).is_err());
    }
}
