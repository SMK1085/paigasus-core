// SPDX-License-Identifier: Apache-2.0

//! Pure-logic behavioral kernel for Paigasus.
//!
//! The cross-language primitives that must behave identically in every runtime:
//! [`Prn`] (Paigasus Resource Names), [`mint_uuid7`] (UUIDv7 from injected bytes — no
//! ambient entropy, so the crate builds for `wasm32-unknown-unknown`), and
//! [`to_cedar_uid`] (Cedar entity UIDs).
//!
//! No I/O, no FFI, and no adapter dependencies live here. The Python, Node and browser
//! bindings under `rs/crates/bindings/` call into this crate rather than reimplementing
//! it (ADR-0005).

pub mod cedar;
// The PRN value type lives in `resource_name`, NOT `prn`: `prn` (PRN) is a Windows reserved device
// name, so a `prn.rs` file cannot be checked out on Windows (git fails with "invalid path"). Do not
// rename this back to `prn`. The public type is still `Prn` (re-exported below).
pub mod resource_name;
pub mod uuid7;

pub use cedar::{CedarUid, to_cedar_uid};
pub use resource_name::{Prn, PrnError};
pub use uuid7::mint_uuid7;

/// Sum two integers — the kernel's first real, pure primitive. Deliberately minimal
/// (placeholder for real domain logic); its purpose is to give the PyO3 binding a genuine
/// kernel call to consume so the `paigasus-kernel-rs → paigasus-py-bindings-rs` edge is real
/// (ADR-0005, SMA-409).
#[must_use]
pub fn sum(a: i64, b: i64) -> i64 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::sum;

    #[test]
    fn sums_two_integers() {
        assert_eq!(sum(2, 3), 5);
        assert_eq!(sum(-4, 4), 0);
    }
}
