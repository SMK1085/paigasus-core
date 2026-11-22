// SPDX-License-Identifier: Apache-2.0
//! The strict JSON reading that the tool policy and the detectors share (§ 6.1.4): a duplicate
//! key, a bad nesting depth and the exact reading of a number. It has no Cedar type.

#[cfg(feature = "tool-policy")]
use std::collections::HashSet;
#[cfg(feature = "tool-policy")]
use std::fmt;

#[cfg(feature = "tool-policy")]
use serde::de::{Deserialize, Deserializer, Error as _, MapAccess, Visitor};
#[cfg(feature = "tool-policy")]
use serde_json::value::RawValue;

/// Arguments nested deeper than this are unparsed. `RawValue` skips the 128 level limit of
/// `serde_json`, so the walk checks the depth itself before it recurses.
pub(crate) const MAX_NESTING: usize = 128;
/// The largest exponent the number reader tracks. A larger one saturates here. No text length
/// reaches this bound, so the length of the fraction never cancels a saturated exponent.
#[cfg(any(feature = "tool-policy", test))]
const EXPONENT_CAP: i64 = i64::MAX / 4;

/// How a JSON number maps into Cedar, by value.
#[cfg(any(feature = "tool-policy", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NumberClass {
    Long(i64),
    Decimal(String),
    Text(String),
}
/// Classify a JSON number by its value, from its text, with no float. The text comes from a valid
/// JSON document. A value that `Long` or `decimal` cannot hold exactly, or text that this reader
/// cannot split into digits and an exponent, gives `Text` with the original text.
#[cfg(any(feature = "tool-policy", test))]
pub(crate) fn classify_number(text: &str) -> NumberClass {
    classify(text).unwrap_or_else(|| NumberClass::Text(text.to_owned()))
}

#[cfg(any(feature = "tool-policy", test))]
fn is_digits(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

/// The `i64` for a sign and a magnitude, if it fits.
#[cfg(any(feature = "tool-policy", test))]
fn to_i64(negative: bool, magnitude: u128) -> Option<i64> {
    if negative {
        (magnitude <= 1u128 << 63).then(|| (-(magnitude as i128)) as i64)
    } else {
        i64::try_from(magnitude).ok()
    }
}

/// `Some(class)` for a number that `Long` or `decimal` holds exactly; `None` means "keep the text".
#[cfg(any(feature = "tool-policy", test))]
fn classify(text: &str) -> Option<NumberClass> {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, text),
    };
    let (mantissa, exponent) = match rest.find(['e', 'E']) {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if int_part.is_empty() || !is_digits(int_part) || !is_digits(frac_part) {
        return None;
    }
    let mut exp: i64 = 0;
    if let Some(e) = exponent {
        let (exp_negative, digits) = match e.strip_prefix('-') {
            Some(d) => (true, d),
            None => (false, e.strip_prefix('+').unwrap_or(e)),
        };
        if digits.is_empty() || !is_digits(digits) {
            return None;
        }
        for b in digits.bytes() {
            exp = exp.saturating_mul(10).saturating_add(i64::from(b - b'0')).min(EXPONENT_CAP);
        }
        if exp_negative {
            exp = -exp;
        }
    }
    // value = sign * digits * 10^scale, with no leading and no trailing zero in `digits`.
    let mut digits: String = format!("{int_part}{frac_part}").trim_start_matches('0').to_owned();
    if digits.is_empty() {
        return Some(NumberClass::Long(0));
    }
    let mut scale = exp.saturating_sub(i64::try_from(frac_part.len()).ok()?);
    while digits.ends_with('0') {
        digits.pop();
        scale += 1;
    }
    if digits.len() > 19 {
        return None;
    }
    let base: u128 = digits.parse().ok()?;
    if scale >= 0 {
        if i64::try_from(digits.len()).ok()? + scale > 19 {
            return None;
        }
        let magnitude = base * 10u128.pow(u32::try_from(scale).ok()?);
        return to_i64(negative, magnitude).map(NumberClass::Long);
    }
    let fraction_digits = usize::try_from(-scale).ok()?;
    if fraction_digits > 4 {
        return None;
    }
    // Cedar `decimal` holds value * 10^4 in an i64.
    let scaled = base * 10u128.pow(u32::try_from(4 - fraction_digits).ok()?);
    to_i64(negative, scaled)?;
    let (int_digits, frac_digits) = if digits.len() <= fraction_digits {
        ("0".to_owned(), format!("{}{digits}", "0".repeat(fraction_digits - digits.len())))
    } else {
        let (i, f) = digits.split_at(digits.len() - fraction_digits);
        (i.to_owned(), f.to_owned())
    };
    let sign = if negative { "-" } else { "" };
    Some(NumberClass::Decimal(format!("{sign}{int_digits}.{frac_digits}")))
}

