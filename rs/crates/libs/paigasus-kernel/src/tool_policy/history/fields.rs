// SPDX-License-Identifier: Apache-2.0
//! Field extraction for the session history (SMA-753 P3).
//!
//! `extract_fields` uses the same parser as `map_input` (`parse_arguments`), so both read one value
//! from the same arguments. The types that history can compare are a separate, smaller set: a
//! `Long`, a string, a bool and a decimal with at most four places. Every other value is `Mistyped`
//! in history, also when `context.input` holds it. For example, an integer outside the `Long` range
//! is a Cedar string in `context.input`, but `Mistyped` in history. A record, a set and a decimal
//! with more places are `Mistyped` too. So a value that a policy can compare in `context.input`
//! cannot always be compared in a history condition.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use uuid::Uuid;

use crate::tool_policy::ToolInput;
use crate::tool_policy::walk::{Arg, parse_arguments};

/// A path is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathError {
    /// The path has no segment.
    Empty,
    /// A segment is empty.
    EmptySegment,
    /// A segment holds a `.`.
    DottedSegment,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "a field path has no segment",
            Self::EmptySegment => "a field path segment is empty",
            Self::DottedSegment => "a field path segment holds a dot",
        })
    }
}

impl std::error::Error for PathError {}

/// A path of record keys into the arguments of a call.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldPath(Vec<String>);

impl FieldPath {
    /// Make a path. It needs one segment or more, and no segment is empty or holds a `.`.
    pub fn new(segments: Vec<String>) -> Result<Self, PathError> {
        if segments.is_empty() {
            return Err(PathError::Empty);
        }
        for s in &segments {
            if s.is_empty() {
                return Err(PathError::EmptySegment);
            }
            if s.contains('.') {
                return Err(PathError::DottedSegment);
            }
        }
        Ok(Self(segments))
    }

    /// The segments of the path.
    pub fn segments(&self) -> &[String] {
        &self.0
    }

    /// The segments joined with `.`.
    pub fn joined(&self) -> String {
        self.0.join(".")
    }
}

/// The id of a digest key: the first 8 bytes of `HMAC-SHA-256(key, "paigasus-history-kid")`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyId(pub [u8; 8]);

impl KeyId {
    pub fn new(bytes: [u8; 8]) -> Self {
        Self(bytes)
    }

    /// Lower-case hex, 16 characters.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The inverse of `to_hex`. Only 16 lower-case hex characters are accepted.
    pub fn from_hex(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 16 || !bytes.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let mut out = [0u8; 8];
        for (slot, pair) in out.iter_mut().zip(bytes.chunks(2)) {
            *slot = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        }
        Some(Self(out))
    }
}

/// A keyed digest of one value, cut to 16 bytes.
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Digest(pub [u8; 16]);

impl Digest {
    pub fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Digest(..)")
    }
}

/// A value that the history stores as a digest.
#[non_exhaustive]
#[derive(Clone, PartialEq, Eq)]
pub enum Scalar {
    Str(String),
    Bool(bool),
    /// The decimal value times 10,000.
    Decimal(i64),
}

impl Scalar {
    /// The tag and the value, as the digest input uses them (spec P3).
    pub fn canonical_bytes(&self) -> Vec<u8> {
        match self {
            Self::Str(s) => [b"s".as_slice(), s.as_bytes()].concat(),
            Self::Bool(true) => b"b1".to_vec(),
            Self::Bool(false) => b"b0".to_vec(),
            Self::Decimal(n) => [b"d".as_slice(), &n.to_be_bytes()].concat(),
        }
    }
}

impl fmt::Debug for Scalar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Str(_) => "Scalar::Str(..)",
            Self::Bool(_) => "Scalar::Bool(..)",
            Self::Decimal(_) => "Scalar::Decimal(..)",
        })
    }
}

/// The paths that a policy reads.
pub type PathSet = BTreeSet<FieldPath>;

/// The value at one path, before the digest.
#[non_exhaustive]
#[derive(Clone, PartialEq, Eq)]
pub enum Extracted {
    Long(i64),
    Scalar(Scalar),
    Absent,
    Mistyped,
}

