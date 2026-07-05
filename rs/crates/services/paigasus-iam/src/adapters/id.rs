// SPDX-License-Identifier: Apache-2.0

//! `KernelIdGenerator` — mints a UUIDv7 + PRN via `paigasus-kernel`, supplying the host's
//! clock and entropy (the kernel is pure and does neither).

// Nothing in `main.rs` wires this into a use case yet — the composition root lands in
// Task 11. Until then it's exercised only via the `#[cfg(test)]` test below; same
// reasoning as `application::create_user` (Task 6).
#![allow(dead_code)]

use paigasus_iam_core::{IdGenerator, PrincipalId};
use paigasus_kernel::{Prn, mint_uuid7};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Default, Clone, Copy)]
pub struct KernelIdGenerator;

impl IdGenerator for KernelIdGenerator {
    fn new_principal_id(&self) -> PrincipalId {
        let unix_ms = SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock before 1970").as_millis() as u64;
        let rand: [u8; 10] = rand::random();
        let uuid = mint_uuid7(unix_ms, rand);
        // Statically infallible for these fixed, valid inputs (service/type are valid labels,
        // region empty, org none, id a valid UUID).
        let prn = Prn::build("iam", "", None, "principal", uuid).expect("valid IAM principal PRN");
        PrincipalId::from_prn(prn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mints_a_v7_principal_prn() {
        let id = KernelIdGenerator.new_principal_id();
        assert_eq!(id.uuid().get_version_num(), 7);
        let canonical = id.canonical();
        assert!(canonical.starts_with("prn:pgs:iam:::principal/"), "unexpected PRN: {canonical}");
        // Distinct calls mint distinct ids.
        assert_ne!(KernelIdGenerator.new_principal_id().uuid(), id.uuid());
    }
}
