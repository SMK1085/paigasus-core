// SPDX-License-Identifier: Apache-2.0

//! The detector adapters (SMA-855 § 6.6). `built_in` is the stage of the kernel built-ins.

pub mod built_in;

pub use built_in::{BuiltInDetector, SCAN_LIMIT};

pub mod webhook;

pub use webhook::{NonceSource, OsNonce, WebhookDetector, prime_webhook_failure_metrics};