impl fmt::Debug for Extracted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Long(_) => f.write_str("Extracted::Long(..)"),
            Self::Scalar(s) => write!(f, "Extracted::{s:?}"),
            Self::Absent => f.write_str("Extracted::Absent"),
            Self::Mistyped => f.write_str("Extracted::Mistyped"),
        }
    }
}

/// The values at the planned paths, or no values when the arguments are unparsed.
#[non_exhaustive]
#[derive(Clone, PartialEq, Eq)]
pub struct ExtractedFields(Option<BTreeMap<FieldPath, Extracted>>);

impl fmt::Debug for ExtractedFields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtractedFields")
            .field("paths", &self.0.as_ref().map_or(0, BTreeMap::len))
            .field("unparsed", &self.0.is_none())
            .finish()
    }
}

/// The value of one path, as the history stores it. `Debug` prints the kind only, never the value.
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Long(i64),
    Digest(Digest),
    Absent,
    Mistyped,
}

impl fmt::Debug for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Long(_) => "Field::Long(..)",
            Self::Digest(_) => "Field::Digest(..)",
            Self::Absent => "Field::Absent",
            Self::Mistyped => "Field::Mistyped",
        })
    }
}

/// The stored fields of one call.
#[non_exhaustive]
#[derive(Clone, PartialEq, Eq)]
pub enum Fields {
    Unparsed,
    Values(BTreeMap<FieldPath, Field>),
}

impl fmt::Debug for Fields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fields")
            .field(
                "paths",
                &match self {
                    Self::Unparsed => 0,
                    Self::Values(m) => m.len(),
                },
            )
            .field("unparsed", &matches!(self, Self::Unparsed))
            .finish()
    }
}

/// The keyed digest of a value. The gateway implements it. The kernel has no hash dependency.
pub trait HistoryDigester: Send + Sync {
    /// The digest under the current key, or `None` when the gateway has no key.
    fn current(&self, org: Uuid, path: &FieldPath, value: &Scalar) -> Option<(KeyId, Digest)>;
    /// The digests under the current key and the previous key (a literal), current first.
    fn all(&self, org: Uuid, path: &FieldPath, value: &Scalar) -> Vec<(KeyId, Digest)>;
}

impl ExtractedFields {
    /// The arguments are not one strict JSON object.
    pub fn is_unparsed(&self) -> bool {
        self.0.is_none()
    }

    /// The value at a planned path. `None` for an unparsed call or an unplanned path.
    pub fn get(&self, path: &FieldPath) -> Option<&Extracted> {
        self.0.as_ref()?.get(path)
    }

    /// Digest the scalars. With no key, a scalar is stored as `Mistyped` (plan decision D-B).
    /// The key id is `Some` only when at least one digest was made.
    pub fn into_fields(self, org: Uuid, digester: &dyn HistoryDigester) -> (Fields, Option<KeyId>) {
        let Some(map) = self.0 else {
            return (Fields::Unparsed, None);
        };
        let mut kid = None;
        let mut out = BTreeMap::new();
        for (path, value) in map {
            let field = match value {
                Extracted::Long(n) => Field::Long(n),
                Extracted::Absent => Field::Absent,
                Extracted::Mistyped => Field::Mistyped,
                Extracted::Scalar(s) => match digester.current(org, &path, &s) {
                    Some((id, digest)) => {
                        kid = Some(id);
                        Field::Digest(digest)
                    }
                    None => Field::Mistyped,
                },
            };
            out.insert(path, field);
        }
        (Fields::Values(out), kid)
    }
}

/// A Cedar decimal literal times 10,000: an optional `-`, ASCII digits, `.`, then 1 to 4 fraction
/// digits. The scaled value must fit `i64`. This is the one scaling path of both sides of an
/// equality test: the literal of a condition and the value of a call argument (spec P3).
pub fn scale_decimal(text: &str) -> Option<i64> {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, text),
    };
    let (int_part, frac_part) = rest.split_once('.')?;
    let digits_only = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits_only(int_part) || !digits_only(frac_part) || frac_part.len() > 4 {
        return None;
    }
    let magnitude: i128 = format!("{int_part}{frac_part:0<4}").parse().ok()?;
    i64::try_from(if negative { -magnitude } else { magnitude }).ok()
}

