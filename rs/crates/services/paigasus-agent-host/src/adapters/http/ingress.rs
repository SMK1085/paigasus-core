// SPDX-License-Identifier: Apache-2.0

//! `POST /v1/inboxes/{inbox}/events` (spec §5.2).
//!
//! The layers, from the outside in: [`precheck`] (the content type, then `Content-Length`),
//! [`authenticate_middleware`] (puts the [`Caller`] in the extensions), and the handler under a
//! `DefaultBodyLimit`. So a large `Content-Length` is a 413 before IAM is called. A chunked body
//! that grows too large is a 413 when the handler reads it, after authentication.

use std::sync::Arc;

use axum::Extension;
use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use paigasus_agent_host_core::ids::{EventId, ThreadId};
use paigasus_agent_host_core::inbox::NewInboxEvent;
use paigasus_agent_host_core::ports::{InboxRepository, InsertOutcome};
use paigasus_cloudevents::{ExtensionValue, ParseError};
use serde_json::json;

use super::Ingress;
use super::auth::{Caller, authenticate_request, authorize_write, deny_unknown_inbox};
use super::bytes::EnvelopeBytes;
use super::error::ApiError;
use super::path::{InboxIdField, UuidPath};
use crate::adapters::id::mint_uuid;
use crate::application::ingest::{Channel, ingest};

/// The only content type of the route.
const CLOUDEVENTS_JSON: &str = "application/cloudevents+json";
/// The prefix of the types that the host makes. A caller cannot send it.
const RESERVED_TYPE_PREFIX: &str = "agents.";
/// The start of the `source` that the host sets.
const SOURCE_PREFIX: &str = "urn:paigasus:agents:http:";

/// The state of the events handler.
#[derive(Clone)]
pub struct IngressState {
    pub inboxes: Arc<dyn InboxRepository>,
    pub ingress: Arc<Ingress>,
}

/// Is this `Content-Type` value the CloudEvents JSON type? The comparison ignores case. A
/// parameter such as `charset=utf-8` is allowed.
fn is_cloudevents_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case(CLOUDEVENTS_JSON))
}

/// Steps 1 and 2 of §5.2: the content type, then the declared body size. Both run before
/// authentication.
pub async fn precheck(State(ingress): State<Arc<Ingress>>, req: Request, next: Next) -> Response {
    if !is_cloudevents_json(req.headers()) {
        return ApiError::UnsupportedContentType.into_response();
    }
    let declared = req.headers().get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > ingress.max_body_bytes as u64) {
        return ApiError::RequestTooLarge.into_response();
    }
    next.run(req).await
}

/// Step 3 of §5.2: authenticate through IAM and put the [`Caller`] in the request extensions.
pub async fn authenticate_middleware(State(ingress): State<Arc<Ingress>>, req: Request, next: Next) -> Response {
    authenticate_request(&*ingress.iam, req, next).await
}

/// Steps 4 to 8 of §5.2.
pub async fn post_event(
    State(state): State<IngressState>,
    UuidPath { id: inbox_uuid, .. }: UuidPath<InboxIdField>,
    Extension(caller): Extension<Caller>,
    EnvelopeBytes(body): EnvelopeBytes,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    // 4. The inbox lookup. An unknown inbox and a refused caller get the same answer (step 5):
    // the same IAM calls, `403`, and `503` during an IAM outage.
    let inbox = state.inboxes.get_by_uuid(inbox_uuid).await.map_err(|e| {
        tracing::error!(error = %e, "the inbox lookup failed");
        ApiError::StoreUnavailable
    })?;
    let Some(inbox) = inbox else {
        return Err(deny_unknown_inbox(&*state.ingress.iam, &caller).await);
    };

    // 5. Authorisation.
    authorize_write(&*state.ingress.iam, &caller, &inbox).await?;

    // 6. Parse and validate.
    let mut event = paigasus_cloudevents::parse(&body).map_err(|e| match e {
        ParseError::Json(_) => ApiError::BadRequestBody,
        ParseError::Schema(_) => ApiError::InvalidRequestSchema,
    })?;
    if event.ty().starts_with(RESERVED_TYPE_PREFIX) {
        return Err(ApiError::InvalidRequestSchema);
    }

    // 7. The fields that the host sets.
    let prn = caller.principal.as_prn().canonical();
    event.set_source(&format!("{SOURCE_PREFIX}{prn}")).map_err(|_| ApiError::InvalidRequestSchema)?;
    event.set_extension("actorprn", ExtensionValue::String(prn)).map_err(|_| ApiError::InvalidRequestSchema)?;
    // The raw `threadid` stays in the envelope. The repository drops a thread of another inbox.
    let thread = event.threadid().and_then(|raw| ThreadId::parse(raw).ok());

    let new_event = NewInboxEvent {
        id: EventId::from_uuid(mint_uuid()),
        inbox: inbox.id().clone(),
        source: event.source().to_owned(),
        ce_id: event.id().to_owned(),
        ce_type: event.ty().to_owned(),
        thread,
        envelope: event.to_json(),
    };

    // 8. The insert. A duplicate returns the stored id.
    let outcome = ingest(&*state.inboxes, new_event, Channel::Http).await.map_err(|e| {
        tracing::error!(error = %e, "the event insert failed");
        ApiError::StoreUnavailable
    })?;
    let id = match outcome {
        InsertOutcome::Inserted { id } | InsertOutcome::Duplicate { id, .. } => id,
        // The inbox has no active agent: the agent is archived.
        InsertOutcome::AgentArchived => return Err(ApiError::AgentArchived),
    };
    Ok((StatusCode::ACCEPTED, Json(json!({ "id": id.uuid().to_string() }))))
}
