// SPDX-License-Identifier: Apache-2.0

//! Adapters — concrete implementations of the core's ports.

pub mod api_keys;
pub mod auth;
pub mod authz;
pub mod boot;
pub mod clock;
pub mod events;
pub mod grpc;
pub mod http;
pub mod id;
pub mod oidc;
pub mod persistence;
pub(crate) mod error_chain;
pub(crate) mod redis_conn;
pub(crate) mod retryable;