fn classify(arg: &Arg) -> Extracted {
    match arg {
        Arg::Long(n) => Extracted::Long(*n),
        Arg::Str(s) | Arg::RawText(s) => Extracted::Scalar(Scalar::Str(s.clone())),
        Arg::Bool(b) => Extracted::Scalar(Scalar::Bool(*b)),
        Arg::Decimal(d) => scale_decimal(d).map_or(Extracted::Mistyped, |n| Extracted::Scalar(Scalar::Decimal(n))),
        Arg::Record(_) | Arg::Set(_) | Arg::TextNumber(_) => Extracted::Mistyped,
    }
}

fn lookup(root: &Arg, path: &FieldPath) -> Extracted {
    let mut current = root;
    for segment in path.segments() {
        match current {
            Arg::Record(fields) => match fields.iter().find(|(k, _)| k == segment) {
                Some((_, next)) => current = next,
                None => return Extracted::Absent,
            },
            _ => return Extracted::Absent,
        }
    }
    classify(current)
}

/// Read the planned paths from the arguments of one call.
pub fn extract_fields(input: &ToolInput, plan: &PathSet) -> ExtractedFields {
    let parsed = parse_arguments(input);
    match parsed.args {
        Some(root) if !parsed.unparsed => ExtractedFields(Some(plan.iter().map(|p| (p.clone(), lookup(&root, p))).collect())),
        _ => ExtractedFields(None),
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::tool_policy::ToolInput;

    /// Digest = the first 16 bytes of `path.joined() + canonical_bytes()`, zero padded.
    struct TestDigester {
        keyed: bool,
    }

    impl HistoryDigester for TestDigester {
        fn current(&self, _org: Uuid, path: &FieldPath, value: &Scalar) -> Option<(KeyId, Digest)> {
            if !self.keyed {
                return None;
            }
            let mut bytes = path.joined().into_bytes();
            bytes.extend(value.canonical_bytes());
            let mut out = [0u8; 16];
            for (slot, b) in out.iter_mut().zip(bytes) {
                *slot = b;
            }
            Some((KeyId([1; 8]), Digest(out)))
        }
        fn all(&self, org: Uuid, path: &FieldPath, value: &Scalar) -> Vec<(KeyId, Digest)> {
            self.current(org, path, value).into_iter().collect()
        }
    }

    fn p(segments: &[&str]) -> FieldPath {
        FieldPath::new(segments.iter().map(|s| (*s).to_owned()).collect()).unwrap()
    }

    fn paths(list: &[&[&str]]) -> PathSet {
        list.iter().map(|s| p(s)).collect()
    }

    #[test]
    fn scale_decimal_follows_the_cedar_grammar() {
        for bad in [
            "1.23456",
            "1",
            "1.",
            ".5",
            "922337203685477.5808",
            "1e2",
            "",
            "-",
            "-.5",
            "+1.5",
            "1.5 ",
            "-922337203685477.5809",
            "\u{661}.\u{665}",
        ] {
            assert_eq!(scale_decimal(bad), None, "text {bad:?}");
        }
        for (text, scaled) in [
            ("0.0", 0),
            ("1.0", 10_000),
            ("-1.25", -12_500),
            ("922337203685477.5807", i64::MAX),
            ("-922337203685477.5808", i64::MIN),
            ("007.5", 75_000),
        ] {
            assert_eq!(scale_decimal(text), Some(scaled), "text {text:?}");
        }
    }

    #[test]
    fn a_literal_and_an_argument_value_of_one_decimal_scale_equally() {
        let e = extract_fields(&ToolInput::Json(r#"{"d":1.5}"#.into()), &paths(&[&["d"]]));
        let Some(Extracted::Scalar(from_argument)) = e.get(&p(&["d"])) else {
            panic!("a decimal argument is a scalar")
        };
        assert_eq!(from_argument, &Scalar::Decimal(scale_decimal("1.5000").unwrap()));
    }

    #[test]
    fn every_value_class_maps_to_its_field() {
        let raw = r#"{"n":5,"s":"x","t":true,"d":1.5,"o":{"k":1},"a":[1],"big":12345678901234567890,"z":null}"#;
        let e = extract_fields(
            &ToolInput::Json(raw.into()),
            &paths(&[&["n"], &["s"], &["t"], &["d"], &["o"], &["a"], &["big"], &["z"], &["missing"], &["o", "k"], &["s", "deeper"]]),
        );
        assert_eq!(e.get(&p(&["n"])), Some(&Extracted::Long(5)));
        assert_eq!(e.get(&p(&["s"])), Some(&Extracted::Scalar(Scalar::Str("x".into()))));
        assert_eq!(e.get(&p(&["t"])), Some(&Extracted::Scalar(Scalar::Bool(true))));
        assert_eq!(e.get(&p(&["d"])), Some(&Extracted::Scalar(Scalar::Decimal(15_000))));
        for m in [&["o"][..], &["a"], &["big"]] {
            assert_eq!(e.get(&p(m)), Some(&Extracted::Mistyped));
        }
        for a in [&["z"][..], &["missing"], &["s", "deeper"]] {
            assert_eq!(e.get(&p(a)), Some(&Extracted::Absent));
        }
        assert_eq!(e.get(&p(&["o", "k"])), Some(&Extracted::Long(1)));
    }

    #[test]
    fn numbers_are_classified_as_context_input_classifies_them() {
        let raw = r#"{"a":5.0,"b":-0.0,"c":1e2,"d":1.5000,"e":1.23456,"f":1e19,"g":-0.5}"#;
        let e = extract_fields(&ToolInput::Json(raw.into()), &paths(&[&["a"], &["b"], &["c"], &["d"], &["e"], &["f"], &["g"]]));
        assert_eq!(e.get(&p(&["a"])), Some(&Extracted::Long(5)));
        assert_eq!(e.get(&p(&["b"])), Some(&Extracted::Long(0)));
        assert_eq!(e.get(&p(&["c"])), Some(&Extracted::Long(100)));
        assert_eq!(e.get(&p(&["d"])), Some(&Extracted::Scalar(Scalar::Decimal(15_000))));
        assert_eq!(e.get(&p(&["e"])), Some(&Extracted::Mistyped));
        assert_eq!(e.get(&p(&["f"])), Some(&Extracted::Mistyped));
        assert_eq!(e.get(&p(&["g"])), Some(&Extracted::Scalar(Scalar::Decimal(-5_000))));
    }

    #[test]
    fn a_value_below_depth_16_is_its_raw_text_string() {
        // The top object and 15 nested "a" records are depth 1 to 16. The member `x` is at depth 17.
        let mut raw = String::from(r#"{"x":1}"#);
        for _ in 0..15 {
            raw = format!(r#"{{"a":{raw}}}"#);
        }
        let mut segs: Vec<&str> = vec!["a"; 15];
        segs.push("x");
        let e = extract_fields(&ToolInput::Json(raw), &paths(&[&segs]));
        assert_eq!(e.get(&p(&segs)), Some(&Extracted::Scalar(Scalar::Str("1".into()))));
    }

    #[test]
    fn unparsed_and_free_text_arguments_have_no_fields() {
        for input in [ToolInput::Json(r#"{"k":1,"k":2}"#.into()), ToolInput::Json("[1]".into()), ToolInput::Text("x".into())] {
            assert!(extract_fields(&input, &paths(&[&["k"]])).is_unparsed());
        }
        let (fields, kid) = extract_fields(&ToolInput::Text("x".into()), &paths(&[&["k"]])).into_fields(Uuid::nil(), &TestDigester { keyed: true });
        assert_eq!(fields, Fields::Unparsed);
        assert_eq!(kid, None);
    }

    #[test]
    fn into_fields_digests_a_scalar_and_keeps_a_long() {
        let e = extract_fields(&ToolInput::Json(r#"{"n":5,"s":"x"}"#.into()), &paths(&[&["n"], &["s"], &["m"]]));
        let (fields, kid) = e.into_fields(Uuid::nil(), &TestDigester { keyed: true });
        let Fields::Values(map) = fields else { panic!("parsed arguments give values") };
        assert_eq!(map.get(&p(&["n"])), Some(&Field::Long(5)));
        assert_eq!(map.get(&p(&["m"])), Some(&Field::Absent));
        let mut expected = [0u8; 16];
        expected[..3].copy_from_slice(b"ssx");
        assert_eq!(map.get(&p(&["s"])), Some(&Field::Digest(Digest(expected))));
        assert_eq!(kid, Some(KeyId([1; 8])));
    }

    #[test]
    fn no_key_stores_a_scalar_as_mistyped_and_no_key_id() {
        let e = extract_fields(&ToolInput::Json(r#"{"n":5,"s":"x"}"#.into()), &paths(&[&["n"], &["s"]]));
        let (fields, kid) = e.into_fields(Uuid::nil(), &TestDigester { keyed: false });
        let Fields::Values(map) = fields else { panic!("parsed arguments give values") };
        assert_eq!(map.get(&p(&["s"])), Some(&Field::Mistyped));
        assert_eq!(map.get(&p(&["n"])), Some(&Field::Long(5)));
        assert_eq!(kid, None);
    }

    #[test]
    fn a_path_with_an_empty_or_dotted_segment_is_refused() {
        assert!(FieldPath::new(vec![]).is_err());
        assert!(FieldPath::new(vec!["".into()]).is_err());
        assert!(FieldPath::new(vec!["a.b".into()]).is_err());
        assert_eq!(p(&["a", "b"]).joined(), "a.b");
        assert_eq!(p(&["a", "b"]).segments(), ["a".to_owned(), "b".to_owned()]);
    }

    #[test]
    fn scalar_canonical_bytes() {
        assert_eq!(Scalar::Str("é".into()).canonical_bytes(), [b"s".as_slice(), "é".as_bytes()].concat());
        assert_eq!(Scalar::Bool(false).canonical_bytes(), b"b0");
        assert_eq!(Scalar::Bool(true).canonical_bytes(), b"b1");
        assert_eq!(Scalar::Decimal(-15_000).canonical_bytes(), [b"d".as_slice(), &(-15_000i64).to_be_bytes()].concat());
    }

    #[test]
    fn debug_prints_no_value() {
        let raw = r#"{"secret_path":"secret-value","n":4242}"#;
        let plan = paths(&[&["secret_path"], &["n"]]);
        let e = extract_fields(&ToolInput::Json(raw.into()), &plan);
        let shown = format!("{e:?}");
        let (fields, _) = e.into_fields(Uuid::nil(), &TestDigester { keyed: true });
        let shown_fields = format!("{fields:?}");
        for text in [&shown, &shown_fields] {
            assert!(!text.contains("secret-value"), "{text}");
            assert!(!text.contains("secret_path"), "{text}");
            assert!(!text.contains("4242"), "{text}");
            assert!(text.contains('2'), "the path count is shown: {text}");
        }
        assert_eq!(format!("{:?}", Digest([9; 16])), "Digest(..)");
    }

    #[test]
    fn field_debug_prints_no_value() {
        assert_eq!(format!("{:?}", Field::Long(4242)), "Field::Long(..)");
        assert_eq!(format!("{:?}", Field::Digest(Digest([9; 16]))), "Field::Digest(..)");
        assert_eq!(format!("{:?}", Field::Absent), "Field::Absent");
        assert_eq!(format!("{:?}", Field::Mistyped), "Field::Mistyped");
    }

    #[test]
    fn key_id_and_digest_build_outside_the_module_with_new() {
        assert_eq!(KeyId::new([7; 8]).0, [7; 8]);
        assert_eq!(Digest::new([9; 16]).0, [9; 16]);
    }

    #[test]
    fn key_id_hex_round_trips_and_refuses_bad_text() {
        let k = KeyId::new([0xab, 0, 1, 2, 3, 4, 5, 0xff]);
        assert_eq!(k.to_hex(), "ab000102030405ff");
        assert_eq!(KeyId::from_hex("ab000102030405ff"), Some(k));
        assert_eq!(KeyId::from_hex("ab"), None);
        assert_eq!(KeyId::from_hex("zz000102030405ff"), None);
        assert_eq!(KeyId::from_hex("AB000102030405FF"), None, "lower case only");
    }
}
