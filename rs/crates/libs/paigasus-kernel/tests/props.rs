// SPDX-License-Identifier: Apache-2.0
//! Property-based coverage of the kernel itself — the "property-based suite against the Rust
//! impl" half of ADR-0005's safety net (SMA-433). Randomized, fresh each run; proptest persists
//! any failing seed to `proptest-regressions/` for reproduction. Inputs are drawn as `i32` and
//! widened to the kernel's `i64`, so `a + b` can never overflow `i64` — and the range mirrors the
//! committed corpus's i32-safe parity domain.

use paigasus_kernel::sum;
use proptest::prelude::*;

proptest! {
    #[test]
    fn matches_integer_addition(a: i32, b: i32) {
        prop_assert_eq!(sum(a as i64, b as i64), a as i64 + b as i64);
    }

    #[test]
    fn is_commutative(a: i32, b: i32) {
        prop_assert_eq!(sum(a as i64, b as i64), sum(b as i64, a as i64));
    }

    #[test]
    fn zero_is_identity(a: i64) {
        prop_assert_eq!(sum(a, 0), a);
    }
}
