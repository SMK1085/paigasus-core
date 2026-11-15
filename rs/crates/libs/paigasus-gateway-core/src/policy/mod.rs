// SPDX-License-Identifier: Apache-2.0
//! The tool-policy side of the gateway core (SMA-752): the mapping from gateway-core request and
//! call types into the kernel's tool-policy types. No I/O.

mod mapping;

pub use mapping::{call_findings, ingress, invocation, name_collisions};
