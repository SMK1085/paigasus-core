// SPDX-License-Identifier: Apache-2.0
//! One-line gRPC handler-boundary instrumentation (static labels; status from the Result).

use std::time::Instant;

use metrics::{counter, histogram};

/// Record a completed tonic handler call. `service`/`method` are compile-time literals (never
/// `:path`-derived — bounded cardinality); `grpc_status` is `"ok"` or the canonical code name.
pub fn record_grpc<T>(service: &'static str, method: &'static str, started: Instant, result: &Result<T, tonic::Status>) {
    let grpc_status = match result {
        Ok(_) => "ok",
        Err(status) => grpc_code_name(status.code()),
    };
    counter!(
        "iam_grpc_requests_total",
        "service" => service,
        "method" => method,
        "grpc_status" => grpc_status
    )
    .increment(1);
    histogram!(
        "iam_grpc_request_duration_seconds",
        "service" => service,
        "method" => method
    )
    .record(started.elapsed().as_secs_f64());
}

fn grpc_code_name(code: tonic::Code) -> &'static str {
    use tonic::Code::*;
    match code {
        Ok => "ok",
        Cancelled => "cancelled",
        Unknown => "unknown",
        InvalidArgument => "invalid_argument",
        DeadlineExceeded => "deadline_exceeded",
        NotFound => "not_found",
        AlreadyExists => "already_exists",
        PermissionDenied => "permission_denied",
        ResourceExhausted => "resource_exhausted",
        FailedPrecondition => "failed_precondition",
        Aborted => "aborted",
        OutOfRange => "out_of_range",
        Unimplemented => "unimplemented",
        Internal => "internal",
        Unavailable => "unavailable",
        DataLoss => "data_loss",
        Unauthenticated => "unauthenticated",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init;
    use std::time::Instant;

    #[test]
    fn records_static_labels_and_status_from_result() {
        let handle = init("test-svc");
        let ok: Result<(), tonic::Status> = Ok(());
        record_grpc("Authorization", "IsAuthorized", Instant::now(), &ok);
        let err: Result<(), tonic::Status> = Err(tonic::Status::permission_denied("no"));
        record_grpc("Authorization", "IsAuthorized", Instant::now(), &err);
        let out = handle.render();
        assert!(out.contains("iam_grpc_requests_total"));
        assert!(out.contains("service=\"Authorization\""));
        assert!(out.contains("method=\"IsAuthorized\""));
        assert!(out.contains("grpc_status=\"ok\""));
        assert!(out.contains("grpc_status=\"permission_denied\""));
    }
}
