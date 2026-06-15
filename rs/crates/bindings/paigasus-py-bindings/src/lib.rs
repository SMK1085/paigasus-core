// SPDX-License-Identifier: Apache-2.0

//! PyO3 binding shim for `paigasus-kernel` (ADR-0005): exposes the kernel's pure functions
//! to Python. Compiled as an `abi3` `extension-module` cdylib; packaging into a wheel and
//! wiring it into the uv workspace are a later issue. The affected-graph cascade
//! `paigasus-kernel-rs → paigasus-py-bindings-rs` is proven by this crate compiling against a
//! real `paigasus_kernel::*` call (SMA-409).

use pyo3::prelude::*;

/// Python-callable wrapper over [`paigasus_kernel::sum`], returning the result as a string
/// (the canonical PyO3 first-binding shape — a real value crossing the FFI boundary).
#[pyfunction]
fn sum_as_string(a: i64, b: i64) -> String {
    paigasus_kernel::sum(a, b).to_string()
}

/// The extension module. Its name is provisional — it will be reconciled with the
/// `paigasus-kernel-py` wrapper when the wheel-integration issue lands.
#[pymodule]
fn paigasus_py_bindings(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(sum_as_string, m)?)?;
    Ok(())
}
