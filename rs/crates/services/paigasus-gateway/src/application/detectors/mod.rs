// SPDX-License-Identifier: Apache-2.0

//! The detector use case (SMA-855 § 6.6.2): the pipeline that runs the built-in stage, the
//! webhooks and the judge on one batch.

pub mod pipeline;

pub use pipeline::DetectorPipeline;

use metrics::counter;
use paigasus_observability::names;

use crate::domain::detectors::WebhookFailure;

/// Count one failed call of a webhook provider (SMA-856 D17). `provider` is the configured name,
/// never tenant content.
pub fn count_webhook_failure(provider: &str, reason: WebhookFailure) {
    counter!(
        names::GATEWAY_TOOL_POLICY_WEBHOOK_FAILURES_TOTAL,
        "provider" => provider.to_owned(),
        "reason" => reason.as_str()
    )
    .increment(1);
}
