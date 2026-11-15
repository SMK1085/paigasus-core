// SPDX-License-Identifier: Apache-2.0

//! The port to the policy compiler (spec section 5.2). The adapter in the service crate runs
//! the `paigasus-policy-compiler` binary as a child process.

use async_trait::async_trait;

use super::protocol::{CompileRequest, CompiledOutput, Rejection};

/// A compile that gave no result. Neither case stores anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompilerFailure {
    /// The compiler semaphore is full (`tool-policy-compiler-busy`, retryable).
    Busy,
    /// The compiler crashed, timed out or broke the protocol (`tool-policy-compiler-unavailable`).
    /// The string is for logs only and is never returned to a caller.
    Unavailable(String),
}

/// The result of a compile that the compiler answered.
#[derive(Debug, Clone, PartialEq)]
pub enum CompileOutcome {
    Accepted(CompiledOutput),
    Rejected(Vec<Rejection>),
}

/// Compiles one tool policy set.
#[async_trait]
pub trait PolicyCompiler: Send + Sync {
    /// The compiler generation that this compiler reported at boot.
    fn generation(&self) -> u32;

    /// Compile one request.
    async fn compile(&self, req: &CompileRequest) -> Result<CompileOutcome, CompilerFailure>;
}
