// SPDX-License-Identifier: Apache-2.0

//! The detector adapters (SMA-855 § 6.6). `built_in` is the stage of the kernel built-ins.

pub mod built_in;

pub use built_in::{BuiltInDetector, SCAN_LIMIT};
