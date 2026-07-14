// SPDX-License-Identifier: Apache-2.0
//! One-line gRPC handler-boundary instrumentation (static labels; status from the Result).

use std::time::Instant;

use metrics::{counter, describe_counter, describe_histogram, histogram};

/// Registers `# HELP`/`# TYPE` exposition text for the two IAM gRPC families (spec §4.1). Callers
/// (currently only `paigasus-iam`) invoke this once, after `crate::init` has installed the global
/// recorder — a `describe_*!` call before a recorder is installed is a harmless no-op (the
/// `metrics` facade only forwards it once a recorder exists), so this has no ordering requirement
/// beyond "after `init`, when metrics are enabled."
pub fn describe_grpc() {
    describe_counter!(
        crate::names::IAM_GRPC_REQUESTS_TOTAL,
        "Completed tonic gRPC handler calls, labeled by service, method, and grpc_status."
    );
    describe_histogram!(
        crate::names::IAM_GRPC_REQUEST_DURATION_SECONDS,
        "gRPC handler latency in seconds, recorded at the same handler-boundary call site as the request counter."
    );
}

/// Record a completed tonic handler call. `service`/`method` are compile-time literals (never
/// `:path`-derived — bounded cardinality); `grpc_status` is `"ok"` or the canonical code name.
pub fn record_grpc<T>(service: &'static str, method: &'static str, started: Instant, result: &Result<T, tonic::Status>) {
    let grpc_status = match result {
        Ok(_) => "ok",
        Err(status) => grpc_code_name(status.code()),
    };
    counter!(
        crate::names::IAM_GRPC_REQUESTS_TOTAL,
        "service" => service,
        "method" => method,
        "grpc_status" => grpc_status
    )
    .increment(1);
    histogram!(
        crate::names::IAM_GRPC_REQUEST_DURATION_SECONDS,
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
        assert!(out.contains("iam_grpc_request_duration_seconds"));
        assert!(out.contains("service=\"Authorization\""));
        assert!(out.contains("method=\"IsAuthorized\""));
        assert!(out.contains("grpc_status=\"ok\""));
        assert!(out.contains("grpc_status=\"permission_denied\""));
    }

    #[test]
    fn describe_grpc_registers_help_text_rendered_in_the_exposition() {
        let handle = init("test-svc");
        describe_grpc();
        let ok: Result<(), tonic::Status> = Ok(());
        record_grpc("Authorization", "IsAuthorized", Instant::now(), &ok);
        let out = handle.render();
        assert!(out.contains("# HELP iam_grpc_requests_total"), "expected HELP line for iam_grpc_requests_total:\n{out}");
        assert!(out.contains("# TYPE iam_grpc_requests_total counter"), "expected TYPE line for iam_grpc_requests_total:\n{out}");
        assert!(
            out.contains("# HELP iam_grpc_request_duration_seconds"),
            "expected HELP line for iam_grpc_request_duration_seconds:\n{out}"
        );
    }
}
