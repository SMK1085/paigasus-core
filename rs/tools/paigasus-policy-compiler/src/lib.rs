// SPDX-License-Identifier: Apache-2.0
//! The Paigasus tool policy compiler: lowers Dogwood tool policy sets to Cedar.

pub mod compile;
pub mod lower;
pub mod protocol;
pub mod translate;

/// The compiler generation (spec 2026-11-15 iam-tool-policy-sets, section 6.2). Bump it when
/// the compiler output can change: a Dogwood bump, a translation change or a protocol change.
/// `golden_output_is_pinned_to_the_compiler_generation` in `tests/golden.rs` pins it.
pub const COMPILER_GENERATION: u32 = 1;

/// The version of the JSON protocol with IAM (spec section 5.2).
pub const PROTOCOL_VERSION: u32 = 1;

/// The Dogwood commit in `Cargo.toml`.
pub const DOGWOOD_COMMIT: &str = "ff158506cfd9c7871385ed3a87d7435505fd24b1";

/// The revision string for logs: `<generation>+dogwood.<12-char commit>+proto.<protocol>`.
pub fn revision_string() -> String {
    format!("{COMPILER_GENERATION}+dogwood.{}+proto.{PROTOCOL_VERSION}", &DOGWOOD_COMMIT[..12])
}
