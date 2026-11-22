// SPDX-License-Identifier: Apache-2.0

//! The application layer (SMA-677 § 4.3a): the use cases between the HTTP adapter and the domain.
//! Flat files, like `paigasus-iam/src/application/`, except `tool_policy`: the first subdirectory,
//! by SMA-752 spec § 6.3.

pub mod charge_guard;
pub mod decision_export;
pub mod decision_log;
pub mod detectors;
pub mod limits;
pub mod tool_policy;
