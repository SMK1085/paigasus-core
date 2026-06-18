# paigasus-kernel-parity

Cross-binding behavioral-parity corpus for the Paigasus kernel (ADR-0005, SMA-433).

`vectors/sum.json` is a committed, kernel-derived corpus of `{a, b, expected}` cases over the
i32-safe parity domain. Every binding (Python/PyO3, Node/napi, browser/wasm) and the Rust impl
replay it and must reproduce `expected` — the kernel is the single oracle.

- **Regenerate:** `cargo run -p paigasus-kernel-parity --bin gen-parity-vectors` (run from `rs/`).
  The sample is a deterministic enumeration (no PRNG), so output is byte-stable.
- **Drift guard:** the `repo:parity-corpus-drift` Moon task regenerates the corpus and
  `git diff --exit-code`s it, so a kernel edit landed without regenerating fails CI red. The
  in-crate `tests/replay.rs` asserts the same thing in `cargo nextest`.

Scope note: parity here is *decoded-value* equality on the i32-safe domain, not *surface*
identity — the Python binding returns a stringified i64 (`sum_as_string`), napi/wasm a `number`.
Surface unification + the full i64 range are deferred (spec § Out of scope, L5).
