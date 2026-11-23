// SPDX-License-Identifier: Apache-2.0

//! The agent host service: configuration, persistence and the HTTP surface.

// SMA-790 §6.11: the test model and the file barrier must never reach a release binary.
#[cfg(all(feature = "test-model", not(debug_assertions)))]
compile_error!("the test-model feature is for debug test builds only");

pub mod adapters;
pub mod application;
pub mod config;
