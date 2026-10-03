// SPDX-License-Identifier: Apache-2.0

//! The Docker-skip policy for this crate's container-backed suites (SMA-538). Since SMA-726 it
//! lives once, in the dev-only crate `paigasus-test-docker`, which the gateway's suites share.
//! This file only re-exports it, so every suite keeps calling `support::docker::start_or_skip`,
//! and the Redis-only files keep including it with `#[path = "support/docker.rs"] mod docker;`.
//!
//! `#[allow(unused_imports)]`: this file is compiled into every test binary that includes it, and
//! under `[workspace.lints.rust] warnings = "deny"` a binary that used no item of the glob would
//! fail to build.

#[allow(unused_imports)]
pub use paigasus_test_docker::*;
