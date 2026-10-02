// SPDX-License-Identifier: Apache-2.0

//! paigasus-iam library surface (for integration tests + the binary).
//!
//! Config reference + defaults: `iam.toml.example` (crate root). In particular, see the
//! `[authn.jwks_cache]` TRUST NOTE there before enabling `backend = "redis"` — the JWKS
//! cache holds the configured issuers' public signing keys, so a writable cache is an
//! authentication bypass (D15, `docs/superpowers/specs/2026-07-06-sma-443-m2-authentication-design.md`).

pub mod adapters;
pub mod application;
pub mod config;
pub mod service_info;
