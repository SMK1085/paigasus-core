// SPDX-License-Identifier: Apache-2.0

//! `ServiceInfoService`: IAM's capability descriptor over gRPC (ADR-0020, SMA-505).
//!
//! Bearer-enforced automatically — `AuthLayer` covers every `:path` absent from
//! `grpc::authn::is_exempt`, and this one is deliberately not added there. No authorization
//! action is checked, matching the HTTP route.
//!
//! Shares `AppState.capabilities.descriptor()` with `adapters::http::service_info`, so the two
//! transports cannot describe different builds (pinned by `tests/grpc_service_info.rs`).

use std::time::Instant;

use paigasus_observability::record_grpc;
use paigasus_proto::paigasus::common::v1::service_info_service_server::ServiceInfoService;
use paigasus_proto::paigasus::common::v1::{GetServiceInfoRequest, GetServiceInfoResponse};
use tonic::{Request, Response, Status};

use crate::adapters::http::AppState;

pub struct ServiceInfoGrpc {
    state: AppState,
}

impl ServiceInfoGrpc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl ServiceInfoService for ServiceInfoGrpc {
    /// `service_info` is ALWAYS populated. The proto requires clients to treat an absent
    /// `service_info` as an error rather than as "no capabilities", so a `None` here would be
    /// a server bug, not a representable state.
    async fn get_service_info(&self, _request: Request<GetServiceInfoRequest>) -> Result<Response<GetServiceInfoResponse>, Status> {
        let started = Instant::now();
        let result = Ok(Response::new(GetServiceInfoResponse {
            service_info: Some(self.state.capabilities.descriptor()),
        }));
        record_grpc("ServiceInfo", "GetServiceInfo", started, &result);
        result
    }
}
