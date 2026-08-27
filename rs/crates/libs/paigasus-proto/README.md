# paigasus-proto

Generated protobuf message types and tonic gRPC service stubs for Paigasus, compiled from
the canonical contracts in
[`contracts/proto`](https://github.com/SMK1085/paigasus-core/tree/main/contracts/proto)
(ADR-0004).

The generated sources are committed rather than built at consume time, so this crate has no
`build.rs` and no `protoc` dependency. `AuditMetadata`-bearing messages carry
`#[derive(Auditable)]`, injected during codegen from the companion
[`paigasus-proto-derive`](https://crates.io/crates/paigasus-proto-derive) crate.

Licensed under the Apache License, Version 2.0.
