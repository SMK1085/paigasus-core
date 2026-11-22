// SPDX-License-Identifier: Apache-2.0

//! The S3-compatible `MemoryStore` adapter (SMA-829).
//!
//! The adapter keeps each memory file as one object. The object key comes from `keys`.

pub mod envelope;
pub mod errors;
pub mod keys;
