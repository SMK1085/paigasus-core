// SPDX-License-Identifier: Apache-2.0

//! The detector use case (SMA-855 § 6.6.2): the pipeline that runs the built-in stage, the
//! webhooks and the judge on one batch.

pub mod pipeline;

pub use pipeline::DetectorPipeline;
