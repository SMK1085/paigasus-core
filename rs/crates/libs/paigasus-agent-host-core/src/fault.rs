// SPDX-License-Identifier: Apache-2.0

//! Fault points for tests: a place in a run where a test can inject a failure.

use async_trait::async_trait;

/// The stage of a run at which a fault point is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultStage {
    AfterRun,
    BeforeCommit,
    AfterCommit,
}

/// A fault point. The default method does nothing.
#[async_trait]
pub trait FaultPoint: Send + Sync {
    /// Called when a run reaches `stage`.
    async fn reach(&self, stage: FaultStage) {
        let _ = stage;
    }
}

/// The fault point that injects no fault.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoFaults;

impl FaultPoint for NoFaults {}
