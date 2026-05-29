# contracts/

Protobuf source of truth and code generation for paigasus-core. Holds the
`.proto` definitions under `proto/paigasus/<context>/<version>/` and the
[buf](https://buf.build) configuration that generates Rust, Python, and
TypeScript bindings.

**Status:** buf workspace scaffolded (SMA-360) — `buf.yaml`, `buf.gen.yaml`, and
the rs/py/ts `generated/` targets are wired. No `.proto` schemas yet.
