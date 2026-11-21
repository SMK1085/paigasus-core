// SPDX-License-Identifier: Apache-2.0

//! The outbound IAM gRPC adapter: the `Iam` port and its `tonic` implementation (spec §6.2).

pub mod client;

pub use client::{CallerCredential, Iam, IamClient, IamError};
