// SPDX-License-Identifier: Apache-2.0

//! The error envelope of the HTTP surface: `{"error":{"code":"…","message":"…"}}`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// The registered code and the message of the 404 answer. The membership test below checks the
/// code against the registry in `common/v1/error.proto`.
pub const NOT_FOUND: (&str, &str) = ("not-found", "no route matches this path");

/// The fallback of the router: 404 in the error envelope.
pub async fn not_found() -> Response {
    let (code, message) = NOT_FOUND;
    (StatusCode::NOT_FOUND, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_not_found_code_is_in_the_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        let (code, _message) = NOT_FOUND;
        assert_eq!(ErrorReason::from_wire_reason(code), Some(ErrorReason::NotFound), "{code} is not declared in common/v1/error.proto");
    }
}
