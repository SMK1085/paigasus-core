// SPDX-License-Identifier: Apache-2.0

//! The CloudEvents 1.0 envelope `paigasus-iam` publishes its domain events in (SMA-471 D6).
//!
//! **Structured content mode**: the whole event is one JSON object in the message body, content
//! type `application/cloudevents+json`. Chosen over binary mode because the relay hands the
//! publisher a complete event and consumers are polyglot — one JSON blob is what every
//! CloudEvents SDK reads without NATS-specific glue.
//!
//! **This type is the public wire contract.** External consumers depend on these attribute
//! names and on `EventType`'s wire strings; changing either is a breaking change.
//!
//! **Dedup identity, deliberately narrowed.** CloudEvents scopes event identity to
//! `(source, id)`; JetStream's `Nats-Msg-Id` dedup keys on `id` alone. Consumers must therefore
//! dedup on `id`, and `source` must stay stable for the lifetime of a stream (ADR-0016).
//!
//! Extension attribute names (`schemaversion`, `actorprn`, `correlationid`) are lowercase
//! alphanumeric with no separators because the CloudEvents spec requires exactly that — hence
//! `actorprn`, not `actor_prn`.

use chrono::{DateTime, SecondsFormat, Utc};
use paigasus_iam_core::DomainEvent;
use serde::Serialize;
use uuid::Uuid;

/// One `DomainEvent` rendered as a CloudEvents 1.0 JSON envelope. Borrows from the event; build
/// it with [`CloudEvent::from_domain_event`] immediately before serializing.
#[derive(Debug, Serialize)]
pub struct CloudEvent<'a> {
    specversion: &'static str,
    id: String,
    source: &'a str,
    #[serde(rename = "type")]
    event_type: &'static str,
    /// Omitted when empty: CloudEvents requires `subject`, if present, to be non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    subject: Option<&'a str>,
    time: String,
    datacontenttype: &'static str,
    schemaversion: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    actorprn: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    correlationid: Option<String>,
    data: &'a serde_json::Value,
}

impl<'a> CloudEvent<'a> {
    #[must_use]
    pub fn from_domain_event(ev: &'a DomainEvent, source: &'a str) -> CloudEvent<'a> {
        CloudEvent {
            specversion: "1.0",
            id: render_id(ev.id),
            source,
            event_type: ev.event_type.as_wire(),
            subject: if ev.aggregate_prn.is_empty() { None } else { Some(ev.aggregate_prn.as_str()) },
            time: render_time(ev.occurred_at),
            datacontenttype: "application/json",
            schemaversion: ev.schema_version,
            actorprn: ev.actor_prn.as_deref(),
            correlationid: ev.correlation_id.map(render_id),
            data: &ev.payload,
        }
    }
}

/// The one place a `Uuid` becomes a string for the wire. Both the CloudEvents `id` and the
/// `Nats-Msg-Id` header go through here, so they can never disagree (SMA-471 D3).
#[must_use]
pub fn render_id(id: Uuid) -> String {
    id.hyphenated().to_string()
}

