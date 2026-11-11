// SPDX-License-Identifier: Apache-2.0
//! What the two wire protocols share (spec § 4, D12). Task 6 adds the `Protocol` trait.
#![allow(dead_code)] // Removed in Task 8, when the interceptor uses every item.

/// A violation of the canonical assembly form (spec § 4.3). Always a stream-level `on_failure`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Deviation(pub &'static str);
