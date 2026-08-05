// SPDX-License-Identifier: Apache-2.0

//! `POST /v1/authz/system-policies/{id}/retire` (SMA-481): a thin adapter over
//! `AppState.retirement` — parse -> `SystemRetirementService::retire` -> response, no business
//! logic here (mirrors `http::dead_letters`).
//!
//! Root-only, enforced INSIDE `SystemRetirementService::retire` itself (mirrors every
//! `dead_letters` route), so a non-Root caller gets `403` with nothing about the target row
//! reaching the response. Sits on the bearer-gated `protected` sub-router; the caller's PRN
//! comes from the auth middleware's `AuthContext`, never a client-supplied value.
//!
//! **200, never 204, on success.** A retirement destroys a row permanently, and the response
//! body (`policy_id`/`kind`/`role_deleted`) is the operator's only IMMEDIATE record of exactly
//! what was destroyed — an empty `204` would throw that record away at the one moment it is
//! cheapest to keep it. The durable second copy is the audit entry
//! `SystemRetirementService::retire` writes in the same transaction, but reading it back means
//! a separate query; the HTTP response is free.
//!
//! **The two refusals (`Blocked`/`NeedsAcknowledgement`) are hand-built `409`s, not routed
//! through `ApiError`.** `ApiError` exists for a `TenancyError` — a genuine failure the caller
//! can only react to by fixing their request, rendered as the stable `{"error": {"code",
//! "message"}}` envelope alone. A refusal here is different in kind: it is an
//! `Ok(RetireOutcome)` carrying exactly the information an operator needs to proceed next —
//! which grants survive and the true count, or what a static policy's removal would change —
//! so it is not an error at all, just an outcome that isn't `Retired`. Routing it through
//! `ApiError` would force that generic mapper to either drop the payload or special-case one
//! action's outcome inside code meant to stay a single, boring `TenancyError -> {code,
//! message}` mapping. `conflict` below still emits `409` and keeps the SAME `error.code`/
//! `error.message` shape every other error in this service uses — it only adds sibling fields
//! next to `error`, so a client that only ever reads `error.code` behaves identically whether
//! the `409` came from here or from `ApiError`.
//!
//! **The request body is optional.** A `POST` with no body and no `Content-Type` header must
//! not be rejected before ever reaching the service: the safe reading of an unspecified
//! acknowledgement is "not acknowledged" (the service's own D4), so `Option<EnvelopeJson<
//! RetireBody>>` plus `#[serde(default)]` on the one field collapses "didn't say" and "said no"
//! into the identical `false`. A body that DOES declare `Content-Type: application/json` but
//! fails to parse still gets the crate's stable `{"error":{code,message}}` envelope, via the
//! SAME `EnvelopeJson` (`http::authn`) `http::authn::introspect`/`http::api_keys::introspect`
//! already reuse for exactly this normalisation, rather than axum's bare `JsonRejection` text.
//!
//! **The outcome -> response mapping is its own pure function (`response_for`), not inlined in
//! the handler.** A fix-round review changed `Retired`'s status to `204` in an earlier revision
//! of this file and reran the whole crate's test suite unchanged — nothing exercised the
//! handler against a real `RetireOutcome` value, so the exact regression this module's doc
//! calls out as load-bearing would have shipped silently. Pulling the match out into a function
//! that takes an owned `RetireOutcome` and returns a `Response` lets every variant be
//! constructed directly in a test, with no `AppState`/database/request needed.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use paigasus_iam_core::authz::reconcile::policy_kind_str;
use paigasus_iam_core::{GrantRef, RetireOutcome};
use paigasus_kernel::Prn;
use serde_json::json;

use super::AppState;
use super::authn::EnvelopeJson;
use super::error::ApiError;
use crate::adapters::auth::AuthContext;

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/authz/system-policies/{id}/retire", post(retire))
}

