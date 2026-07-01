// SPDX-License-Identifier: Apache-2.0
//! Property coverage for injected UUIDv7 minting (RFC 9562 layout): version/variant bits, the
//! embedded 48-bit timestamp, and k-sortability — the "ids mint and are k-sortable" AC (SMA-448).

use paigasus_kernel::mint_uuid7;
use proptest::prelude::*;

proptest! {
    #[test]
    fn version_and_variant(ms: u64, rand: [u8; 10]) {
        let u = mint_uuid7(ms, rand);
        prop_assert_eq!(u.get_version_num(), 7);
        prop_assert_eq!(u.as_bytes()[8] & 0xC0, 0x80); // RFC 4122 variant 0b10xxxxxx
    }

    #[test]
    fn timestamp_is_embedded(ms: u64, rand: [u8; 10]) {
        let u = mint_uuid7(ms, rand);
        let b = u.as_bytes();
        let mut ts = 0u64;
        for &byte in &b[0..6] {
            ts = (ts << 8) | u64::from(byte);
        }
        prop_assert_eq!(ts, ms & 0x0000_FFFF_FFFF_FFFF);
    }

    #[test]
    fn k_sortable(ms_a in 0u64..(1u64 << 48), ms_b in 0u64..(1u64 << 48), r1: [u8; 10], r2: [u8; 10]) {
        prop_assume!(ms_a < ms_b);
        prop_assert!(mint_uuid7(ms_a, r1).as_bytes() < mint_uuid7(ms_b, r2).as_bytes());
    }
}
