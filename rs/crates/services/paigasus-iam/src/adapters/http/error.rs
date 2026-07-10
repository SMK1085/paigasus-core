// SPDX-License-Identifier: Apache-2.0

//! Maps `TenancyError` into the HTTP surface's stable error contract: a status code by
//! `ErrorClass` and a `{"error": {"code", "message"}}` JSON body. `Internal` never leaks
//! its source — it logs and returns a generic message (D-per brief: never leak details).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::application::error::{ErrorClass, TenancyError};

/// Wraps a `TenancyError` so handlers can return it directly via `?` (see the blanket
/// `From` impl below) and axum turns it into a JSON error response.
pub struct ApiError(pub TenancyError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0.class() {
            ErrorClass::Validation => StatusCode::BAD_REQUEST,
            ErrorClass::NotFound => StatusCode::NOT_FOUND,
            ErrorClass::Conflict | ErrorClass::Precondition => StatusCode::CONFLICT,
            ErrorClass::Forbidden => StatusCode::FORBIDDEN,
            ErrorClass::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let message = if matches!(self.0.class(), ErrorClass::Internal) {
            tracing::error!(error = %self.0, code = self.0.code(), "internal error handling HTTP request");
            "internal error".to_string()
        } else {
            self.0.to_string()
        };

        (status, Json(json!({ "error": { "code": self.0.code(), "message": message } }))).into_response()
    }
}

/// Any error the application layer can hand back (`TenancyError` itself, or anything that
/// converts into one) becomes an `ApiError` — lets handlers use `?` against
/// `Result<_, TenancyError>` service calls directly.
impl<E: Into<TenancyError>> From<E> for ApiError {
    fn from(err: E) -> Self {
        ApiError(err.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn maps_classes_to_status_and_body() {
        let resp = ApiError(TenancyError::SlugConflict).into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "slug-conflict");

        let resp = ApiError(TenancyError::NotFound).into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let resp = ApiError(TenancyError::NothingToRename).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let resp = ApiError(TenancyError::ParentArchived).into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn forbidden_maps_to_403_with_generic_body_and_no_challenge() {
        let resp = ApiError(TenancyError::Forbidden).into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(
            resp.headers().get(axum::http::header::WWW_AUTHENTICATE).is_none(),
            "403 forbidden must not carry a WWW-Authenticate challenge (that's a 401 concern)"
        );
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "forbidden");
        assert_eq!(body["error"]["message"], "access denied");
    }

    #[tokio::test]
    async fn internal_errors_never_leak_details() {
        let resp = ApiError(TenancyError::Internal).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "internal");
        assert_eq!(body["error"]["message"], "internal error");
    }
}
