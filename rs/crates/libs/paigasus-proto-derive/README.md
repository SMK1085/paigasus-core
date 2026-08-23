# paigasus-proto-derive

`#[derive(Auditable)]` for Paigasus audit metadata.

Generates the `Auditable` accessor implementation for protobuf messages that embed
`paigasus.common.v1.AuditMetadata`. The macro is injected onto the generated types during
codegen and re-exported from
[`paigasus-proto`](https://crates.io/crates/paigasus-proto)'s `audit` module, so consumers
normally depend on that crate rather than this one directly.

Licensed under the Apache License, Version 2.0.
