// SPDX-License-Identifier: Apache-2.0

//! The id source of the agent host. The kernel mints a UUIDv7. It is pure, so this adapter
//! supplies the host clock and the entropy (as `paigasus-iam/src/adapters/id.rs`).

use std::time::{SystemTime, UNIX_EPOCH};

use paigasus_kernel::mint_uuid7;
use uuid::Uuid;

/// A new UUIDv7 from the host clock and 10 random bytes.
#[must_use]
pub fn mint_uuid() -> Uuid {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock before 1970").as_millis();
    mint_uuid7(u64::try_from(ms).unwrap_or(u64::MAX), rand::random::<[u8; 10]>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_uuid_gives_distinct_v7_ids() {
        let a = mint_uuid();
        assert_eq!(a.get_version_num(), 7);
        assert_ne!(mint_uuid(), a);
    }
}
