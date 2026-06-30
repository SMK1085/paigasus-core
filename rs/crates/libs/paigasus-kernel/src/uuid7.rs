// SPDX-License-Identifier: Apache-2.0
//! Injected UUIDv7 minting (RFC 9562). Pure and deterministic: the caller supplies the clock
//! (`unix_ms`) and entropy (`rand`), so the kernel does NO I/O — no system clock, no `getrandom`
//! — and the wasm build stays free of both (ADR-0005, SMA-448).

use uuid::Uuid;

/// Build a UUIDv7 from an injected millisecond timestamp and 10 random bytes.
///
/// Layout (16 bytes): `[0..6]` = low 48 bits of `unix_ms` (big-endian); `[6..16]` = `rand`, with
/// the version nibble (`0b0111`) overwriting the high nibble of byte 6 and the variant (`0b10`)
/// overwriting the high 2 bits of byte 8. 74 random bits survive; the high nibble of `rand[0]` and
/// the high 2 bits of `rand[2]` are discarded. `unix_ms` is masked to 48 bits.
#[must_use]
pub fn mint_uuid7(unix_ms: u64, rand: [u8; 10]) -> Uuid {
    let ms_be = (unix_ms & 0x0000_FFFF_FFFF_FFFF).to_be_bytes(); // low 48 bits live in [2..8]
    let mut bytes = [0u8; 16];
    bytes[0..6].copy_from_slice(&ms_be[2..8]);
    bytes[6..16].copy_from_slice(&rand);
    bytes[6] = 0x70 | (bytes[6] & 0x0F); // version 7
    bytes[8] = 0x80 | (bytes[8] & 0x3F); // RFC 4122 variant
    Uuid::from_bytes(bytes)
}
