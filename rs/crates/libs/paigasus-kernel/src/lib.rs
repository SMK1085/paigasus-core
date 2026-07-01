// SPDX-License-Identifier: Apache-2.0

//! Pure-logic behavioral kernel for Paigasus.
//!
//! Bound to Python / Node / WASM via the crates under `rs/crates/bindings/`. No FFI or
//! adapter dependencies live here (ADR-0005). Empty until real logic lands.

pub mod cedar;
pub mod prn;
pub mod uuid7;

pub use cedar::{CedarUid, to_cedar_uid};
pub use prn::{Prn, PrnError};
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
