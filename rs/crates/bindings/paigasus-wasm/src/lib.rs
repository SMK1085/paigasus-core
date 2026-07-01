// SPDX-License-Identifier: Apache-2.0

//! wasm-bindgen binding shim for `paigasus-kernel` (ADR-0005): exposes the kernel's pure functions
//! to browsers/Edge. Compiled to `wasm32-unknown-unknown` and post-processed by wasm-pack
//! (`--target bundler`) into a `.wasm` + JS glue. The affected-graph cascade
//! `paigasus-kernel-rs → paigasus-wasm-rs` is proven by this crate compiling against a real
//! `paigasus_kernel::*` call (SMA-427).

use wasm_bindgen::prelude::{JsError, wasm_bindgen};

use paigasus_kernel::{Prn, to_cedar_uid};

/// Browser-callable wrapper over [`paigasus_kernel::sum`]. Uses `i32` at the FFI boundary so the
/// JS surface is a plain `number` (matching the napi binding); the kernel fn is `i64`, cast at the
/// boundary. A future kernel fn needing the full `i64` range gets explicit handling then (shared
/// across all bindings — SMA-427 L5).
#[wasm_bindgen]
pub fn sum(a: i32, b: i32) -> i32 {
    paigasus_kernel::sum(a as i64, b as i64) as i32
}

/// Parse a 20-char lowercase hex string into 10 raw bytes, or return `JsError("bad-rand-hex")`.
fn parse_rand_hex(rand_hex: &str) -> Result<[u8; 10], JsError> {
    if rand_hex.len() != 20 || !rand_hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(JsError::new("bad-rand-hex"));
    }
    let mut out = [0u8; 10];
    for i in 0..10 {
        out[i] = u8::from_str_radix(&rand_hex[i * 2..i * 2 + 2], 16).map_err(|_| JsError::new("bad-rand-hex"))?;
    }
    Ok(out)
}

/// Parse `s` and return its canonical form, or throw `kind()` on an invalid PRN.
#[wasm_bindgen(js_name = prnCanonicalize)]
pub fn prn_canonicalize(s: String) -> Result<String, JsError> {
    Prn::parse(&s).map(|p| p.canonical()).map_err(|e| JsError::new(e.kind()))
}

/// Return the stable `PrnError::kind()` token for an invalid PRN, or `""` if `s` parses.
#[wasm_bindgen(js_name = prnErrorKind)]
pub fn prn_error_kind(s: String) -> String {
    Prn::parse(&s).map_or_else(|e| e.kind().to_string(), |_| String::new())
}

/// Build a PRN from typed fields and return its canonical form, or throw `kind()`.
#[wasm_bindgen(js_name = prnBuild)]
pub fn prn_build(service: String, region: String, org: String, resource_type: String, resource_id: String) -> Result<String, JsError> {
    let candidate = format!("prn:pgs:{service}:{region}:{org}:{resource_type}/{resource_id}");
    Prn::parse(&candidate).map(|p| p.canonical()).map_err(|e| JsError::new(e.kind()))
}

/// Parse `s` and return its service field, or throw `kind()`.
#[wasm_bindgen(js_name = prnService)]
pub fn prn_service(s: String) -> Result<String, JsError> {
    Prn::parse(&s).map(|p| p.service().to_string()).map_err(|e| JsError::new(e.kind()))
}

/// Parse `s` and return its region field, or throw `kind()`.
#[wasm_bindgen(js_name = prnRegion)]
pub fn prn_region(s: String) -> Result<String, JsError> {
    Prn::parse(&s).map(|p| p.region().to_string()).map_err(|e| JsError::new(e.kind()))
}

/// Parse `s` and return its org field (hyphenated UUID, or `""` if absent), or throw `kind()`.
#[wasm_bindgen(js_name = prnOrg)]
pub fn prn_org(s: String) -> Result<String, JsError> {
    Prn::parse(&s)
        .map(|p| p.org().map(|u| u.as_hyphenated().to_string()).unwrap_or_default())
        .map_err(|e| JsError::new(e.kind()))
}

/// Parse `s` and return its resource-type field, or throw `kind()`.
#[wasm_bindgen(js_name = prnResourceType)]
pub fn prn_resource_type(s: String) -> Result<String, JsError> {
    Prn::parse(&s).map(|p| p.resource_type().to_string()).map_err(|e| JsError::new(e.kind()))
}

/// Parse `s` and return its resource-id field (hyphenated UUID), or throw `kind()`.
#[wasm_bindgen(js_name = prnResourceId)]
pub fn prn_resource_id(s: String) -> Result<String, JsError> {
    Prn::parse(&s).map(|p| p.resource_id().as_hyphenated().to_string()).map_err(|e| JsError::new(e.kind()))
}

/// Validate a millisecond timestamp before the `as u64` cast, or return `JsError("bad-unix-ms")`
/// (a bare cast would silently coerce NaN→0, +Inf→`u64::MAX`, negative→0, fractional→truncated, and
/// any finite value ≥ `u64::MAX` saturated to `u64::MAX`).
fn checked_unix_ms(unix_ms: f64) -> Result<u64, JsError> {
    if !unix_ms.is_finite() || unix_ms < 0.0 || unix_ms.fract() != 0.0 || unix_ms >= u64::MAX as f64 {
        return Err(JsError::new("bad-unix-ms"));
    }
    Ok(unix_ms as u64)
}

/// Mint a UUIDv7 from an injected millisecond timestamp and a 20-char lowercase hex string of
/// entropy (throws `"bad-rand-hex"` if `rand_hex` is malformed).
#[wasm_bindgen(js_name = mintUuid7)]
pub fn mint_uuid7(unix_ms: f64, rand_hex: String) -> Result<String, JsError> {
    let rand = parse_rand_hex(&rand_hex)?;
    let ms = checked_unix_ms(unix_ms)?;
    Ok(paigasus_kernel::mint_uuid7(ms, rand).as_hyphenated().to_string())
}

/// Parse `s` and return its Cedar entity type (e.g. `Pgs::Iam::Project`), or throw `kind()`.
#[wasm_bindgen(js_name = prnCedarEntityType)]
pub fn prn_cedar_entity_type(s: String) -> Result<String, JsError> {
    Prn::parse(&s).map(|p| to_cedar_uid(&p).entity_type).map_err(|e| JsError::new(e.kind()))
}

/// Parse `s` and return its Cedar entity id, or throw `kind()`.
#[wasm_bindgen(js_name = prnCedarEntityId)]
pub fn prn_cedar_entity_id(s: String) -> Result<String, JsError> {
    Prn::parse(&s).map(|p| to_cedar_uid(&p).entity_id).map_err(|e| JsError::new(e.kind()))
}
