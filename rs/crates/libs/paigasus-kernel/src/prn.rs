// SPDX-License-Identifier: Apache-2.0
//! The `Prn` (Paigasus Resource Name) value type — parse / build / canonicalize / accessors over
//! `prn:pgs:<service>:<region>:<org>:<resource-type>/<resource-id>` (ADR-0014). Pure logic; the
//! kernel validates grammar/syntax only, not tenancy semantics (SMA-448).

use uuid::Uuid;

/// Maximum accepted PRN length, in bytes. Bounds parse cost and downstream storage (ARNs cap for
/// the same reason).
const MAX_LEN: usize = 512;

/// A parsed, validated Paigasus Resource Name. Equality is over the canonical field tuple, which
/// is equivalent to canonical-string equality (UUIDs compare by value regardless of input case).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Prn {
    service: String,
    region: String,
    org: Option<Uuid>,
    resource_type: String,
    resource_id: Uuid,
}

/// A typed PRN parse/validation error. `kind()` returns the stable token used by the FFI
/// `prn_error_kind` surface and the cross-language parity corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PrnError {
    #[error("empty PRN")]
    Empty,
    #[error("PRN exceeds the maximum length")]
    TooLong,
    #[error("bad scheme (expected `prn`)")]
    BadScheme,
    #[error("bad partition (expected `pgs`)")]
    BadPartition,
    #[error("wrong field count (expected 6 colon-separated fields)")]
    WrongFieldCount,
    #[error("bad service segment")]
    BadService,
    #[error("bad region segment")]
    BadRegion,
    #[error("bad org (expected empty or a UUID)")]
    BadOrg,
    #[error("bad resource path (expected `<type>/<id>`)")]
    BadResourcePath,
    #[error("bad resource-type segment")]
    BadResourceType,
    #[error("bad resource-id (expected a UUID)")]
    BadResourceId,
}

impl PrnError {
    /// The stable, lowercase, cross-language error-kind token.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            PrnError::Empty => "empty",
            PrnError::TooLong => "too-long",
            PrnError::BadScheme => "bad-scheme",
            PrnError::BadPartition => "bad-partition",
            PrnError::WrongFieldCount => "wrong-field-count",
            PrnError::BadService => "bad-service",
            PrnError::BadRegion => "bad-region",
            PrnError::BadOrg => "bad-org",
            PrnError::BadResourcePath => "bad-resource-path",
            PrnError::BadResourceType => "bad-resource-type",
            PrnError::BadResourceId => "bad-resource-id",
        }
    }
}

/// `^[a-z][a-z0-9]*(-[a-z][a-z0-9]*)*$` — lowercase; every `-`-separated segment starts with a
/// letter, so there is no leading/trailing/double hyphen AND no digit-only post-hyphen segment
/// (which would collide under the Cedar PascalCase mapping — e.g. `a1` and `a-1` both map to `A1`).
fn is_valid_label(s: &str) -> bool {
    let mut segments = s.split('-');
    match segments.next() {
        Some(first) => {
            let mut chars = first.chars();
            match chars.next() {
                Some(c) if c.is_ascii_lowercase() => {}
                _ => return false,
            }
            if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
                return false;
            }
        }
        None => return false,
    }
    segments.all(|seg| {
        let mut chars = seg.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_lowercase()) && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// `^[a-z0-9]+(-[a-z0-9]+)*$` — like a label but a leading digit is allowed (forward-compat region
/// syntax; v1 only ever mints an empty region).
fn is_valid_region(s: &str) -> bool {
    !s.is_empty() && s.split('-').all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
}

/// Parse a strict 36-char lowercase/uppercase hyphenated UUID (rejecting simple/braced/urn forms).
fn parse_uuid_field(s: &str) -> Option<Uuid> {
    if s.len() != 36 {
        return None;
    }
    Uuid::try_parse(s).ok()
}

impl Prn {
    /// Parse and validate a PRN string. See the module docs for the grammar.
    pub fn parse(s: &str) -> Result<Self, PrnError> {
        if s.is_empty() {
            return Err(PrnError::Empty);
        }
        if s.len() > MAX_LEN {
            return Err(PrnError::TooLong);
        }
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 6 {
            return Err(PrnError::WrongFieldCount);
        }
        if parts[0] != "prn" {
            return Err(PrnError::BadScheme);
        }
        if parts[1] != "pgs" {
            return Err(PrnError::BadPartition);
        }
        if !is_valid_label(parts[2]) {
            return Err(PrnError::BadService);
        }
        if !parts[3].is_empty() && !is_valid_region(parts[3]) {
            return Err(PrnError::BadRegion);
        }
        let org = if parts[4].is_empty() { None } else { Some(parse_uuid_field(parts[4]).ok_or(PrnError::BadOrg)?) };
        let path = parts[5];
        if path.matches('/').count() != 1 {
            return Err(PrnError::BadResourcePath);
        }
        let (rtype, rid) = path.split_once('/').ok_or(PrnError::BadResourcePath)?;
        if !is_valid_label(rtype) {
            return Err(PrnError::BadResourceType);
        }
        let resource_id = parse_uuid_field(rid).ok_or(PrnError::BadResourceId)?;
        Ok(Prn {
            service: parts[2].to_string(),
            region: parts[3].to_string(),
            org,
            resource_type: rtype.to_string(),
            resource_id,
        })
    }

    /// Build a PRN from typed fields. Validates the string fields via [`Prn::parse`] (DRY): an
    /// invalid `service`/`region`/`resource_type` returns the same typed error parse would.
    pub fn build(service: &str, region: &str, org: Option<Uuid>, resource_type: &str, resource_id: Uuid) -> Result<Self, PrnError> {
        let org_s = org.map(|u| u.as_hyphenated().to_string()).unwrap_or_default();
        let candidate = format!("prn:pgs:{service}:{region}:{org_s}:{resource_type}/{}", resource_id.as_hyphenated());
        Prn::parse(&candidate)
    }

    /// The canonical PRN string (lowercase, fixed field count). Basis for equality/cache-keys/signatures.
    #[must_use]
    pub fn canonical(&self) -> String {
        let org = self.org.map(|u| u.as_hyphenated().to_string()).unwrap_or_default();
        format!("prn:pgs:{}:{}:{}:{}/{}", self.service, self.region, org, self.resource_type, self.resource_id.as_hyphenated())
    }

    #[must_use]
    pub fn service(&self) -> &str {
        &self.service
    }
    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }
    #[must_use]
    pub fn org(&self) -> Option<Uuid> {
        self.org
    }
    #[must_use]
    pub fn resource_type(&self) -> &str {
        &self.resource_type
    }
    #[must_use]
    pub fn resource_id(&self) -> Uuid {
        self.resource_id
    }
}
