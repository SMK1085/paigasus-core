// SPDX-License-Identifier: Apache-2.0

//! Outbox relay + event-publisher adapters (SMA-446, Slice B Task B8): the background drain
//! that turns committed `event_outbox` rows (written by `PgOutbox::enqueue` inside the
//! triggering mutation's own transaction, B2) into calls on an injected `EventPublisher`, plus a
//! `tracing`-backed `EventPublisher` implementation to inject. Wiring this into the composition
//! root (`main.rs`, alongside `spawn_reload`/the denial-audit drain) is a later task (B9).

pub mod relay;
pub mod tracing_publisher;

pub use relay::{OutboxRelay, TickReport};
pub use tracing_publisher::TracingEventPublisher;
