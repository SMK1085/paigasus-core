// SPDX-License-Identifier: Apache-2.0

//! `GET /v1/service-info` — the gateway's capability descriptor (ADR-0020, SMA-505).
//!
//! Protected by [`super::auth::require_authenticated`], NOT by `require_iam_auth`: discovery
//! requires a valid credential but no authorization action. The gateway serves the descriptor
//! over HTTP only — it has no tonic server, and giving it one would mean a second listening
//! port plus Helm and ingress entries for every self-hoster (SMA-499 D3).

use axum::{Json, extract::State};
use paigasus_service_info::ServiceInfoDto;

use super::AppState;
use crate::service_info::Capabilities;

pub async fn get_service_info(State(state): State<AppState>) -> Json<ServiceInfoDto> {
    let caps = Capabilities { chat_stream: state.stream_enabled };
    Json(ServiceInfoDto::from(&caps.descriptor()))
}