/// RFC 3339 with microsecond precision and a `Z` suffix. `occurred_at` is already microsecond-
/// truncated by the `Clock` port, and the wire format matches that precision deliberately so
/// consumers keep sub-second ordering. Widening precision later would be a breaking change to
/// the public contract.
fn render_time(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Micros, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};
    use paigasus_iam_core::EventType;
    use uuid::Uuid;

    fn sample(actor: Option<&str>, correlation: Option<Uuid>, aggregate: &str) -> DomainEvent {
        DomainEvent {
            id: Uuid::from_u128(0x1234),
            event_type: EventType::PrincipalCreated,
            schema_version: 1,
            aggregate_prn: aggregate.to_string(),
            actor_prn: actor.map(str::to_string),
            occurred_at: Utc.with_ymd_and_hms(2026, 8, 7, 12, 30, 45).unwrap().with_nanosecond(123_456_000).unwrap(),
            payload: serde_json::json!({"kind": "user", "nested": {"a": 1}}),
            correlation_id: correlation,
        }
    }

    fn render(ev: &DomainEvent) -> serde_json::Value {
        serde_json::to_value(CloudEvent::from_domain_event(ev, "urn:paigasus:iam")).unwrap()
    }

    #[test]
    fn maps_every_required_attribute() {
        let ev = sample(Some("prn:pgs:iam:::principal/aa"), Some(Uuid::from_u128(9)), "prn:pgs:iam:::principal/bb");
        let j = render(&ev);
        assert_eq!(j["specversion"], "1.0");
        assert_eq!(j["id"], ev.id.to_string());
        assert_eq!(j["source"], "urn:paigasus:iam");
        assert_eq!(j["type"], "iam.principal.created");
        assert_eq!(j["subject"], "prn:pgs:iam:::principal/bb");
        assert_eq!(j["datacontenttype"], "application/json");
        assert_eq!(j["schemaversion"], 1);
        assert_eq!(j["actorprn"], "prn:pgs:iam:::principal/aa");
        assert_eq!(j["correlationid"], Uuid::from_u128(9).to_string());
        assert_eq!(j["data"], serde_json::json!({"kind": "user", "nested": {"a": 1}}));
    }

    #[test]
    fn time_is_rfc3339_and_round_trips() {
        let ev = sample(None, None, "prn:x");
        let rendered = render(&ev)["time"].as_str().unwrap().to_string();
        // Verify the rendered string includes microsecond digits
        assert!(rendered.contains("123456"), "rendered time must include microseconds: {rendered}");
        // Verify it parses back to exactly the original DateTime
        let parsed = chrono::DateTime::parse_from_rfc3339(&rendered).unwrap().with_timezone(&Utc);
        assert_eq!(parsed, ev.occurred_at, "time must round-trip exactly");
    }

    /// CloudEvents has no null attribute values — absent optionals are OMITTED, not `null`.
    #[test]
    fn absent_optionals_are_omitted_not_null() {
        let j = render(&sample(None, None, "prn:x"));
        assert!(j.get("actorprn").is_none(), "actorprn must be absent: {j}");
        assert!(j.get("correlationid").is_none(), "correlationid must be absent: {j}");
    }

    /// CloudEvents requires `subject`, if present, to be non-empty.
    #[test]
    fn an_empty_aggregate_prn_omits_subject() {
        let j = render(&sample(None, None, ""));
        assert!(j.get("subject").is_none(), "empty subject must be omitted: {j}");
    }

    /// The CloudEvents `id` and `Nats-Msg-Id` must be the same string (SMA-471 D3/D6). Pinned
    /// so a `Display` change cannot silently break dedup.
    #[test]
    fn id_renders_as_hyphenated_lowercase() {
        let ev = sample(None, None, "prn:x");
        let rendered = render(&ev)["id"].as_str().unwrap().to_string();
        assert_eq!(rendered, "00000000-0000-0000-0000-000000001234");
        assert_eq!(rendered, ev.id.to_string(), "must match what publish uses for Nats-Msg-Id");
    }

    /// SMA-606 D8: iterates `EventType::ALL` rather than a hand-listed array. The previous
    /// form hard-coded eight variants, so a new one compiled cleanly and went uncovered —
    /// P2-D4 called this a compile-time tripwire and it was not one. `ALL` is kept exhaustive
    /// by `all_lists_every_event_type`'s wildcard-free match, so this now transitively fails
    /// to compile for a variant with no wire string.
    #[test]
    fn type_matches_the_wire_string_for_every_variant() {
        for et in EventType::ALL {
            let mut ev = sample(None, None, "prn:x");
            ev.event_type = et;
            assert_eq!(render(&ev)["type"], et.as_wire(), "rendered `type` must equal the wire string for {et:?}");
        }
    }

    /// SMA-471 §5: no payload may carry a secret or PII. If a future payload adds one, this
    /// reds CI instead of quietly broadcasting it to every subscriber.
    #[test]
    fn no_payload_shape_carries_a_secret_or_pii_key() {
        let payloads = [
            serde_json::json!({"principal_id": "p", "kind": "user"}),
            serde_json::json!({"key_id": "k", "prefix": "pgs_live_ab", "scope": "s", "status": "active", "expires_at": "2026-01-01T00:00:00Z"}),
            serde_json::json!({"grant_id": "g", "role_key": "admin", "scope": "prn:pgs:iam:::org/o"}),
            serde_json::json!({"policy_id": "pol", "policy_key": "starter"}),
            // SMA-606 D9: the tenancy shapes. Hand-listed because this test scans sample
            // values by substring — it cannot see runtime content, so it proves the SHAPE
            // carries no banned key, not that an operator's `name` is free of PII (see the
            // spec's Limitations and the ADR-0016 amendment).
            serde_json::json!({"node_prn": "prn:pgs:iam:::org/o", "slug": "acme", "name": "Acme", "status": "active", "effective_status": "active"}),
            serde_json::json!({"node_prn": "prn:pgs:iam:::org/o", "slug": "acme", "name": "Acme"}),
            serde_json::json!({"node_prn": "prn:pgs:iam:::org/o", "status": "archived", "effective_status": "archived"}),
            // SMA-606 fix wave finding 8: the auto-provisioned default team's `TeamCreated`
            // payload (`organizations.rs:186-201`) carries a `"source"` key an explicit
            // `TeamService::create` does not — its own shape, not a substring of the plain
            // create shape above.
            serde_json::json!({"node_prn": "prn:pgs:iam:::team/o/t", "slug": "default", "name": "Default", "status": "active", "effective_status": "active", "source": "organization_create"}),
            serde_json::json!({"membership_id": "m", "principal_prn": "prn:pgs:iam:::principal/p", "node_prn": "prn:pgs:iam:::org/o"}),
            // SMA-606 fix wave finding 8: `MembershipDetached`'s payload — same shape as
            // `MembershipAttached` above (fix wave finding 3 moved `cascade_of` off the wire
            // payload entirely, onto the audit entry's `detail` only), listed explicitly so the
            // inventory names every emitter rather than leaving detach's coverage implicit.
            serde_json::json!({"membership_id": "m", "principal_prn": "prn:pgs:iam:::principal/p", "node_prn": "prn:pgs:iam:::project/o/t/p"}),
            serde_json::json!({"grant_id": "g", "role_key": "org_admin", "scope": "prn:pgs:iam:::org/o", "source": "organization_create"}),
        ];
        let banned = ["hash", "secret", "plaintext", "email", "pepper", "token", "password"];
        for payload in payloads {
            let mut ev = sample(None, None, "prn:x");
            ev.payload = payload;
            let rendered = serde_json::to_string(&CloudEvent::from_domain_event(&ev, "urn:paigasus:iam")).unwrap().to_lowercase();
            for needle in banned {
                assert!(!rendered.contains(needle), "payload leaked a `{needle}` key: {rendered}");
            }
        }
    }
}
