// SPDX-License-Identifier: Apache-2.0

//! Outbox relay + event-publisher adapters (SMA-446, Slice B Task B8): the background drain
//! that turns committed `event_outbox` rows (written by `PgOutbox::enqueue` inside the
//! triggering mutation's own transaction, B2) into calls on an injected `EventPublisher`, plus
//! the `EventPublisher` implementations to inject.
//!
//! Two publishers ship here, selected by `[outbox.publisher].backend`:
//! [`NatsEventPublisher`] is the production sink (SMA-471, ADR-0016) — it renders each row as a
//! [`CloudEvent`] and publishes it to NATS JetStream, waiting for the persistence ack — and
//! [`TracingEventPublisher`] is the broker-less default, which logs instead of delivering.
//! `tracing` remains the default so a deployment with no broker configured still boots.

pub mod cloud_event;
pub mod nats_publisher;
pub mod relay;
pub mod tracing_publisher;

pub use cloud_event::{CloudEvent, render_id};
pub use nats_publisher::{NatsEventPublisher, NatsPublisherError};
pub use relay::{OutboxRelay, TickReport};
pub use tracing_publisher::TracingEventPublisher;
