# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A maintainer writes this file by hand. release-plz does not process this crate, because its Cargo
manifest sets `publish = false`.

## [Unreleased]

### Added

- The CloudEvents ingress `POST /v1/inboxes/{inbox}/events`. It is mounted only when `[iam]` is set.
- The inbox tables `thread`, `inbox_lease`, `run`, `run_event` and `inbox_event`.
- The inbox lease, the lease heartbeat and the claim of a batch of events.
- The NATS JetStream source of the inbox, with the `[nats]` section and its channels.
- The route `GET /v1/service-info`.
- The config keys `worker`, `lease`, `claim`, `ingress`, `iam`, `mid_run` and `nats`.
- The `timer` table (migration `m0004_create_timer`), for follow-ups and cron schedules.
- The scheduler loop. It fires due timers into the inbox, with one transaction for each row.
- The four timer tools `schedule_follow_up`, `create_schedule`, `list_timers` and `cancel_timer`.
  Nothing registers them for a run before SMA-790.
- The `[timers]` config section, with the creation budget.
- The event type `agents.timer.fired`.