/// The acting principal's canonical `Prn`, from the bearer-resolved `AuthContext` — mirrors
/// `http::dead_letters::actor_prn`.
fn actor_prn(ctx: &AuthContext) -> Prn {
    ctx.principal_id.prn().clone()
}

/// An absent or empty body means "not acknowledged" — the flag must be typed deliberately, so
/// the safe reading is the default (D4).
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
struct RetireBody {
    acknowledge_decision_change: bool,
}

/// Collapses "no body at all" and "a body that explicitly says no" into the identical `false` —
/// pulled out of the handler so the "absent means unacknowledged" contract is unit-testable
/// without needing an `AppState`/database to build a full request through the router.
fn acknowledged(body: Option<RetireBody>) -> bool {
    body.is_some_and(|b| b.acknowledge_decision_change)
}

/// `POST /v1/authz/system-policies/{id}/retire`: Root-only (enforced inside
/// `SystemRetirementService::retire`). Returns `200` with what was destroyed, never `204` — a
/// body is the operator's only immediate record of an irreversible act, and the two refusals
/// below carry the information needed to act on them. The actual outcome -> response mapping
/// lives in `response_for` (this module's doc), so it stays testable independent of this thin
/// axum-wiring layer.
async fn retire(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<String>, body: Option<EnvelopeJson<RetireBody>>) -> Result<Response, ApiError> {
    let ack = acknowledged(body.map(|EnvelopeJson(b)| b));
    let outcome = s.retirement.retire(&actor_prn(&ctx), &id, ack).await?;
    Ok(response_for(outcome))
}

/// Maps a `RetireOutcome` to its wire response — see this module's doc for why this is its own
/// pure function rather than inlined in `retire` above.
fn response_for(outcome: RetireOutcome) -> Response {
    match outcome {
        RetireOutcome::Retired { policy_id, kind, role_deleted } => {
            (StatusCode::OK, Json(json!({ "policy_id": policy_id, "kind": policy_kind_str(kind), "role_deleted": role_deleted }))).into_response()
        }
        RetireOutcome::Blocked { role_key, grants, total, truncated } => conflict(
            "grants-survive",
            &format!(
                "{total} grant(s) of '{role_key}' must be revoked before it can be retired. If a revoke returns 403 \
                 because its scope node is archived, restore the node, revoke, then re-archive it."
            ),
            json!({ "grants": grants_json(&grants), "total_surviving": total, "truncated": truncated }),
        ),
        RetireOutcome::NeedsAcknowledgement { policy_id, kind, source, description } => conflict(
            "decision-change-unacknowledged",
            &format!(
                "'{policy_id}' is a static policy: it is evaluated on every request, so retiring it changes decisions \
                 fleet-wide. Re-send with acknowledge_decision_change=true."
            ),
            json!({ "kind": policy_kind_str(kind), "source": source, "description": description }),
        ),
    }
}

/// Renders a `Blocked` refusal's surviving grants for the wire. `GrantRef` deliberately doesn't
/// derive `Serialize` (`retirement.rs`'s own doc: "crosses straight into an HTTP body and never
/// back into a decision") — projecting it here, at the one place it crosses into JSON, keeps
/// that crate free of an HTTP-shaped dependency.
fn grants_json(grants: &[GrantRef]) -> serde_json::Value {
    json!(
        grants
            .iter()
            .map(|g| json!({ "id": g.id, "principal_prn": g.principal_prn, "scope_prn": g.scope_prn }))
            .collect::<Vec<_>>()
    )
}

