// SPDX-License-Identifier: Apache-2.0

//! OIDC adapters (SMA-443, M2): discovery + JWKS fetch/cache/rotation live in `jwks`.
//! The `Authenticator` v1 implementation (Task 7) is deliberately NOT declared here yet —
//! its module doesn't exist until that task lands.

pub mod jwks;
