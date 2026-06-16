// SPDX-License-Identifier: Apache-2.0

//! napi-rs binding shim for `paigasus-kernel` (ADR-0005): exposes the kernel's pure functions
//! to Node/TypeScript. Compiled as a cdylib that `@napi-rs/cli` post-processes into a `.node`
//! addon. The affected-graph cascade `paigasus-kernel-rs → paigasus-node-bindings-rs` is proven
//! by this crate compiling against a real `paigasus_kernel::*` call (SMA-420).

use napi_derive::napi;

/// Node-callable wrapper over [`paigasus_kernel::sum`] (the canonical first-binding shape — a
/// real value crossing the FFI boundary). Uses `i32` so napi-rs maps the surface to a JS
/// `number` deterministically (spec decision #5 / review F3): an `i64` return can surface as a
/// `BigInt` on some napi-rs versions (`5n !== 5`). The kernel fn is `i64`; we cast at the
/// boundary. A future kernel fn needing the full `i64` range gets explicit BigInt handling then.
#[napi]
pub fn sum(a: i32, b: i32) -> i32 {
    paigasus_kernel::sum(a as i64, b as i64) as i32
}
