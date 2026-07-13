// SPDX-License-Identifier: Apache-2.0

//! Shared observability plumbing for Paigasus services: a global `metrics`-facade Prometheus
//! recorder, a `GET /metrics` router, an axum request-metrics layer, a gRPC handler helper, and
//! the canonical metric-name registry. Mirrors `paigasus-logging`'s role for tracing.
