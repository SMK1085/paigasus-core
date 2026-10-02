# paigasus-kernel-parity

Cross-binding behavioral-parity corpus for the Paigasus kernel (ADR-0005, SMA-433).

The committed, kernel-derived corpora live in `vectors/`. Every binding (Python/PyO3, Node/napi,
browser/wasm) and the Rust impl replay each of them, and the kernel is the single oracle:

- `sum.json`: `{a, b, expected}` over the i32-safe parity domain.
- `uuid7.json`: UUIDv7 minting from injected time and random bytes.
- `prn_canonical.json`: PRN parse and canonical form, with one row per error kind.
- `prn_cedar.json`: PRN to Cedar entity type and id.
- `prn_fields.json`: the five PRN field accessors and the `prn_build` round trip.
- `prn_parse.json`: the one-call parse wire form `[error_kind, service, region, org,
  resource_type, resource_id]` (SMA-673). Python has no one-call binding, so it replays this file
  through its six single-field accessors.

- **Regenerate:** `cargo run -p paigasus-kernel-parity --bin gen-parity-vectors` (run from `rs/`).
  The sample is a deterministic enumeration (no PRNG), so output is byte-stable.
- **Drift guard:** the `repo:parity-corpus-drift` Moon task regenerates the corpus and
  runs `git diff --exit-code`, then fails on any untracked file under `vectors/`, so a kernel
  edit landed without regenerating fails CI red. The in-crate `tests/replay.rs` asserts the same
  thing in `cargo nextest`.

Scope note: parity here is *decoded-value* equality on the i32-safe domain, not *surface*
identity — the Python binding returns a stringified i64 (`sum_as_string`), napi/wasm a `number`.
Surface unification + the full i64 range are deferred (spec § Out of scope, L5).
