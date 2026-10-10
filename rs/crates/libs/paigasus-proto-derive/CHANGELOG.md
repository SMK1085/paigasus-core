# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/SMK1085/paigasus-core/compare/paigasus-proto-derive-v0.5.0...paigasus-proto-derive-v0.6.0) - 2026-10-10

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
