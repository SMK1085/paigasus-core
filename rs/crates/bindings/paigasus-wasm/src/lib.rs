// SPDX-License-Identifier: Apache-2.0

//! wasm-bindgen binding shim for `paigasus-kernel` (ADR-0005): exposes the kernel's pure functions
//! to browsers/Edge. Compiled to `wasm32-unknown-unknown` and post-processed by wasm-pack
//! (`--target bundler`) into a `.wasm` + JS glue. The affected-graph cascade
//! `paigasus-kernel-rs → paigasus-wasm-rs` is proven by this crate compiling against a real
//! `paigasus_kernel::*` call (SMA-427).

use wasm_bindgen::prelude::wasm_bindgen;

/// Browser-callable wrapper over [`paigasus_kernel::sum`]. Uses `i32` at the FFI boundary so the
/// JS surface is a plain `number` (matching the napi binding); the kernel fn is `i64`, cast at the
/// boundary. A future kernel fn needing the full `i64` range gets explicit handling then (shared
/// across all bindings — SMA-427 L5).
#[wasm_bindgen]
pub fn sum(a: i32, b: i32) -> i32 {
    paigasus_kernel::sum(a as i64, b as i64) as i32
}
