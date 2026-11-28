# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.0](https://github.com/SMK1085/paigasus-core/compare/paigasus-proto-derive-v0.8.0...paigasus-proto-derive-v0.9.0) - 2026-11-28

### Added

- *(rs)* add the agent-definition registry to the agent host (SMA-852) ([#537](https://github.com/SMK1085/paigasus-core/pull/537))
- *(rs)* evaluate tool-policy history conditions in the gateway (SMA-753) ([#536](https://github.com/SMK1085/paigasus-core/pull/536))
- *(rs)* add tool-call detector findings with severity and the built-in detectors (SMA-855) ([#534](https://github.com/SMK1085/paigasus-core/pull/534))
- *(rs)* add agent-host timers, follow-ups, schedules and the scheduler loop (SMA-788) ([#525](https://github.com/SMK1085/paigasus-core/pull/525))
- *(rs)* add the event inbox of the agent host (SMA-787) ([#518](https://github.com/SMK1085/paigasus-core/pull/518))

## [0.8.0](https://github.com/SMK1085/paigasus-core/compare/paigasus-proto-derive-v0.7.0...paigasus-proto-derive-v0.8.0) - 2026-11-20

### Added

- *(rs)* add the agent-host M0 skeleton (SMA-786) ([#512](https://github.com/SMK1085/paigasus-core/pull/512))
- *(rs)* add the gateway tool decision log (SMA-755) ([#509](https://github.com/SMK1085/paigasus-core/pull/509))
- *(rs)* re-compile tool policies and hold changed versions (SMA-781)
- *(rs)* IAM tool policy settings and the gateway IAM policy source (SMA-775) ([#485](https://github.com/SMK1085/paigasus-core/pull/485))
- *(rs)* add the IAM tool catalog, tool policy sets and the policy compiler (SMA-774) ([#478](https://github.com/SMK1085/paigasus-core/pull/478))
- *(rs)* add Cedar InvokeTool decisions to the gateway (SMA-752) ([#475](https://github.com/SMK1085/paigasus-core/pull/475))
- *(rs)* add the /v1/responses ingress with tool-call interception (SMA-769) ([#472](https://github.com/SMK1085/paigasus-core/pull/472))

### Fixed

- *(rs)* keep the Responses schema of blocked shell and computer results (SMA-776) ([#484](https://github.com/SMK1085/paigasus-core/pull/484))

## [0.7.0](https://github.com/SMK1085/paigasus-core/compare/paigasus-proto-derive-v0.6.0...paigasus-proto-derive-v0.7.0) - 2026-11-13

### Added

- *(rs)* intercept tool calls in gateway chat completions (SMA-749) ([#453](https://github.com/SMK1085/paigasus-core/pull/453))

## [0.6.0](https://github.com/SMK1085/paigasus-core/compare/paigasus-proto-derive-v0.5.0...paigasus-proto-derive-v0.6.0) - 2026-10-12

### Added

- *(rs)* the gateway accepts a DPoP-bound token and IAM checks the proof (SMA-700) ([#389](https://github.com/SMK1085/paigasus-core/pull/389))

## [0.5.0](https://github.com/SMK1085/paigasus-core/compare/paigasus-proto-derive-v0.4.0...paigasus-proto-derive-v0.5.0) - 2026-10-04

### Added

- *(rs)* rate limit and token budget for gateway chat completions (SMA-677) ([#373](https://github.com/SMK1085/paigasus-core/pull/373))

## [0.1.0](https://github.com/SMK1085/paigasus-core/compare/paigasus-proto-derive-v0.1.0-alpha.1...paigasus-proto-derive-v0.1.0) - 2026-08-29

### Added

- *(rs)* make the proto family publishable at 0.1.0 (SMA-577)
- *(rs)* derive macro to auto-impl Auditable for DTOs embedding AuditMetadata (SMA-438)

### Fixed

- *(repo)* make the downstream cascade real in moon ci (SMA-528) ([#145](https://github.com/SMK1085/paigasus-core/pull/145))
