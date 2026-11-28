// SPDX-License-Identifier: Apache-2.0

//! The bodies, the queries and the answers of the REST agent API (SMA-852 spec §6.5).

use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use axum::response::{IntoResponse, Response};
use paigasus_agent_host_core::registry::{AgentFields, AgentRecord, InboxRecord};
use paigasus_kernel::resource_name::Prn;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use uuid::Uuid;

use super::error::AgentApiError;
use crate::adapters::http::error::ApiError;
use crate::application::agents::AdminError;
use crate::application::auth::{IAM_SERVICE, ORGANIZATION};

/// The body of `POST /v1/agents`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBody {
    pub org: String,
    pub name: String,
    pub model: String,
    pub instructions: String,
    pub tools: Vec<String>,
}

impl CreateBody {
    /// The fields that the caller sets.
    pub fn fields(&self) -> AgentFields {
        fields(&self.name, &self.model, &self.instructions, &self.tools)
    }
}

/// The body of `PATCH /v1/agents/{agent}`: the version that the caller read and a full replace.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateBody {
    pub version: u64,
    pub name: String,
    pub model: String,
    pub instructions: String,
    pub tools: Vec<String>,
}

impl UpdateBody {
    /// The fields that the caller sets.
    pub fn fields(&self) -> AgentFields {
        fields(&self.name, &self.model, &self.instructions, &self.tools)
    }
}

/// The body of `POST /v1/agents/{agent}/inboxes`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindBody {
    pub principal: String,
}

/// The query of `GET /v1/agents`. Each value is read by hand, so a bad value is an answer in the
/// error envelope.
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    pub org: Option<String>,
    pub include_archived: Option<String>,
    pub page_size: Option<String>,
    pub page_token: Option<String>,
}

/// The query of `GET /v1/agents/{agent}/inboxes`.
#[derive(Debug, Default, Deserialize)]
pub struct PageQuery {
    pub page_size: Option<String>,
    pub page_token: Option<String>,
}

fn fields(name: &str, model: &str, instructions: &str, tools: &[String]) -> AgentFields {
    AgentFields {
        name: name.to_owned(),
        model_id: model.to_owned(),
        instructions: instructions.to_owned(),
        tool_names: tools.to_vec(),
    }
}

/// The page size of a query. Absent is `0`, which means the default. A value that is not a
/// non-negative integer is [`AdminError::InvalidPagination`].
pub fn page_size(raw: Option<&str>) -> Result<u32, AgentApiError> {
    raw.map_or(Ok(0), |v| v.parse::<u32>().map_err(|_| AgentApiError(AdminError::InvalidPagination)))
}

/// The `include_archived` flag of a query. Absent is `false`. Any other value than `true` or
/// `false` is [`AdminError::InvalidQueryParameter`], as a duplicate key is.
pub fn include_archived(raw: Option<&str>) -> Result<bool, AgentApiError> {
    match raw {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(AgentApiError(AdminError::InvalidQueryParameter)),
    }
}

/// The org of an org PRN. Any other PRN, and a string that is no PRN, is the invalid field `org`.
pub fn parse_org_prn(raw: &str) -> Result<Uuid, AgentApiError> {
    let invalid = || AgentApiError(AdminError::InvalidField("org"));
    let prn = Prn::parse(raw).map_err(|_| invalid())?;
    if prn.service() != IAM_SERVICE || !prn.region().is_empty() || prn.org().is_some() || prn.resource_type() != ORGANIZATION {
        return Err(invalid());
    }
    Ok(prn.resource_id())
}

/// The PRN of an org.
pub(crate) fn org_prn(org: Uuid) -> String {
    Prn::build(IAM_SERVICE, "", None, ORGANIZATION, org).expect("static organization PRN parts are valid").canonical()
}

/// A JSON body, read by hand. A body that is not JSON is `invalid-request-body`, JSON of the wrong
/// shape is `invalid-request-schema`, and a body over the limit is `request-too-large`. Each one
/// answers in the error envelope.
pub struct JsonBody<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for JsonBody<T> {
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state).await.map_err(|rejection| {
            if rejection.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
                ApiError::RequestTooLarge.into_response()
            } else {
                ApiError::BadRequestBody.into_response()
            }
        })?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| AgentApiError(AdminError::InvalidRequestBody).into_response())?;
        serde_json::from_value(value).map(JsonBody).map_err(|_| AgentApiError(AdminError::InvalidRequestSchema).into_response())
    }
}

/// The JSON answer of an agent.
pub fn agent_json(record: &AgentRecord) -> Value {
    let def = &record.definition;
    json!({
        "agent": def.id.as_prn().canonical(),
        "org": org_prn(def.id.org()),
        "name": def.name,
        "model": def.model_id,
        "instructions": def.instructions,
        "tools": def.tool_names,
        "version": record.version,
        "created_at": record.created_at.to_string(),
        "updated_at": record.updated_at.to_string(),
        "archived_at": record.archived_at.map(|t| t.to_string()),
    })
}

/// The JSON answer of an inbox.
pub fn inbox_json(record: &InboxRecord) -> Value {
    let inbox = &record.inbox;
    json!({
        "inbox": inbox.id().as_prn().canonical(),
        "agent": inbox.agent().as_prn().canonical(),
        "principal": inbox.principal().as_prn().canonical(),
        "created_at": inbox.created_at().to_string(),
        "agent_archived_at": record.agent_archived_at.map(|t| t.to_string()),
    })
}

/// A page: `{"items":[…],"next_page_token":"…"|null}`.
pub fn page_json(items: Vec<Value>, next: Option<String>) -> Value {
    json!({ "items": items, "next_page_token": next })
}
