// SPDX-License-Identifier: Apache-2.0

//! A uuid path-segment extractor that answers inside IAM's error envelope (SMA-586 D5.1).
//!
//! axum's own `Path<Uuid>` rejects a malformed segment with a plain-text 400 that is not the
//! `{"error":{code,message}}` contract every other IAM failure uses — so 26 routes answered
//! outside their own error contract, and `invalid-uuid` had no HTTP emitter at all while its
//! gRPC twins did (AC-1). This closes both.
//!
//! Mirrors `authn::EnvelopeJson`, which already does exactly this for `Json<T>` rejections.
//!
//! The field name is carried by a MARKER TYPE rather than a request extension: `&'static str`
//! is not a stable const-generic parameter, and an extension set by a route-level layer would
//! let a route that forgets the layer report a wrong name at runtime. A marker puts the name
//! in the handler signature, so a route cannot compile without choosing one.

use std::marker::PhantomData;

use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::adapters::http::error::ApiError;
use crate::application::error::TenancyError;

/// Names the wire field a uuid path segment stands for, for `TenancyError::InvalidUuid`.
pub(crate) trait PathField {
    const NAME: &'static str;
}

/// Declares a marker type and its wire field name.
macro_rules! path_field {
    ($(#[$m:meta])* $name:ident => $wire:literal) => {
        $(#[$m])*
        pub(crate) struct $name;
        impl PathField for $name {
            const NAME: &'static str = $wire;
        }
    };
}

path_field!(/// `{id}` on an organization route.
    OrganizationId => "organization_id");
path_field!(/// `{id}` or `{team_id}` on a team route.
    TeamId => "team_id");
path_field!(/// `{id}` on a project route.
    ProjectId => "project_id");
path_field!(/// `{sa}` — a service account's bare uuid.
    ServiceAccountId => "service_account_id");
path_field!(/// `{id}` on an api-key route.
    ApiKeyId => "api_key_id");
path_field!(/// `{id}` on a membership route.
    MembershipId => "membership_id");
path_field!(/// `{id}` on a role-grant route.
    RoleGrantId => "role_grant_id");
path_field!(/// `{id}` on a dead-letter route.
    DeadLetterId => "dead_letter_id");

/// A single uuid path segment, reported as `F::NAME` when it is malformed.
pub(crate) struct UuidPath<F: PathField> {
    pub id: Uuid,
    _marker: PhantomData<F>,
}

impl<S: Send + Sync, F: PathField> FromRequestParts<S> for UuidPath<F> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<Uuid>::from_request_parts(parts, state).await {
            Ok(Path(id)) => Ok(UuidPath { id, _marker: PhantomData }),
            Err(_) => Err(ApiError(TenancyError::InvalidUuid(F::NAME)).into_response()),
        }
    }
}

/// Two uuid path segments, for `/{sa}/api-keys/{id}`.
///
/// Both are reported under the SAME field name: axum's `PathRejection` does not say WHICH
/// segment failed, and inventing one would be a guess presented as fact.
pub(crate) struct UuidPathPair<F: PathField> {
    pub first: Uuid,
    pub second: Uuid,
    _marker: PhantomData<F>,
}

impl<S: Send + Sync, F: PathField> FromRequestParts<S> for UuidPathPair<F> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<(Uuid, Uuid)>::from_request_parts(parts, state).await {
            Ok(Path((first, second))) => Ok(UuidPathPair { first, second, _marker: PhantomData }),
            Err(_) => Err(ApiError(TenancyError::InvalidUuid(F::NAME)).into_response()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::routing::get;
    use tower::ServiceExt;

    async fn ok(path: UuidPath<MembershipId>) -> String {
        path.id.to_string()
    }

    /// SMA-586 D5.1: a malformed uuid path segment answers inside the error envelope, with
    /// `invalid-uuid`. Before this, axum's own `Path<Uuid>` rejection produced a plain-text
    /// 400 that was not the `{"error":{code,message}}` contract at all — on all 26 routes.
    #[tokio::test]
    async fn a_malformed_uuid_segment_answers_in_the_error_envelope() {
        // Reason compared via `as_wire_reason()`, never a bare kebab literal — a literal in a
        // `src/` file would put this production module on `ci/error-registry/check.py`'s
        // MANIFEST, blinding that gate to a future *production* code literal in this file.
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        let wire = ErrorReason::InvalidUuid.as_wire_reason().expect("not the Unspecified sentinel");

        let app = Router::new().route("/x/{id}", get(ok));
        let resp = app.oneshot(axum::http::Request::builder().uri("/x/not-a-uuid").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], wire);
        assert_eq!(body["error"]["message"], "membership_id must be a uuid");
    }

    /// A well-formed uuid still reaches the handler unchanged.
    #[tokio::test]
    async fn a_well_formed_uuid_segment_extracts() {
        let id = uuid::Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
        let app = Router::new().route("/x/{id}", get(ok));
        let resp = app
            .oneshot(axum::http::Request::builder().uri(format!("/x/{id}")).body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), id.to_string());
    }

    /// Every marker's NAME is the literal the route's error should carry. Pinned so a rename
    /// that drifts from the URL segment it describes fails here.
    #[test]
    fn the_path_field_names_are_stable() {
        assert_eq!(OrganizationId::NAME, "organization_id");
        assert_eq!(TeamId::NAME, "team_id");
        assert_eq!(ProjectId::NAME, "project_id");
        assert_eq!(ServiceAccountId::NAME, "service_account_id");
        assert_eq!(ApiKeyId::NAME, "api_key_id");
        assert_eq!(MembershipId::NAME, "membership_id");
        assert_eq!(RoleGrantId::NAME, "role_grant_id");
        assert_eq!(DeadLetterId::NAME, "dead_letter_id");
    }
}
