// SPDX-License-Identifier: Apache-2.0

//! An in-process stub of the `Iam` port for the HTTP route tests. It answers at the trait level,
//! so it starts no gRPC server. It records each call by name, and it records every self-query as
//! `(credential, principal PRN, action, org PRN)`. It can grant per org and per action. (A tonic
//! fake IAM server is a separate file, `fake_iam.rs`, for the tests that need a real gRPC link.)

use std::sync::Mutex;

use async_trait::async_trait;
use paigasus_agent_host::adapters::iam::{CallerCredential, Iam, IamError};
use paigasus_proto::paigasus::common::v1::ErrorReason;
use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, IntrospectResponse};
use tonic::{Code, Status};
use tonic_types::{ErrorDetails, StatusExt};
use uuid::Uuid;

/// The token that the tests send.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub const TOKEN: &str = "caller-secret";

/// One answer of a leg: a message, or a gRPC `Status` with an optional IAM reason.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
#[derive(Clone)]
pub enum Leg<T> {
    Ok(T),
    Rpc(Code, Option<ErrorReason>),
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
impl<T: Clone> Leg<T> {
    fn answer(&self) -> Result<T, IamError> {
        match self {
            Leg::Ok(v) => Ok(v.clone()),
            Leg::Rpc(code, None) => Err(IamError::Rpc(Status::new(*code, "stub"))),
            Leg::Rpc(code, Some(reason)) => Err(IamError::Rpc(Status::with_error_details(
                *code,
                "stub",
                ErrorDetails::with_error_info(
                    reason.as_wire_reason().expect("a declared reason"),
                    &*paigasus_proto::error::IAM_DOMAIN,
                    std::collections::HashMap::new(),
                ),
            ))),
        }
    }
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub struct StubIam {
    key: Leg<IntrospectApiKeyResponse>,
    token: Leg<IntrospectResponse>,
    allowed: Leg<bool>,
    /// When set, the self-query answers from this table and not from `allowed`. An error leg of
    /// `allowed` still wins, so an outage stays an outage.
    grants: Option<Grants>,
    calls: Mutex<Vec<&'static str>>,
    self_queries: Mutex<Vec<SelfQuery>>,
}

/// One self-query as IAM received it: `(credential, principal PRN, action, org PRN)`.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub type SelfQuery = (CallerCredential, String, String, String);

/// The actions that a self-query is granted on one org.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
#[derive(Clone)]
struct Grants {
    org_prn: String,
    actions: &'static [&'static str],
}

/// The PRN of an organization.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub fn org_prn(id: Uuid) -> String {
    format!("prn:pgs:iam:::organization/{id}")
}

/// The PRN of a principal.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub fn principal_prn(id: Uuid) -> String {
    format!("prn:pgs:iam:::principal/{id}")
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
impl StubIam {
    fn new(key: Leg<IntrospectApiKeyResponse>, token: Leg<IntrospectResponse>) -> Self {
        StubIam {
            key,
            token,
            allowed: Leg::Ok(true),
            grants: None,
            calls: Mutex::new(Vec::new()),
            self_queries: Mutex::new(Vec::new()),
        }
    }

    /// An active API key of `principal`, issued under the Root scope. The self-query allows.
    pub fn api_key(principal: Uuid) -> Self {
        StubIam::new(
            Leg::Ok(IntrospectApiKeyResponse {
                status: "active".to_owned(),
                principal_prn: principal_prn(principal),
                scope_prn: "prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000".to_owned(),
                key_id: "key-1".to_owned(),
                ..Default::default()
            }),
            Leg::Rpc(Code::Unauthenticated, Some(ErrorReason::InvalidToken)),
        )
    }

    /// A token that IAM validates and does not provision.
    pub fn unprovisioned() -> Self {
        StubIam::new(
            Leg::Rpc(Code::Unauthenticated, Some(ErrorReason::InvalidToken)),
            Leg::Rpc(Code::PermissionDenied, Some(ErrorReason::IdentityNotProvisioned)),
        )
    }

    /// IAM cannot be reached on any call.
    pub fn outage() -> Self {
        StubIam::new(Leg::Rpc(Code::Unavailable, None), Leg::Rpc(Code::Unavailable, None))
    }

    /// The answer of the self-query.
    pub fn allowing(mut self, allowed: bool) -> Self {
        self.allowed = Leg::Ok(allowed);
        self
    }

    /// Grant `actions` on `org` only. Every other action or org is denied. An outage set by
    /// [`StubIam::self_query_outage`] still wins.
    pub fn granting(mut self, org: Uuid, actions: &'static [&'static str]) -> Self {
        self.grants = Some(Grants { org_prn: org_prn(org), actions });
        self
    }

    /// Change the scope PRN of the API key.
    pub fn with_key_scope(mut self, scope_prn: &str) -> Self {
        if let Leg::Ok(resp) = &mut self.key {
            resp.scope_prn = scope_prn.to_owned();
        }
        self
    }

    /// Introspection works, and the self-query finds IAM unavailable.
    pub fn self_query_outage(mut self) -> Self {
        self.allowed = Leg::Rpc(Code::Unavailable, None);
        self
    }

    /// The names of the calls, in order.
    pub fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }

    /// Every self-query, in order: `(credential, principal PRN, action, org PRN)`.
    pub fn self_query_log(&self) -> Vec<SelfQuery> {
        self.self_queries.lock().unwrap().clone()
    }

    /// Every self-query, in order, as `(principal PRN, action, org PRN)`.
    pub fn self_query_args(&self) -> Vec<(String, String, String)> {
        self.self_query_log().into_iter().map(|(_, principal, action, org)| (principal, action, org)).collect()
    }

    /// How many self-queries ran.
    pub fn self_queries(&self) -> usize {
        self.calls().iter().filter(|c| **c == "is_authorized_self").count()
    }
}

#[async_trait]
impl Iam for StubIam {
    async fn introspect_api_key(&self, token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
        assert_eq!(token, TOKEN);
        self.calls.lock().unwrap().push("introspect_api_key");
        self.key.answer()
    }

    async fn introspect_token(&self, token: &str) -> Result<IntrospectResponse, IamError> {
        assert_eq!(token, TOKEN);
        self.calls.lock().unwrap().push("introspect_token");
        self.token.answer()
    }

    async fn is_authorized_self(&self, caller: &CallerCredential, principal_prn: &str, action: &str, resource_prn: &str) -> Result<bool, IamError> {
        self.calls.lock().unwrap().push("is_authorized_self");
        self.self_queries
            .lock()
            .unwrap()
            .push((caller.clone(), principal_prn.to_owned(), action.to_owned(), resource_prn.to_owned()));
        let allowed = self.allowed.answer()?;
        match &self.grants {
            Some(g) => Ok(g.actions.contains(&action) && g.org_prn == resource_prn),
            None => Ok(allowed),
        }
    }
}
