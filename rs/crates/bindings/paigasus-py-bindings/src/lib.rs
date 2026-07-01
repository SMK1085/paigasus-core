// SPDX-License-Identifier: Apache-2.0

//! PyO3 binding shim for `paigasus-kernel` (ADR-0005): exposes the kernel's pure functions
//! to Python. Compiled as an `abi3` `extension-module` cdylib; packaging into a wheel and
//! wiring it into the uv workspace are a later issue. The affected-graph cascade
//! `paigasus-kernel-rs → paigasus-py-bindings-rs` is proven by this crate compiling against a
//! real `paigasus_kernel::*` call (SMA-409).

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use paigasus_kernel::{Prn, PrnError, to_cedar_uid};

/// Python-callable wrapper over [`paigasus_kernel::sum`], returning the result as a string
/// (the canonical PyO3 first-binding shape — a real value crossing the FFI boundary).
#[pyfunction]
fn sum_as_string(a: i64, b: i64) -> String {
    paigasus_kernel::sum(a, b).to_string()
}

/// Map a [`PrnError`] to a Python `ValueError` carrying the stable `kind()` token.
fn map_prn_err(e: PrnError) -> PyErr {
    PyValueError::new_err(e.kind())
}

/// Parse a 20-char lowercase hex string into 10 raw bytes, or raise `ValueError("bad-rand-hex")`.
fn parse_rand_hex(rand_hex: &str) -> PyResult<[u8; 10]> {
    if rand_hex.len() != 20 || !rand_hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(PyValueError::new_err("bad-rand-hex"));
    }
    let mut out = [0u8; 10];
    for i in 0..10 {
        out[i] = u8::from_str_radix(&rand_hex[i * 2..i * 2 + 2], 16).map_err(|_| PyValueError::new_err("bad-rand-hex"))?;
    }
    Ok(out)
}

/// Parse `s` and return its canonical form, or raise `ValueError(kind)` on an invalid PRN.
#[pyfunction]
fn prn_canonicalize(s: &str) -> PyResult<String> {
    Prn::parse(s).map(|p| p.canonical()).map_err(map_prn_err)
}

/// Return the stable `PrnError::kind()` token for an invalid PRN, or `""` if `s` parses.
#[pyfunction]
fn prn_error_kind(s: &str) -> String {
    Prn::parse(s).map_or_else(|e| e.kind().to_string(), |_| String::new())
}

/// Build a PRN from typed fields and return its canonical form, or raise `ValueError(kind)`.
#[pyfunction]
fn prn_build(service: &str, region: &str, org: &str, resource_type: &str, resource_id: &str) -> PyResult<String> {
    let candidate = format!("prn:pgs:{service}:{region}:{org}:{resource_type}/{resource_id}");
    Prn::parse(&candidate).map(|p| p.canonical()).map_err(map_prn_err)
}

/// Parse `s` and return its service field, or raise `ValueError(kind)`.
#[pyfunction]
fn prn_service(s: &str) -> PyResult<String> {
    Prn::parse(s).map(|p| p.service().to_string()).map_err(map_prn_err)
}

/// Parse `s` and return its region field, or raise `ValueError(kind)`.
#[pyfunction]
fn prn_region(s: &str) -> PyResult<String> {
    Prn::parse(s).map(|p| p.region().to_string()).map_err(map_prn_err)
}

/// Parse `s` and return its org field (hyphenated UUID, or `""` if absent), or raise `ValueError(kind)`.
#[pyfunction]
fn prn_org(s: &str) -> PyResult<String> {
    Prn::parse(s).map(|p| p.org().map(|u| u.as_hyphenated().to_string()).unwrap_or_default()).map_err(map_prn_err)
}

/// Parse `s` and return its resource-type field, or raise `ValueError(kind)`.
#[pyfunction]
fn prn_resource_type(s: &str) -> PyResult<String> {
    Prn::parse(s).map(|p| p.resource_type().to_string()).map_err(map_prn_err)
}

/// Parse `s` and return its resource-id field (hyphenated UUID), or raise `ValueError(kind)`.
#[pyfunction]
fn prn_resource_id(s: &str) -> PyResult<String> {
    Prn::parse(s).map(|p| p.resource_id().as_hyphenated().to_string()).map_err(map_prn_err)
}

/// Mint a UUIDv7 from an injected millisecond timestamp and a 20-char lowercase hex string of
/// entropy (raises `ValueError("bad-rand-hex")` if `rand_hex` is malformed).
#[pyfunction]
fn mint_uuid7(unix_ms: f64, rand_hex: &str) -> PyResult<String> {
    let rand = parse_rand_hex(rand_hex)?;
    Ok(paigasus_kernel::mint_uuid7(unix_ms as u64, rand).as_hyphenated().to_string())
}

/// Parse `s` and return its Cedar entity type (e.g. `Pgs::Iam::Project`), or raise `ValueError(kind)`.
#[pyfunction]
fn prn_cedar_entity_type(s: &str) -> PyResult<String> {
    Prn::parse(s).map(|p| to_cedar_uid(&p).entity_type).map_err(map_prn_err)
}

/// Parse `s` and return its Cedar entity id, or raise `ValueError(kind)`.
#[pyfunction]
fn prn_cedar_entity_id(s: &str) -> PyResult<String> {
    Prn::parse(s).map(|p| to_cedar_uid(&p).entity_id).map_err(map_prn_err)
}

/// The extension module. Its name is provisional — it will be reconciled with the
/// `paigasus-kernel-py` wrapper when the wheel-integration issue lands.
#[pymodule]
fn paigasus_py_bindings(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(sum_as_string, m)?)?;
    m.add_function(wrap_pyfunction!(prn_canonicalize, m)?)?;
    m.add_function(wrap_pyfunction!(prn_error_kind, m)?)?;
    m.add_function(wrap_pyfunction!(prn_build, m)?)?;
    m.add_function(wrap_pyfunction!(prn_service, m)?)?;
    m.add_function(wrap_pyfunction!(prn_region, m)?)?;
    m.add_function(wrap_pyfunction!(prn_org, m)?)?;
    m.add_function(wrap_pyfunction!(prn_resource_type, m)?)?;
    m.add_function(wrap_pyfunction!(prn_resource_id, m)?)?;
    m.add_function(wrap_pyfunction!(mint_uuid7, m)?)?;
    m.add_function(wrap_pyfunction!(prn_cedar_entity_type, m)?)?;
    m.add_function(wrap_pyfunction!(prn_cedar_entity_id, m)?)?;
    Ok(())
}
