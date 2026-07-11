// SPDX-License-Identifier: Apache-2.0

//! `OsRngKeyEntropy` — the `KeyEntropy` port's v1 implementation: 32 bytes of CSPRNG output
//! sourced from the operating system's random number generator (`rand::rngs::OsRng`, backed by
//! `getrandom`). The core stays getrandom-free (ADR-0005, `KeyEntropy`'s own doc comment); this
//! adapter is where the service actually reaches for entropy, same posture as
//! `adapters::id::KernelIdGenerator`'s `rand::random` draw.

use paigasus_iam_core::KeyEntropy;
use rand::TryRngCore;
use rand::rngs::OsRng;

#[derive(Debug, Default, Clone, Copy)]
pub struct OsRngKeyEntropy;

impl KeyEntropy for OsRngKeyEntropy {
    fn new_secret(&self) -> [u8; 32] {
        let mut secret = [0u8; 32];
        OsRng.try_fill_bytes(&mut secret).expect("OS CSPRNG unavailable");
        secret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entropy_new_secret_is_32_bytes() {
        let secret = OsRngKeyEntropy.new_secret();
        assert_eq!(secret.len(), 32);
    }

    #[test]
    fn entropy_new_secret_differs_across_calls() {
        let a = OsRngKeyEntropy.new_secret();
        let b = OsRngKeyEntropy.new_secret();
        assert_ne!(a, b, "two OS-RNG draws collided (astronomically unlikely) or the RNG is broken");
    }
}
