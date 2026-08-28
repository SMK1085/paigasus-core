# paigasus-py-bindings

PyO3 bindings for the Paigasus behavioral kernel — PRN canonicalization and UUIDv7
minting, implemented once in Rust (`paigasus-kernel`) and bound to Python, Node and
WebAssembly (ADR-0005).

Most users want [`paigasus-kernel`](https://pypi.org/project/paigasus-kernel/), the
Python-facing wrapper, rather than this extension module directly.

## Installation

Wheels are published for CPython 3.12+ on macOS (arm64, x86_64), Windows (x86_64) and
Linux (x86_64 and aarch64, glibc and musl). They are `abi3` wheels, so one wheel per
platform covers every CPython from 3.12 onward.

## Building from source

The source distribution builds on any platform with a Rust toolchain:

    pip install paigasus-py-bindings --no-binary paigasus-py-bindings

**Minimum supported Rust version: 1.95** (the crate is edition 2024). The sdist
deliberately ships no `rust-toolchain.toml`, so your installed toolchain is what builds
it; an older rustc fails during `pip install` with a cargo error.

## License

Apache-2.0. See [LICENSE](./LICENSE).
