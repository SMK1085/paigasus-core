// SPDX-License-Identifier: Apache-2.0
//! Shared test support for the paigasus-gateway-core integration tests (SMA-749 § 8).
#![allow(dead_code)] // Each test binary uses a different subset of this module.

pub mod driver;
pub mod oracle;
pub mod streams;
