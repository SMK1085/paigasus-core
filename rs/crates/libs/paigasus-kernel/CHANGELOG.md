# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/SMK1085/paigasus-core/compare/paigasus-kernel-v0.1.0-alpha.1...paigasus-kernel-v0.1.0) - 2026-08-29

### Added

- *(rs)* kernel-family 0.1.0 floor + repo:version-lockstep gate + release-PR job ([#157](https://github.com/SMK1085/paigasus-core/pull/157))
- *(rs)* make paigasus-kernel publishable on crates.io (SMA-376) ([#128](https://github.com/SMK1085/paigasus-core/pull/128))
- *(rs)* PRN primitive + UUIDv7 minting bound to Py/Node/WASM (SMA-448) ([#66](https://github.com/SMK1085/paigasus-core/pull/66))
- *(rs)* cross-binding behavioral parity harness (SMA-433) ([#52](https://github.com/SMK1085/paigasus-core/pull/52))
- *(rs)* wire kernel→bindings affected-graph cascade + add CI regression guard (SMA-409) ([#43](https://github.com/SMK1085/paigasus-core/pull/43))

### Fixed

- *(repo)* make the downstream cascade real in moon ci (SMA-528) ([#145](https://github.com/SMK1085/paigasus-core/pull/145))
- *(rs)* avoid Windows-reserved 'prn' filename in kernel + ts tests (SMA-448) ([#67](https://github.com/SMK1085/paigasus-core/pull/67))

### Other

- *(repo)* set explicit Moon layer across projects and scaffold templates (SMA-381)
- *(rs)* suffix Rust Moon project ids with -rs (SMA-380)
- *(rs)* bootstrap Cargo workspace with libs/bindings/services layout (SMA-357)
