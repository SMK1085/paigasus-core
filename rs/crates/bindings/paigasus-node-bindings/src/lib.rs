// SPDX-License-Identifier: Apache-2.0

//! napi-rs binding shim for `paigasus-kernel` (ADR-0005): exposes the kernel's pure functions
//! to Node/TypeScript. Compiled as a cdylib that `@napi-rs/cli` post-processes into a `.node`
//! addon. The affected-graph cascade `paigasus-kernel-rs → paigasus-node-bindings-rs` is proven
//! by this crate compiling against a real `paigasus_kernel::*` call (SMA-420).

use napi_derive::napi;

use paigasus_kernel::{Prn, PrnError, to_cedar_uid};

/// Node-callable wrapper over [`paigasus_kernel::sum`] (the canonical first-binding shape — a
/// real value crossing the FFI boundary). Uses `i32` so napi-rs maps the surface to a JS
/// `number` deterministically (spec decision #5 / review F3): an `i64` return can surface as a
/// `BigInt` on some napi-rs versions (`5n !== 5`). The kernel fn is `i64`; we cast at the
/// boundary. A future kernel fn needing the full `i64` range gets explicit BigInt handling then.
#[napi]
pub fn sum(a: i32, b: i32) -> i32 {
    paigasus_kernel::sum(a as i64, b as i64) as i32
}

/// Map a [`PrnError`] to a `napi::Error` carrying the stable `kind()` token.
fn to_napi(e: PrnError) -> napi::Error {
    napi::Error::from_reason(e.kind())
}

/// Parse a 20-char lowercase hex string into 10 raw bytes, or return `napi::Error("bad-rand-hex")`.
fn parse_rand_hex(rand_hex: &str) -> napi::Result<[u8; 10]> {
    if rand_hex.len() != 20 || !rand_hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(napi::Error::from_reason("bad-rand-hex"));
    }
    let mut out = [0u8; 10];
    for i in 0..10 {
        out[i] = u8::from_str_radix(&rand_hex[i * 2..i * 2 + 2], 16).map_err(|_| napi::Error::from_reason("bad-rand-hex"))?;
    }
    Ok(out)
}

/// Parse `s` and return its canonical form, or throw `kind()` on an invalid PRN.
#[napi(js_name = "prnCanonicalize")]
pub fn prn_canonicalize(s: String) -> napi::Result<String> {
    Prn::parse(&s).map(|p| p.canonical()).map_err(to_napi)
}

/// Return the stable `PrnError::kind()` token for an invalid PRN, or `""` if `s` parses.
#[napi(js_name = "prnErrorKind")]
pub fn prn_error_kind(s: String) -> String {
    Prn::parse(&s).map_or_else(|e| e.kind().to_string(), |_| String::new())
}

/// Build a PRN from typed fields and return its canonical form, or throw `kind()`.
#[napi(js_name = "prnBuild")]
pub fn prn_build(service: String, region: String, org: String, resource_type: String, resource_id: String) -> napi::Result<String> {
    let candidate = format!("prn:pgs:{service}:{region}:{org}:{resource_type}/{resource_id}");
    Prn::parse(&candidate).map(|p| p.canonical()).map_err(to_napi)
}

/// Parse `s` and return its service field, or throw `kind()`.
#[napi(js_name = "prnService")]
pub fn prn_service(s: String) -> napi::Result<String> {
    Prn::parse(&s).map(|p| p.service().to_string()).map_err(to_napi)
}

/// Parse `s` and return its region field, or throw `kind()`.
#[napi(js_name = "prnRegion")]
pub fn prn_region(s: String) -> napi::Result<String> {
    Prn::parse(&s).map(|p| p.region().to_string()).map_err(to_napi)
}

/// Parse `s` and return its org field (hyphenated UUID, or `""` if absent), or throw `kind()`.
#[napi(js_name = "prnOrg")]
pub fn prn_org(s: String) -> napi::Result<String> {
    Prn::parse(&s).map(|p| p.org().map(|u| u.as_hyphenated().to_string()).unwrap_or_default()).map_err(to_napi)
}

/// Parse `s` and return its resource-type field, or throw `kind()`.
#[napi(js_name = "prnResourceType")]
pub fn prn_resource_type(s: String) -> napi::Result<String> {
    Prn::parse(&s).map(|p| p.resource_type().to_string()).map_err(to_napi)
}

/// Parse `s` and return its resource-id field (hyphenated UUID), or throw `kind()`.
#[napi(js_name = "prnResourceId")]
pub fn prn_resource_id(s: String) -> napi::Result<String> {
    Prn::parse(&s).map(|p| p.resource_id().as_hyphenated().to_string()).map_err(to_napi)
}

/// Validate a millisecond timestamp before the `as u64` cast, or return `napi::Error("bad-unix-ms")`
/// (a bare cast would silently coerce NaN→0, +Inf→`u64::MAX`, negative→0, fractional→truncated, and
/// any finite value ≥ `u64::MAX` saturated to `u64::MAX`).
fn checked_unix_ms(unix_ms: f64) -> napi::Result<u64> {
    if !unix_ms.is_finite() || unix_ms < 0.0 || unix_ms.fract() != 0.0 || unix_ms >= u64::MAX as f64 {
        return Err(napi::Error::from_reason("bad-unix-ms"));
    }
    Ok(unix_ms as u64)
}

/// Mint a UUIDv7 from an injected millisecond timestamp and a 20-char lowercase hex string of
/// entropy (throws `"bad-rand-hex"` if `rand_hex` is malformed).
#[napi(js_name = "mintUuid7")]
pub fn mint_uuid7(unix_ms: f64, rand_hex: String) -> napi::Result<String> {
    let rand = parse_rand_hex(&rand_hex)?;
    let ms = checked_unix_ms(unix_ms)?;
    Ok(paigasus_kernel::mint_uuid7(ms, rand).as_hyphenated().to_string())
}

/// Parse `s` and return its Cedar entity type (e.g. `Pgs::Iam::Project`), or throw `kind()`.
#[napi(js_name = "prnCedarEntityType")]
pub fn prn_cedar_entity_type(s: String) -> napi::Result<String> {
    Prn::parse(&s).map(|p| to_cedar_uid(&p).entity_type).map_err(to_napi)
}

/// Parse `s` and return its Cedar entity id, or throw `kind()`.
#[napi(js_name = "prnCedarEntityId")]
pub fn prn_cedar_entity_id(s: String) -> napi::Result<String> {
    Prn::parse(&s).map(|p| to_cedar_uid(&p).entity_id).map_err(to_napi)
}
