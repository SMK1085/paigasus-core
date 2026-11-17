// SPDX-License-Identifier: Apache-2.0

//! The outbox events of the tool policy sets (SMA-775). A write that stores a new live version
//! enqueues one event in the same transaction. The gateway subscribes to `iam.policy.>` and
//! drops its cached set on the event. The correlation id is the one in the audit entry of the
//! same write: the caller mints it once and passes it to both.

use chrono::{DateTime, Utc};
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::tool_policy::{ToolPolicySettings, ToolPolicyVersion};
use paigasus_iam_core::{DomainEvent, EventType, IdGenerator};
use paigasus_kernel::Prn;
use serde_json::json;
use uuid::Uuid;

use super::origin_str;

/// `iam.policy.tool_set.published`: a new live version of the tenant set of `org`.
pub(crate) fn set_published(ids: &impl IdGenerator, corr: Uuid, actor: &Prn, org_prn: &Prn, org: Uuid, v: &ToolPolicyVersion, now: DateTime<Utc>) -> DomainEvent {
    DomainEvent {
        id: ids.new_event_id(),
        event_type: EventType::ToolPolicySetPublished,
        schema_version: 1,
        aggregate_prn: org_prn.canonical(),
        actor_prn: Some(actor.canonical()),
        occurred_at: now,
        payload: json!({"org_id": org.to_string(), "version": v.version, "origin": origin_str(v.origin)}),
        correlation_id: Some(corr),
    }
}

/// `iam.policy.tool_settings.changed`: the settings of `org` were written.
pub(crate) fn settings_changed(ids: &impl IdGenerator, corr: Uuid, actor: &Prn, org_prn: &Prn, org: Uuid, s: &ToolPolicySettings, now: DateTime<Utc>) -> DomainEvent {
    DomainEvent {
        id: ids.new_event_id(),
        event_type: EventType::ToolPolicySettingsChanged,
        schema_version: 1,
        aggregate_prn: org_prn.canonical(),
        actor_prn: Some(actor.canonical()),
        occurred_at: now,
        payload: json!({"org_id": org.to_string(), "mode": s.mode.as_str(), "fail_mode": s.fail_mode.as_str()}),
        correlation_id: Some(corr),
    }
}

/// `iam.policy.tool_floor.published`: a new live version of the floor set.
pub(crate) fn floor_published(ids: &impl IdGenerator, corr: Uuid, actor: &Prn, v: &ToolPolicyVersion, now: DateTime<Utc>) -> DomainEvent {
    DomainEvent {
        id: ids.new_event_id(),
        event_type: EventType::ToolFloorPublished,
        schema_version: 1,
        aggregate_prn: root_prn().canonical(),
        actor_prn: Some(actor.canonical()),
        occurred_at: now,
        payload: json!({"version": v.version, "origin": origin_str(v.origin)}),
        correlation_id: Some(corr),
    }
}