/// The arguments are not one strict JSON object.
#[cfg(feature = "tool-policy")]
pub(crate) struct Unparsed;

/// The members of one JSON object, in order. A duplicate key is an error.
#[cfg(feature = "tool-policy")]
struct Entries<'a>(Vec<(String, &'a RawValue)>);

#[cfg(feature = "tool-policy")]
impl<'de> Deserialize<'de> for Entries<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Entries<'de>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object with unique keys")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut seen = HashSet::new();
                let mut out = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(A::Error::custom("duplicate key"));
                    }
                    out.push((key, map.next_value::<&'de RawValue>()?));
                }
                Ok(Entries(out))
            }
        }
        deserializer.deserialize_map(V)
    }
}

#[cfg(feature = "tool-policy")]
pub(crate) fn first_byte(text: &str) -> u8 {
    text.as_bytes().first().copied().unwrap_or(0)
}

#[cfg(feature = "tool-policy")]
pub(crate) fn strict_entries(text: &str) -> Result<Vec<(String, &RawValue)>, Unparsed> {
    serde_json::from_str::<Entries<'_>>(text).map(|e| e.0).map_err(|_| Unparsed)
}

#[cfg(feature = "tool-policy")]
pub(crate) fn elements(text: &str) -> Result<Vec<&RawValue>, Unparsed> {
    serde_json::from_str::<Vec<&RawValue>>(text).map_err(|_| Unparsed)
}

/// Whether the brackets of one valid JSON text nest at most `MAX_NESTING` levels. Iterative, so a
/// deep text cannot overflow the stack. A bracket inside a string does not count.
pub(crate) fn nesting_ok(text: &str) -> bool {
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    for b in text.bytes() {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > MAX_NESTING {
                    return false;
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_in_any_spelling_are_long() {
        for t in ["5", "5.0", "5e0", "0.5e1", "50e-1"] {
            assert_eq!(classify_number(t), NumberClass::Long(5), "{t}");
        }
        assert_eq!(classify_number("-0"), NumberClass::Long(0));
        assert_eq!(classify_number("9223372036854775807"), NumberClass::Long(i64::MAX));
        assert_eq!(classify_number("-9223372036854775808"), NumberClass::Long(i64::MIN));
        assert_eq!(classify_number("9223372036854775808"), NumberClass::Text("9223372036854775808".into()));
    }

    #[test]
    fn decimal_edges() {
        assert_eq!(classify_number("1.5"), NumberClass::Decimal("1.5".into()));
        assert_eq!(classify_number("0.1234"), NumberClass::Decimal("0.1234".into()));
        assert_eq!(classify_number("0.12345"), NumberClass::Text("0.12345".into()));
        assert_eq!(classify_number("922337203685477.5807"), NumberClass::Decimal("922337203685477.5807".into()));
        assert_eq!(classify_number("922337203685477.5808"), NumberClass::Text("922337203685477.5808".into()));
        assert_eq!(classify_number("-922337203685477.5808"), NumberClass::Decimal("-922337203685477.5808".into()));
        assert_eq!(classify_number("-0.5"), NumberClass::Decimal("-0.5".into()));
        assert_eq!(classify_number("5e-3"), NumberClass::Decimal("0.005".into()));
        assert_eq!(classify_number("125e-1"), NumberClass::Decimal("12.5".into()));
    }

    #[test]
    fn a_big_integer_keeps_its_text() {
        assert_eq!(classify_number("12345678901234567890"), NumberClass::Text("12345678901234567890".into()));
        assert_eq!(classify_number("1e400"), NumberClass::Text("1e400".into()));
        assert_eq!(classify_number("1e99999999999999999999"), NumberClass::Text("1e99999999999999999999".into()));
        assert_eq!(classify_number("0e999999999"), NumberClass::Long(0));
        assert_eq!(classify_number("0.0e-99999999999999999999"), NumberClass::Long(0));
    }

    #[test]
    fn an_inflated_fraction_or_exponent_does_not_change_the_value() {
        let long_fraction = format!("0.{}1e2000000", "0".repeat(1_000_000));
        assert_eq!(classify_number(&long_fraction), NumberClass::Text(long_fraction.clone()));
        let long_integer = format!("1{}e-2000000", "0".repeat(1_000_001));
        assert_eq!(classify_number(&long_integer), NumberClass::Text(long_integer.clone()));
        let huge_exponent = "1e999999999999999999999999";
        assert_eq!(classify_number(huge_exponent), NumberClass::Text(huge_exponent.into()));
        let tiny_exponent = "1e-999999999999999999999999";
        assert_eq!(classify_number(tiny_exponent), NumberClass::Text(tiny_exponent.into()));
    }
}
