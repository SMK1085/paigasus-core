# rs/

Rust workspace for paigasus-core — a single [Cargo](https://doc.rust-lang.org/cargo/)
workspace with three crate groups:

- `crates/libs/` — reusable libraries (e.g. `paigasus-kernel`, `paigasus-proto`)
- `crates/bindings/` — FFI wrappers (PyO3, napi-rs, wasm-bindgen)
- `crates/services/` — service binaries

**Status:** Cargo workspace landed in SMA-357. The three crates are empty placeholders
until their behavior lands in later issues.
