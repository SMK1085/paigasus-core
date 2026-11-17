// SPDX-License-Identifier: Apache-2.0

//! Adapters of the `PolicySource` port (SMA-752). `file` reads a directory. `iam` fetches the compiled sets from IAM (SMA-775).

pub mod file;
pub mod iam;
