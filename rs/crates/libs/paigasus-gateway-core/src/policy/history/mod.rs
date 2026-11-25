// SPDX-License-Identifier: Apache-2.0
//! History conditions in the gateway core (SMA-753): the mapping from the proto form into the
//! kernel types, and the load validation. No I/O.

mod mapping;

pub use mapping::{MAX_PREDICATE_DEPTH, MalformedCondition, map_conditions};
