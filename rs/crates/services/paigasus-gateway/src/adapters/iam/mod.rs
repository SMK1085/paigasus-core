// SPDX-License-Identifier: Apache-2.0

//! Outbound IAM gRPC adapter (the `Iam` port + its `tonic` implementation). See
//! [`client`] for the introspect/self-query semantics and the D9 self-query invariant.

pub mod client;

pub use client::{Iam, IamClient, IamError};
