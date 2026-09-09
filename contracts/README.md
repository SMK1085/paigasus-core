# contracts/

Protobuf source of truth and code generation for paigasus-core. Holds the
`.proto` definitions under `proto/paigasus/<context>/<version>/` and the
[buf](https://buf.build) configuration that generates Rust, Python, and
TypeScript bindings.

**Status:** buf workspace scaffolded (SMA-360) — `buf.yaml`, `buf.gen.yaml`, and
the rs/py/ts `generated/` targets are wired. `contracts:generate` runs TWO
`buf generate` invocations, in order: the main `buf.gen.yaml` template for this
workspace's own `.proto` schemas, then `buf.gen.googleapis.yaml` (TS-only) for a
pinned `google.rpc.ErrorInfo` from the `googleapis` BSR module (SMA-624).
