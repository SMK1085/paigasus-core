# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/SMK1085/paigasus-core/compare/paigasus-proto-v0.1.0-alpha.1...paigasus-proto-v0.1.0) - 2026-08-29

### Added

- *(rs)* bind and ready-gate iam's listeners before migrating (SMA-571) ([#167](https://github.com/SMK1085/paigasus-core/pull/167))
- *(rs)* answer a refused request body in the error envelope on fourteen routes (SMA-587) ([#166](https://github.com/SMK1085/paigasus-core/pull/166))
- *(contracts)* [**breaking**] structured Actor message for AuditMetadata (SMA-439) ([#165](https://github.com/SMK1085/paigasus-core/pull/165))
- *(rs)* make the proto family publishable at 0.1.0 (SMA-577)
- *(rs)* split the invalid-prn catch-all into a per-kind error taxonomy (SMA-586) ([#164](https://github.com/SMK1085/paigasus-core/pull/164))
- *(rs)* authorize CreateUser on both transports (SMA-584) ([#158](https://github.com/SMK1085/paigasus-core/pull/158))
- *(contracts)* proto services for the HTTP-only IAM ops endpoints (SMA-501) ([#156](https://github.com/SMK1085/paigasus-core/pull/156))
- *(rs)* adopt google.rpc.ErrorInfo, correlation ids and retryable metadata (SMA-504) ([#139](https://github.com/SMK1085/paigasus-core/pull/139))
- *(rs)* derive macro to auto-impl Auditable for DTOs embedding AuditMetadata (SMA-438)
- *(rs)* serve the ServiceInfo capability descriptor in iam and gateway (SMA-505) ([#124](https://github.com/SMK1085/paigasus-core/pull/124))
- *(contracts)* add the ServiceInfo capability descriptor in common/v1 (SMA-499) ([#119](https://github.com/SMK1085/paigasus-core/pull/119))
- *(contracts)* canonical error code registry in common/v1/error.proto (SMA-498) ([#120](https://github.com/SMK1085/paigasus-core/pull/120))
- *(rs)* add AI Gateway M0 walking skeleton + IAM auth (SMA-446) ([#88](https://github.com/SMK1085/paigasus-core/pull/88))
- *(rs)* add iam persistent denial audit log + query api (SMA-446) ([#80](https://github.com/SMK1085/paigasus-core/pull/80))
- *(rs)* add m4 api keys & service accounts (SMA-445) ([#79](https://github.com/SMK1085/paigasus-core/pull/79))
- *(rs)* add m3 authorization cedar policy engine (SMA-444) ([#78](https://github.com/SMK1085/paigasus-core/pull/78))
- *(rs)* add m2 authentication byo-idp oidc (SMA-443) ([#76](https://github.com/SMK1085/paigasus-core/pull/76))
- *(rs)* add m1 tenancy organizations, teams, projects and memberships (SMA-442) ([#71](https://github.com/SMK1085/paigasus-core/pull/71))
- *(rs)* paigasus-iam M0 walking skeleton (SMA-441) ([#68](https://github.com/SMK1085/paigasus-core/pull/68))
- *(contracts)* add common.v1 AuditMetadata + per-language Auditable interface (SMA-425) ([#54](https://github.com/SMK1085/paigasus-core/pull/54))
- *(contracts)* first real proto + wire paigasus-proto build-graph to contracts:generate (SMA-389) ([#37](https://github.com/SMK1085/paigasus-core/pull/37))

### Fixed

- *(rs)* reject a malformed grpc audit timestamp bound (SMA-583) ([#159](https://github.com/SMK1085/paigasus-core/pull/159))
- *(repo)* make the downstream cascade real in moon ci (SMA-528) ([#145](https://github.com/SMK1085/paigasus-core/pull/145))
- *(rs)* propagate clippy across Moon edges so a downstream break can't ship green (SMA-526)
- *(rs)* wire the paigasus-service-info Moon edges + a Cargo/Moon parity gate (SMA-524) ([#127](https://github.com/SMK1085/paigasus-core/pull/127))

### Other

- *(repo)* upgrade moon to 2.5.3 and proto to 0.61.1 (SMA-595) ([#175](https://github.com/SMK1085/paigasus-core/pull/175))
- *(contracts)* bootstrap contracts/ proto workspace with buf scaffold (SMA-360)