/// Builds a `409` that keeps the stable `{"error": {"code", "message"}}` envelope every other
/// error in this service uses, and hangs the retirement-specific data off it as sibling fields.
/// The handler builds these itself rather than going through `ApiError` because a refusal is an
/// `Ok` outcome carrying information, not a `TenancyError` (D5, and this module's own doc).
fn conflict(code: &str, message: &str, mut extra: serde_json::Value) -> Response {
    let obj = extra.as_object_mut().expect("extra is always a json object — every call site above passes a json!({...})");
    obj.insert("error".to_string(), json!({ "code": code, "message": message }));
    (StatusCode::CONFLICT, Json(extra)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::extract::{FromRequest, Request};
    use paigasus_iam_core::authz::model::PolicyKind;

    #[test]
    fn retire_body_defaults_acknowledge_to_false_when_the_field_is_absent() {
        let body: RetireBody = serde_json::from_str("{}").unwrap();
        assert!(!body.acknowledge_decision_change);
    }

    #[test]
    fn retire_body_honours_an_explicit_true() {
        let body: RetireBody = serde_json::from_str(r#"{"acknowledge_decision_change": true}"#).unwrap();
        assert!(body.acknowledge_decision_change);
    }

    #[test]
    fn acknowledged_collapses_absent_and_explicit_false_to_the_same_result() {
        assert!(!acknowledged(None), "no body at all must mean unacknowledged");
        assert!(!acknowledged(Some(RetireBody::default())), "an explicit false must mean unacknowledged");
        assert!(acknowledged(Some(RetireBody { acknowledge_decision_change: true })));
    }

    /// Exercises the REAL extractor axum runs for the handler's `body: Option<EnvelopeJson<
    /// RetireBody>>` parameter — not a hand-rolled stand-in — against a request with no body and
    /// no `Content-Type` header at all (the shape a bare `curl -X POST` with no `-d`/`-H`
    /// sends). `EnvelopeJson`'s `OptionalFromRequest` impl only yields `Ok(None)` when the
    /// header is absent (mirrors axum's own `Json<T>` behavior exactly); were this handler to
    /// instead require `EnvelopeJson<RetireBody>` directly, the SAME request would be rejected
    /// before ever reaching the service with a `415`/`400`, not the intended "unacknowledged"
    /// default. Uses `&()` as the extractor's state — the `Json`/`Option` extraction path never
    /// touches `AppState`, so no database or composition-root wiring is needed to prove this.
    #[tokio::test]
    async fn a_request_with_no_body_at_all_extracts_as_none_and_defaults_to_unacknowledged() {
        let req = Request::builder().method("POST").uri("/v1/authz/system-policies/legacy_auditor/retire").body(Body::empty()).unwrap();
        let extracted = <Option<EnvelopeJson<RetireBody>> as FromRequest<()>>::from_request(req, &())
            .await
            .expect("an absent body must never be a 400");
        assert!(extracted.is_none(), "no Content-Type header must yield None, never an attempt to parse zero bytes as JSON");
        assert!(!acknowledged(extracted.map(|EnvelopeJson(b)| b)));
    }

    /// The sibling case: a body that DOES declare JSON but omits the field entirely (`{}`) must
    /// also default to unacknowledged, via `#[serde(default)]` rather than the `Option` branch.
    #[tokio::test]
    async fn an_empty_json_object_body_also_defaults_to_unacknowledged() {
        let req = Request::builder()
            .method("POST")
            .uri("/v1/authz/system-policies/legacy_auditor/retire")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let extracted = <Option<EnvelopeJson<RetireBody>> as FromRequest<()>>::from_request(req, &())
            .await
            .expect("a valid empty JSON object must never be rejected");
        assert!(!acknowledged(extracted.map(|EnvelopeJson(b)| b)));
    }

    /// The third case, and the one an operator is most likely to trip over: declaring
    /// `Content-Type: application/json` while sending a GENUINELY empty body (`curl -X POST -H
    /// 'Content-Type: application/json'` with no `--data`). This is NOT the same request as the
    /// no-header case above and does NOT mean "unacknowledged" — the header commits the request
    /// to carrying JSON, and zero bytes is not valid JSON, so it is rejected. Pinned because
    /// `docs/ops/RUNBOOK-observability.md` now tells operators exactly this, and a runbook claim
    /// nothing enforces is one refactor away from being a lie.
    #[tokio::test]
    async fn declaring_json_then_sending_an_empty_body_is_rejected_not_treated_as_unacknowledged() {
        let req = Request::builder()
            .method("POST")
            .uri("/v1/authz/system-policies/legacy_auditor/retire")
            .header("content-type", "application/json")
            .body(Body::empty())
            .unwrap();
        let rejection = <Option<EnvelopeJson<RetireBody>> as FromRequest<()>>::from_request(req, &())
            .await
            .expect_err("Content-Type: application/json with zero bytes must be rejected, never silently unacknowledged");
        // Asserting the STATUS and the envelope, not merely that some error occurred: the runbook
        // promises operators a `400`, so a regression that changed it to a 415 or dropped the
        // stable envelope would still leave an `expect_err`-only test green.
        assert_eq!(rejection.status(), StatusCode::BAD_REQUEST, "the runbook documents a 400 for this exact request");
        let body = body_json(rejection).await;
        assert_eq!(body["error"]["code"], json!("invalid_request"));
        assert_eq!(body["error"]["message"], json!("invalid request body"));
    }

    /// The fix-round-flagged gap this now closes: a body that DOES declare
    /// `Content-Type: application/json` but fails to parse must render the crate's stable
    /// `{"error":{code,message}}` envelope (via `EnvelopeJson`), not axum's bare `JsonRejection`
    /// plain-text/differently-shaped body.
    #[tokio::test]
    async fn a_malformed_json_body_is_rejected_with_the_stable_error_envelope() {
        let req = Request::builder()
            .method("POST")
            .uri("/v1/authz/system-policies/legacy_auditor/retire")
            .header("content-type", "application/json")
            .body(Body::from("{not valid json"))
            .unwrap();
        let rejection = <Option<EnvelopeJson<RetireBody>> as FromRequest<()>>::from_request(req, &())
            .await
            .expect_err("malformed JSON must be rejected");
        assert_eq!(rejection.status(), StatusCode::BAD_REQUEST);
        let body = body_json(rejection).await;
        assert_eq!(body["error"]["code"], json!("invalid_request"));
        assert_eq!(body["error"]["message"], json!("invalid request body"));
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn grants_json_renders_every_field_of_every_grant() {
        let grants = vec![
            GrantRef {
                id: "g1".to_string(),
                principal_prn: "prn:iam::principal/00000000-0000-0000-0000-000000000001".to_string(),
                scope_prn: "prn:iam::root".to_string(),
            },
            GrantRef {
                id: "g2".to_string(),
                principal_prn: "prn:iam::principal/00000000-0000-0000-0000-000000000002".to_string(),
                scope_prn: "prn:iam::org/acme".to_string(),
            },
        ];
        let v = grants_json(&grants);
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[0]["id"], json!("g1"));
        assert_eq!(v[0]["principal_prn"], json!("prn:iam::principal/00000000-0000-0000-0000-000000000001"));
        assert_eq!(v[0]["scope_prn"], json!("prn:iam::root"));
        assert_eq!(v[1]["id"], json!("g2"));
    }

    /// Pins the stable envelope PLUS the sibling fields, so a refactor that dropped either the
    /// `error` object or the extra data would fail this test, not just a manual read.
    #[tokio::test]
    async fn conflict_keeps_the_stable_error_envelope_and_adds_sibling_fields() {
        let resp = conflict("grants-survive", "boom", json!({ "total_surviving": 3, "truncated": false }));
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], json!("grants-survive"));
        assert_eq!(body["error"]["message"], json!("boom"));
        assert_eq!(body["total_surviving"], json!(3));
        assert_eq!(body["truncated"], json!(false));
    }

    /// The fix-round finding this whole `response_for` extraction exists to close: without a
    /// test constructing a `RetireOutcome::Retired` directly and checking the response, a
    /// `StatusCode::OK` -> `StatusCode::NO_CONTENT` regression on the success arm passed the
    /// entire crate's test suite unchanged.
    #[tokio::test]
    async fn response_for_retired_is_200_with_what_was_destroyed() {
        let outcome = RetireOutcome::Retired {
            policy_id: "legacy_auditor".to_string(),
            kind: PolicyKind::Template,
            role_deleted: true,
        };
        let resp = response_for(outcome);
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "a successful retirement must be 200, never 204 — the body is the only immediate record of the delete"
        );
        let body = body_json(resp).await;
        assert_eq!(body["policy_id"], json!("legacy_auditor"));
        assert_eq!(body["kind"], json!("template"));
        assert_eq!(body["role_deleted"], json!(true));
    }

    /// `Retired` with no role row (a template with no linked role, or an acknowledged static
    /// retirement) must report `role_deleted: false`, not merely omit the field.
    #[tokio::test]
    async fn response_for_retired_reports_role_deleted_false_when_no_role_existed() {
        let outcome = RetireOutcome::Retired {
            policy_id: "legacy_forbid".to_string(),
            kind: PolicyKind::Static,
            role_deleted: false,
        };
        let body = body_json(response_for(outcome)).await;
        assert_eq!(body["kind"], json!("static"));
        assert_eq!(body["role_deleted"], json!(false));
    }

    /// `Blocked` -> 409 `grants-survive`, with the surviving grants, the true total (not the
    /// page length), `truncated`, AND the archived-scope guidance in the message — that
    /// sentence is the operator's only exit from a revoke-403/archived-scope refusal loop, so
    /// losing it silently would strand an operator exactly there.
    #[tokio::test]
    async fn response_for_blocked_is_409_with_grants_survive_and_the_archived_scope_guidance() {
        let grants = vec![GrantRef {
            id: "g1".to_string(),
            principal_prn: "prn:iam::principal/00000000-0000-0000-0000-000000000001".to_string(),
            scope_prn: "prn:iam::root".to_string(),
        }];
        let outcome = RetireOutcome::Blocked {
            role_key: "legacy_auditor".to_string(),
            grants,
            total: 3,
            truncated: false,
        };
        let resp = response_for(outcome);
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], json!("grants-survive"));
        let message = body["error"]["message"].as_str().expect("message must be a string");
        assert!(message.contains("legacy_auditor"));
        assert!(message.contains("3 grant"));
        assert!(
            message.contains("archived") && message.contains("re-archive"),
            "the message must carry the archived-scope guidance (restore/revoke/re-archive) — \
             an operator's only exit from a revoke-403 refusal loop: {message}"
        );
        assert_eq!(body["total_surviving"], json!(3));
        assert_eq!(body["truncated"], json!(false));
        assert_eq!(body["grants"][0]["id"], json!("g1"));
        assert_eq!(body["grants"][0]["scope_prn"], json!("prn:iam::root"));
    }

    /// `NeedsAcknowledgement` -> 409 `decision-change-unacknowledged`, carrying the exact
    /// content that would be destroyed (`kind`/`source`/`description`) — the refusal IS the
    /// preview, so losing any one of these fields would silently blind an operator deciding
    /// whether to re-send with `acknowledge_decision_change=true`.
    #[tokio::test]
    async fn response_for_needs_acknowledgement_is_409_with_the_content_that_would_be_destroyed() {
        let outcome = RetireOutcome::NeedsAcknowledgement {
            policy_id: "legacy_forbid".to_string(),
            kind: PolicyKind::Static,
            source: "forbid(principal, action, resource);".to_string(),
            description: "a retired guard".to_string(),
        };
        let resp = response_for(outcome);
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], json!("decision-change-unacknowledged"));
        assert!(body["error"]["message"].as_str().unwrap().contains("legacy_forbid"));
        assert!(body["error"]["message"].as_str().unwrap().contains("acknowledge_decision_change=true"));
        assert_eq!(body["kind"], json!("static"));
        assert_eq!(body["source"], json!("forbid(principal, action, resource);"));
        assert_eq!(body["description"], json!("a retired guard"));
    }
}
