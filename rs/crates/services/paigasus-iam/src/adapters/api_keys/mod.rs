// SPDX-License-Identifier: Apache-2.0

//! API-key secret hashing + entropy adapters (SMA-445, M4): the `SecretHasher`/`KeyEntropy`
//! port implementations, the redacting `Pepper` config newtype the hasher is keyed by, and the
//! fail-open introspection cache (memory + redis) sitting in front of the DB-backed validation
//! path (spec §9/D5).

pub mod cache;
pub mod entropy;
pub mod hasher;

pub use cache::{ApiKeyValidationCache, CachedValidation, MemoryApiKeyCache, RedisApiKeyCache};
pub use entropy::OsRngKeyEntropy;
pub use hasher::{HmacSecretHasher, Pepper, PepperConfigError};
