# paigasus-kernel

Pure-logic behavioral kernel for Paigasus — the cross-language primitives that must
behave identically everywhere: Paigasus Resource Names (`Prn`), UUIDv7 minting from
injected bytes, and Cedar entity UIDs.

No I/O, no FFI, no adapters. The Python, Node and browser bindings live in
[`rs/crates/bindings/`](https://github.com/SMK1085/paigasus-core/tree/main/rs/crates/bindings)
and call into this crate rather than reimplementing it (ADR-0005).

Licensed under the Apache License, Version 2.0.
