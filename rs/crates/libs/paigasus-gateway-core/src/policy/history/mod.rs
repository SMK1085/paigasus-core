// SPDX-License-Identifier: Apache-2.0
//! History conditions in the gateway core (SMA-753): the mapping from the proto form into the
//! kernel types, the load validation, and the field and read plans. No I/O.

mod mapping;
mod plan;

pub use mapping::{MAX_PREDICATE_DEPTH, MalformedCondition, map_conditions};
pub use plan::{FieldPlan, HistoryPolicy, ReadPlan, ReadSet};
