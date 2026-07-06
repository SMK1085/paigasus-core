// SPDX-License-Identifier: Apache-2.0

//! OIDC adapters (SMA-443, M2): discovery + JWKS fetch/cache/rotation live in `jwks`; the
//! `Authenticator` v1 implementation lives in `validator`.

pub mod jwks;
pub mod validator;
