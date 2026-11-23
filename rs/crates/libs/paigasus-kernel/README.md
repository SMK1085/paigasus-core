# paigasus-kernel

Pure-logic behavioral kernel for Paigasus — the cross-language primitives that must
behave identically everywhere: Paigasus Resource Names (`Prn`), UUIDv7 minting from
injected bytes, and Cedar entity UIDs.

No I/O, no FFI, no adapters. The Python, Node and browser bindings live in
[`rs/crates/bindings/`](https://github.com/SMK1085/paigasus-core/tree/main/rs/crates/bindings)
and call into this crate rather than reimplementing it (ADR-0005).

## The `tool-policy` feature

The `tool-policy` feature is off by default (SMA-752). It adds the Cedar decision for a tool call
(`tool_policy`). It pulls `cedar-policy`, `serde`, `serde_json` and `unicode-normalization`.
Only the gateway turns it on. A binding crate (wasm, Python, Node) must not turn it on.
The `repo:wasm-getrandom-free` gate fails if a binding crate pulls `cedar-policy` (D21).

## The `detectors` feature

The `detectors` feature is off by default (SMA-855). It holds the pure detector code for tool
calls: host extraction and the built-in token forms. It pulls `serde`, `serde_json`,
`unicode-normalization`, `regex` and `url`. The `tool-policy` feature turns it on.
A binding crate (wasm, Python, Node) must not turn it on.
The `repo:wasm-getrandom-free` gate fails if the feature reaches a binding crate.

## macOS wheels of version 0.2.0

The `paigasus-py-bindings` 0.2.0 macOS wheels (arm64 and x86_64) have a Mach-O string table
with `stroff mod 8 = 4`. The build used SDK 26.5. The arm64 wheel loads on macOS 27.
Nobody has loaded the x86_64 wheel on macOS 27.
On macOS 27.2, dyld rejects the same layout in a binary that SDK 27.2 linked
("mis-aligned LINKEDIT"). The npm darwin 0.2.0 packages (arm64 and x64) are not affected
(`stroff mod 8 = 0`).

The fix (SMA-767) is in the repository. Each release after 0.2.0 includes the fix.

Licensed under the Apache License, Version 2.0.
