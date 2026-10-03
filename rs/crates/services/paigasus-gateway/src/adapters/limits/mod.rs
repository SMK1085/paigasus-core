// SPDX-License-Identifier: Apache-2.0

//! The two `LimitStore` adapters (SMA-677 spec § 4.3).

pub mod memory;

pub use memory::MemoryLimitStore;
pub mod redis;

pub use self::redis::RedisLimitStore;
