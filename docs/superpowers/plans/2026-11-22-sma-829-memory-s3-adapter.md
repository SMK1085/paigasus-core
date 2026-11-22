# SMA-829 S3-compatible memory store Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `S3MemoryStore`, a second `MemoryStore` adapter over an S3-compatible object store, that passes the SMA-789 conformance suite unchanged, with a boot probe, `memory.backend = "s3"` config and `MemoryCommit::External`.

**Architecture:** A new adapter module `adapters/memory_s3/` holds an `Arc<dyn object_store::ObjectStore>`. Each object is a 51-byte header with a random write ID, then the content, so the ETag is the version and never repeats. `apply` lists the scope once, runs a pure pre-check in the order of the fake, then writes every `Put` before any `Delete`, with a check after each failed write, a bounded concurrency of 8 and one deadline. Production uses `AmazonS3`. Tests use `object_store::memory::InMemory` and a scripted test store, and three pinned container images.

**Tech Stack:** Rust 2024 (workspace rust-version 1.95), tokio, `object_store` 0.14.2 (`aws-base`, `reqwest`, `ring`), reqwest 0.13, rustls 0.23 (`ring`), figment, metrics, testcontainers 0.27 through `paigasus-test-docker`, cargo nextest.

**Spec:** `docs/superpowers/specs/2026-11-22-sma-829-memory-s3-adapter-design.md` (approved at Gate 1). Inputs: §6.8 and §6.12 of `docs/superpowers/specs/2026-11-20-sma-789-memory-store-design.md`.

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Edition 2024 and the workspace `rust-version`. The service crate keeps the hexagonal layout: the new code is an adapter under `src/adapters/`.
- `conformance.rs` and the core crate `paigasus-agent-host-core` do not change (AG1). The `MemoryStore` trait and `MemoryApplyError` do not change.
- New workspace dependency, exactly: `object_store = { version = "0.14.2", default-features = false, features = ["aws-base", "reqwest", "ring"] }`. Never the `aws` feature (it pulls `aws-lc-rs`).
- `reqwest = { workspace = true }` and `rustls = { workspace = true }` move to the agent host `[dependencies]`. `rs/deny.toml` does not change.
- Never call `AmazonS3Builder::from_env()`. Never call `std::env::set_var` (it is `unsafe` in edition 2024).
- No `Store(String)` text, log line or metric label holds a memory path, an object key, memory content or a credential (AG5, AG8). Use only the fixed class texts of Task 2.
- `-D warnings` and workspace `warnings = deny`: each task compiles clean. Do not add code that a later task uses ("add now, wire later"). A `pub` item in the `pub mod memory_s3` tree is not dead code; a private helper must have a caller in the same task. Test helpers in `#[cfg(test)]` follow the same rule.
- No `tokio::spawn` in the adapter. The writes run in the calling task (a `FuturesUnordered`), so a dropped future stops its request and a local metrics recorder sees every count.
- Commands run from `rs/` with the proto shims first: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. The crate checks: `cargo build -p paigasus-agent-host`, `cargo clippy -p paigasus-agent-host --all-targets -- -D warnings`, `cargo fmt --check`, `cargo nextest run -p paigasus-agent-host <filter>`.
- A Docker test runs with `PAIGASUS_REQUIRE_DOCKER=1`, so a skip is a failure. A container test uses `paigasus_test_docker::start_or_skip` and `mapped_port`. No `AsyncRunner` import and no `CI` read in `src/` or `tests/` (`repo:iam-docker-policy-single-site`).
- Images are pinned by tag, with the digest in a comment next to the tag: `rustfs/rustfs:1.0.6` (`sha256:ee717072290bec9b11abfc0365f8310b00df1cd9b3639943edae47bdef05044a`), `versity/versitygw:v1.11.0` (`sha256:8319a530d08d537426e2f855bae5280709795dc73a3f0c347973ef915d6343c5`), `versity/versitygw:v1.0.17` (`sha256:a13b9adbda0ea9d4b1d929e4bed327e4eccc18d13c23f7e5d192d987dde5651d`).
- Commits: conventional, scope `rs`, for example `feat(rs): add the S3 memory key layout (SMA-829)`. The body ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No line in the body starts with `#NNN`. Never `--no-verify`, never `--amend`, never `git reset`, never a bare `git stash`.
- Doc comments, README text and error texts use ASD-STE100 Simplified Technical English.
- Each commit step stages only the files in the `Files` list of its task (`git add <paths>`), never `git add -A`.

## Decisions this plan makes (spec §9 and corrections)

- **The order inside one `Put` is the order of `fake.rs`: the NUL check, then the version check, then `check_file`.** The spec §6.5 phase 1 text says "version, then NUL", but it also says "copy the exact order from `fake.rs`". `fake.rs:115-130` checks NUL first, and Postgres fails on NUL at the statement, before its row count. The fake wins, so every adapter reports the same refusal. The spec test `[Put /a conflict, Put /b NUL]` gives `Conflict { /a }` with either order. Task 3 adds a test for one `Put` with a stale version and a NUL: the result is `Store`.
- **`get_config_value` is a method of `AmazonS3Builder`, not of the built client.** So the client is two functions: `s3_builder` (pure, tested with `get_config_value`) and `build_store` (calls `build()`).
- **No status code is reachable by type.** `RetryError` is in a `pub(crate)` module of `object_store` 0.14.2. A 412, 409, 403, 401 and 404 already map to typed variants (`client/retry.rs:157-187`). Every other status becomes `Error::Generic`. Task 2 finds `status=<NNN>` by a search in the `Display` text of the source chain for the fixed marker `non-2xx status code: ` and keeps only the three digits. The text never leaves the function. Without the marker, the class is `other`.
- **Production passes the environment from `std::env::vars_os()`, filtered to UTF-8 pairs.** `std::env::vars()` panics on a non-UTF-8 variable.
- **The test object store is one scripted in-memory `ObjectStore` (`testing.rs`)**, not wrappers over `InMemory`. It has an ETag mode (`Counter`, `ContentHash`, `SizeOnly`), hooks and faults. Each task adds only the parts that its own tests use. Plain `InMemory` also runs the conformance suite.
- **The endpoint URL is parsed with `reqwest::Url`.** So the code names `reqwest`, and no `cargo-machete` ignore entry is needed. If `reqwest` stays unnamed, add it to `[package.metadata.cargo-machete] ignored` with a comment (spec §8).
- **`MemoryCommit::build` takes no `LeaseConfig`.** `Config::validate` already compares `apply_deadline_secs` with `lease.ttl_secs`.
- **A failed `DELETE` whose check `HEAD` shows another ETag is an unknown outcome**, not a failure. The spec names only "absent", "present with `v`" and "failed `HEAD`". Unknown gives `Partial`, which is the safe side.
- **A foreign object found by the write-ID `GET` is an unknown outcome** (spec §6.5). The spec §6.3 sentence "the result is `Store`" names the bare refusal path; §6.5 is the exact rule, and the plan follows it.

## Review Focus

1. **The longest key.** A 128-byte prefix, three UUIDs and a 256-byte path with 8 segments must stay under the S3 key limit of 1,024 bytes. Expected: the key is accepted. Pinned in Task 1 (`the_longest_key_is_under_the_s3_limit`).
2. **An empty file.** A file with content `""` is an object of exactly 51 bytes. Expected: `list` shows it with `size_bytes == 0`; it is not skipped as `size`. Pinned in Task 4 (`an_empty_file_is_listed_with_size_zero`).
3. **More than one listing page.** A scope with more than 1,000 keys. Expected: `delete_scope` removes every key and the second listing is empty. Pinned in Task 11 (`rustfs_delete_scope_removes_more_than_one_listing_page`).
4. **An empty allowlisted credential variable** (`AWS_ACCESS_KEY_ID=""`, for example an unexpanded template). Expected: it is treated as not set, and it does not reach the builder. Pinned in Task 8 (`an_empty_allowlisted_variable_is_not_copied`).
5. **An endpoint with a trailing `/`** (`http://minio.local:9000/`). Expected: it is accepted, the same as without the `/`. Pinned in Task 7 (`an_endpoint_with_a_trailing_slash_is_valid`).

A sixth item is a spec gap, not a test: reqwest reads `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY` by default, and `object_store` sets no `no_proxy` when `proxy_url` is not set (`client/mod.rs:879-896`). So these variables can change the client, which G5 forbids in its wording. Task 8 measures it and records the result. The README names the proxy variables as read. Sven decided on 2026-11-22: keep the proxy variables and document them. The client does not turn off the proxy lookup.

---

### Task 1: The dependency and the key layout

**Files:**
- Modify: `rs/Cargo.toml` (`[workspace.dependencies]`, near `reqwest` line 144)
- Modify: `rs/crates/services/paigasus-agent-host/Cargo.toml` (`[dependencies]`; remove `reqwest` and `rustls` from `[dev-dependencies]` lines ~82-85 and keep their comments next to the new lines)
- Modify: `rs/Cargo.lock`
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/mod.rs` (add `pub mod memory_s3;`)
- Create: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/mod.rs` (module doc and `pub mod keys;` only)
- Create: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/keys.rs`

**Interfaces:**
- Produces: `pub struct KeyPrefix(String)` with `KeyPrefix::parse(raw: &str) -> Result<KeyPrefix, PrefixError>` and `as_str(&self) -> &str`. It derives `Debug, Clone, PartialEq, Eq`.
- Produces: `pub enum PrefixError` (`thiserror`, `Debug, Clone, PartialEq, Eq`): `Empty`, `TooLong { bytes: usize }`, `TooManySegments { segments: usize }`, `EmptySegment`, `LeadingOrTrailingSlash`, `BadSegment`. The texts may name the prefix value: it is operator config.
- Produces: `pub fn scope_path(prefix: &KeyPrefix, scope: &MemoryScope) -> object_store::path::Path` (`<prefix>/<org>/<agent>/<principal>`, hyphenated lower-case UUIDs).
- Produces: `pub fn file_path(prefix: &KeyPrefix, scope: &MemoryScope, path: &MemoryPath) -> Path` and `pub fn dir_path(prefix: &KeyPrefix, scope: &MemoryScope, dir: &MemoryDir) -> Path` (the root dir gives the scope path).
- Produces: `pub fn probe_path(prefix: &KeyPrefix, id: Uuid) -> Path` (`<prefix>/_probe/<uuid>`) and `pub fn probe_dir(prefix: &KeyPrefix) -> Path` (`<prefix>/_probe`).
- Produces: `pub struct OutsideBase;` (`Debug, Clone, Copy, PartialEq, Eq`), `pub enum ListedKey { File(MemoryPath), Refused }` and `pub fn listed_key(base: &Path, key: &Path) -> Result<ListedKey, OutsideBase>`. `base` is the scope or directory path. `Err(OutsideBase)` when `key` does not start with `<base>/` (a store fault, AG6). `Refused` when the rest, with a leading `/`, does not parse as a `MemoryPath`.
- Build the `Path` values with `Path::from_iter` over parts or `Path::parse` of a string that is valid by construction. Do not use `Path::from(&str)`: it percent-encodes.

- [ ] **Step 1: Add the dependency and measure the hazards**

Add the workspace line of the Global Constraints. In the agent host: `object_store = { workspace = true }`, and move `reqwest = { workspace = true }` and `rustls = { workspace = true }` to `[dependencies]` with a comment: "`adapters::memory_s3` — `object_store` uses the rustls backend that `reqwest` turns on. The `ring` provider is installed before each client build." Run `cargo build -p paigasus-agent-host` once (unlocked, so it adds the new packages to `rs/Cargo.lock`), and check that the lock holds `object_store` `0.14.2` and that `git diff rs/Cargo.lock` adds packages and changes no existing version. Then run each command and record the output in the commit body:

Run: `cargo tree --locked -p paigasus-agent-host -e normal -i aws-lc-rs`
Expected: an error that says that the package ID matches no package (absent, G6, AG7).

Run: `cargo tree -p paigasus-agent-host -e normal -d | grep -E '^(reqwest|rustls) '`
Expected: no output (no second major).

Run: `cargo tree -p paigasus-agent-host -e normal -i reqwest | head -3` and the same for `rustls`
Expected: one version each, `reqwest v0.13.x` and `rustls v0.23.x`.

- [ ] **Step 2: Write the failing tests in `keys.rs`**

- `a_valid_prefix_parses`: `mem`, `a/b.c/d-e_f`, 8 segments, a 128-byte prefix: all `Ok`.
- `the_prefix_table_refuses`: each row gives its error: `""` → `Empty`; `..` and `.x` and `a/.b` → `BadSegment`; `/a` and `a/` → `LeadingOrTrailingSlash`; `a//b` → `EmptySegment`; `%2e` and `a b` → `BadSegment`; 129 bytes → `TooLong { bytes: 129 }`; 9 segments → `TooManySegments { segments: 9 }`; a 65-character segment → `BadSegment`.
- `a_file_key_round_trips`: `file_path(mem, scope, /tasks/t1.md)` equals `mem/<org>/<agent>/<principal>/tasks/t1.md`, and `listed_key(&scope_path(..), &key)` is `File(/tasks/t1.md)`.
- `the_root_dir_is_the_scope_path`: `dir_path(.., MemoryDir::root())` equals `scope_path(..)`.
- `a_key_outside_the_base_is_a_fault`: `listed_key(base "mem/s/a", key "mem/s/ab/x")` is `Err(OutsideBase)` (AG6); `listed_key(base, base)` is `Err(OutsideBase)`.
- `a_key_that_memory_path_refuses_is_refused`: a 65-character segment and a `~` segment give `Refused`.
- `the_probe_key_is_not_a_scope_key`: `probe_path(mem, id)` starts with `mem/_probe/`; `"_probe"` does not parse as a UUID, so no scope path starts with `mem/_probe/`.
- `the_longest_key_is_under_the_s3_limit` (Review Focus 1): a 128-byte prefix, three UUIDs and a 256-byte `MemoryPath` with 8 segments give `file_path(..).as_ref().len() < 1024`.

- [ ] **Step 3: Run the tests and see them fail**

Run: `cargo nextest run -p paigasus-agent-host -E 'test(memory_s3::keys)'`
Expected: compile error (no `KeyPrefix`).

- [ ] **Step 4: Implement `keys.rs`**

The prefix rule (spec §6.2): 1 to 8 segments split by `/`; each segment matches `[A-Za-z0-9._-]{1,64}` and does not start with `.`; no leading or trailing `/`; no empty segment; at most 128 bytes. Check the length first, then the slashes, then the segments.

- [ ] **Step 5: Run the tests and the dependency gates**

Run: `cargo nextest run -p paigasus-agent-host -E 'test(memory_s3::keys)'` — Expected: all PASS.
Run (from the repository root): `moon run repo:deny repo:machete` — Expected: both pass. If `machete` reports `reqwest` or `rustls` as unused, add them to `[package.metadata.cargo-machete] ignored` with the comment of Step 1. Task 7 removes the `reqwest` entry when it names `reqwest::Url`; the `rustls` entry goes when Task 8 calls `rustls::crypto::ring::default_provider()`.

- [ ] **Step 6: Commit**

`git add rs/Cargo.toml rs/Cargo.lock rs/crates/services/paigasus-agent-host/Cargo.toml rs/crates/services/paigasus-agent-host/src/adapters/mod.rs rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/`
Message: `feat(rs): add object_store and the S3 memory key layout (SMA-829)`, with the three `cargo tree` results in the body.

---

### Task 2: The envelope and the error classes

**Files:**
- Create: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/envelope.rs`
- Create: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/errors.rs`
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/mod.rs` (`pub mod envelope; pub mod errors;`)

**Interfaces:**
- Produces (`envelope.rs`): `pub const HEADER_LEN: u64 = 51;` and `pub const MAGIC: &str = "paigasus-memory/1 ";`.
- Produces: `pub struct WriteId(String)` (`Debug, Clone, PartialEq, Eq`) with `WriteId::random() -> WriteId` (UUID v4, 32-character simple form, lower-case hex) and `as_str(&self) -> &str`.
- Produces: `pub fn encode(id: &WriteId, content: &str) -> Vec<u8>` (`MAGIC`, the ID, `\n`, the content bytes).
- Produces: `pub fn decode(bytes: &[u8]) -> Result<(WriteId, String), EnvelopeError>` and `pub enum EnvelopeError { Short, Magic, Id, Newline, Utf8 }`.
- Produces: `pub fn content_size(object_size: u64) -> Option<u64>` (`None` when `object_size < HEADER_LEN`).
- Produces (`errors.rs`): `pub fn class(e: &object_store::Error) -> String`, one of `not_found`, `precondition`, `already_exists`, `permission_denied`, `unauthenticated`, `not_implemented`, `status=<NNN>`, `timeout`, `transport`, `unparsable_key`, `other`.
- Produces: `pub fn is_refusal(e: &object_store::Error) -> bool` (true for `AlreadyExists` and `Precondition`, spec §6.4).
- Produces: `pub fn store_text(sentence: &'static str, e: &object_store::Error) -> String` (`"{sentence} ({class})"`). Every `Store(String)` of the adapter comes from this function or from a fixed `&'static str`.

- [ ] **Step 1: Write the failing tests**

`envelope.rs`:
- `the_header_is_51_bytes`: `assert_eq!(HEADER_LEN, 51)`, `assert_eq!(MAGIC.len(), 18)`, `assert_eq!(WriteId::random().as_str().len(), 32)`, and `encode(&id, "").len() == 51`.
- `encode_then_decode`: for `""`, `"a"`, `"über\n"`, the decode gives the same ID and content; `content_size(encode(..).len())` is the content byte length.
- `two_writes_of_one_content_differ`: `encode(&WriteId::random(), "same") != encode(&WriteId::random(), "same")`.
- `decode_refuses`: 50 bytes → `Short`; `paigasus-memory/2 …` → `Magic`; an ID with `g` or upper-case hex → `Id`; a header whose byte 50 is not `\n` → `Newline`; content `[0xff]` → `Utf8`.
- `a_short_object_has_no_content_size`: `content_size(50) == None`, `content_size(51) == Some(0)`.

`errors.rs`:
- `each_kind_maps_to_its_class`: build each variant (`NotFound`, `Precondition`, `AlreadyExists`, `PermissionDenied`, `Unauthenticated`, `NotImplemented`, `InvalidPath`, `Generic` with a plain source) and assert the class text. `InvalidPath` gives `unparsable_key`.
- `no_class_text_holds_the_path_or_the_uri` (AG8): each variant above is built with the path `mem/secret-name/x` and a source whose `Display` is `https://store.example/secret-name?X-Amz-Credential=secret-name`. `store_text("s", &e)` does not contain `secret-name`.
- `a_generic_status_is_kept_as_digits`: a `Generic` whose source `Display` is `Server returned non-2xx status code: 501 Not Implemented: secret-name` gives `status=501`, and the text holds no `secret-name`.
- `refusals`: `is_refusal` is true for `AlreadyExists` and `Precondition`, false for `NotFound` and `Generic`.

- [ ] **Step 2: Run the tests and see them fail**

Run: `cargo nextest run -p paigasus-agent-host -E 'test(memory_s3::envelope) | test(memory_s3::errors)'`
Expected: compile error.

- [ ] **Step 3: Implement both files**

`class` walks the `std::error::Error::source()` chain of a `Generic` error. It downcasts each link to `object_store::client::HttpError` and maps `HttpErrorKind::Timeout` to `timeout`, and `Connect`, `Request`, `Interrupted`, `Decode` to `transport`. Then it looks for the status marker of the Decisions section. `decode` does not trust `MAGIC.len() + 32 + 1` to be 51 at run time: a `const` assertion `assert!(MAGIC.len() + 32 + 1 == HEADER_LEN as usize)` pins it at compile time.

- [ ] **Step 4: Run the tests and see them pass**

Run: the command of Step 2. Expected: all PASS.

- [ ] **Step 5: Commit**

Message: `feat(rs): add the S3 memory envelope and the error classes (SMA-829)`.

---

### Task 3: The pure pre-check and the result rule

**Files:**
- Create: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/plan.rs`
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/mod.rs` (`pub mod plan;`)

**Interfaces:**
- Consumes: `MemoryCommitSet`, `MemoryChange`, `Expected`, `MemoryCaps::check_file`, `MemoryCaps::check`, `MemoryDir::covers`, `MemoryUsage` from the core.
- Produces: `pub struct Snapshot { pub files: BTreeMap<MemoryPath, (u64, MemoryVersion)> }` (path to content size and version). `Snapshot::usage(&self) -> MemoryUsage`.
- Produces: `pub struct WritePlan { pub puts: Vec<usize>, pub deletes: Vec<usize> }` (indexes into `set.changes.changes()`, in set order).
- Produces: `pub fn precheck(set: &MemoryCommitSet, snapshot: &Snapshot, caps: &MemoryCaps) -> Result<WritePlan, MemoryApplyError>`.
- Produces: `pub enum Outcome { Confirmed, Failed(MemoryApplyError), Unknown(MemoryApplyError), NotStarted }` (`Debug, Clone, PartialEq, Eq`).
- Produces: `pub fn result_of(set: &MemoryCommitSet, outcomes: &[Outcome]) -> Result<(), MemoryApplyError>` (`outcomes[i]` belongs to change `i`).
- Produces: `pub enum Seen { Mine, Other, Absent, Foreign, ReadFailed }` and `pub fn put_outcome(refused: bool, cause: MemoryApplyError, seen: Seen, path: &MemoryPath) -> Outcome`.
- Produces: `pub enum SeenAfterDelete { Absent, Expected, Other, ReadFailed }` and `pub fn delete_outcome(cause: MemoryApplyError, seen: SeenAfterDelete) -> Outcome`.

The order of `precheck` (spec §6.5 phase 1, with the order correction of the Decisions section):
1. For each guard in `BTreeSet` order, the snapshot paths that the guard covers, in byte order. The first one that no change names is `Conflict { path }`.
2. For each change in set order, all its checks before the next change. A `Put`: NUL (`Store("the content contains a NUL character")`, the text of `fake.rs`), then the version (`Absent` conflicts when the path is in the snapshot; `Version(v)` conflicts when the path is absent or its version is not `v`), then `caps.check_file(path, content.len())`. A `Delete { v }`: conflict when the path is absent or its version is not `v`.
3. `after` from the snapshot and the set (a create adds one file and its size; an overwrite subtracts the old size and adds the new; a delete subtracts the file). Then `caps.check(before, after)`.

The rules of `result_of` (spec §6.5 "The result"): `confirmed` counts `Confirmed`. All `Confirmed` gives `Ok(())`. `confirmed == 0` and no `Unknown` gives the error of the first `Failed` in set order. Else `Partial { applied: confirmed, failed_at, cause }`, where `failed_at` and `cause` are those of the first `Failed` or `Unknown` in set order. `NotStarted` is never `failed_at`.

The rules of `put_outcome` (spec §6.5): `Mine` → `Confirmed`. `Other` or `Absent` with `refused` → `Failed(Conflict { path })`. `Other` or `Absent` without `refused` → `Failed(cause)`. `Foreign` or `ReadFailed` → `Unknown(cause)`. The rules of `delete_outcome`: `Absent` → `Confirmed`; `Expected` → `Failed(cause)`; `Other` and `ReadFailed` → `Unknown(cause)` (Decisions section).

- [ ] **Step 1: Write the failing table tests**

- `an_empty_set_needs_no_write`: a set with no change and no guard gives an empty `WritePlan`.
- `the_version_rows`: `Put Absent` over a present path → `Conflict`; `Put Version(v)` over an absent path, and over a path with version `w` → `Conflict`; `Delete v` over an absent path, and over `w` → `Conflict`; the matching rows → `Ok`.
- `a_guard_conflict_wins`: guard `/d`, snapshot `/d/a`, set `[Put /x over the file cap]` → `Conflict { /d/a }`, not `CapExceeded`.
- `guards_report_the_first_outsider_in_byte_order`: guards `{/a, /b}`, snapshot `/b/1`, `/a/2`, `/a/1` → `Conflict { /a/1 }`.
- `the_first_change_reports_first`: `[Put /a stale, Put /b with NUL]` → `Conflict { /a }`.
- `nul_comes_before_the_version_check`: `[Put /a stale with NUL]` → `Store("the content contains a NUL character")` (the fake order).
- `the_version_check_comes_before_the_file_cap`: `[Put /a stale over the file cap]` → `Conflict { /a }`.
- `the_store_caps_run_last`: a set that passes every change check but makes `files` go over `max_store_files` → `CapExceeded(StoreFiles { .. })`.
- `after_counts_overwrite_delete_and_create`: snapshot `{/a: 10, /b: 5}`, set `[Put /a 3 bytes over, Delete /b, Put /c 4 bytes]` and caps with `max_store_bytes = 7`, `max_store_files = 2` → `Ok` (after = 2 files, 7 bytes); with `max_store_bytes = 6` → `CapExceeded(StoreBytes { after: 7, limit: 6, now: 15 })`.
- `puts_come_before_deletes_in_the_plan`: `[Delete /old, Put /new]` gives `puts == [1]`, `deletes == [0]`.
- `the_result_rules`: all `Confirmed` → `Ok`; `[Failed(Conflict /a), NotStarted]` → bare `Conflict { /a }`; `[Confirmed, Failed(Store s), NotStarted]` → `Partial { applied: 1, failed_at: /b, cause: Store s }`; `[Unknown(Store t)]` → `Partial { applied: 0, failed_at: /a, .. }`; `[Confirmed, Unknown(x), Failed(y)]` → `failed_at` is the path of index 1.
- `the_put_outcome_rows` and `the_delete_outcome_rows`: one assertion per rule above.

- [ ] **Step 2: Run the tests and see them fail**

Run: `cargo nextest run -p paigasus-agent-host -E 'test(memory_s3::plan)'`
Expected: compile error.

- [ ] **Step 3: Implement `plan.rs`**

- [ ] **Step 4: Run the tests and see them pass**

Run: the command of Step 2. Expected: all PASS.

- [ ] **Step 5: Commit**

Message: `feat(rs): add the pure pre-check of the S3 memory store (SMA-829)`.

---

### Task 4: The reads, `delete_scope` and the skipped counter

**Files:**
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/mod.rs` (the store type and the read methods)
- Create: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/testing.rs` (`#[cfg(test)]`)
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (a constant near line 147, `ALL` near line 407, a registry test near line 498)
- Modify: `rs/crates/services/paigasus-agent-host/src/main.rs` (`describe_agent_host_metrics`, near line 371)

**Interfaces:**
- Consumes: Task 1 keys, Task 2 envelope and errors.
- Produces: `pub struct S3MemoryStore { store: Arc<dyn ObjectStore>, prefix: KeyPrefix, apply_deadline: Duration }` with `S3MemoryStore::new(store: Arc<dyn ObjectStore>, prefix: KeyPrefix, apply_deadline: Duration) -> Self`. It derives `Clone`. `Debug` is manual and prints only the prefix.
- Produces: `impl MemoryStore for S3MemoryStore`, with `get`, `list`, `usage` and `delete_scope` real in this task. `apply` returns `Err(MemoryApplyError::Store("the S3 memory store cannot apply yet".into()))` in this task only; Task 5 replaces it. This is not dead code: it is a trait method.
- Produces: a private `async fn listing(&self, base: &Path) -> Result<Vec<MemoryEntry>, MemoryStoreError>` that `list` and `usage` use, and that Task 5 turns into a `Snapshot`. It applies the skip rules below and counts each skipped key.
- Produces: a private `fn version_of(e_tag: Option<&str>) -> Option<MemoryVersion>` (the identity in this task; Task 11 changes it only if the measurement of §6.3 shows a difference).
- Produces: `pub const AGENT_HOST_MEMORY_S3_SKIPPED_OBJECTS_TOTAL: &str = "agent_host_memory_s3_skipped_objects_total";` with the label `reason` = `key` | `size` | `header` | `unparsable`. Its HELP text: "Objects under a memory scope that a read skipped or refused, labeled by reason. It counts events at each read, not distinct objects."
- Produces (`testing.rs`): `pub struct ScriptedStore` (an in-memory `ObjectStore`: a `Mutex<BTreeMap<Path, (Vec<u8>, String /*etag*/)>>`), with `ScriptedStore::new(mode: EtagMode) -> Arc<Self>`, `pub enum EtagMode { Counter, ContentHash }`, `insert_raw(&self, key: &str, bytes: &[u8])`, `inject_listed_key(&self, key: &str)` (the next `list` also yields this key, whatever the prefix), and `fail_list_with_invalid_path(&self)` (the next `list` yields `Error::InvalidPath`). It honours `PutMode::Create` (`AlreadyExists`) and `PutMode::Update` (`Precondition`), `GetOptions::head`, `delete_stream`, `list` and `list_with_delimiter`, and implements `Display` and `Debug` (the `ObjectStore` bounds). ETags are quoted, as S3 sends them. A method that returns `NotImplemented` is acceptable for `put_multipart_opts` and `copy_opts`.

The rules (spec §6.2 to §6.4, §6.6):
- `get`: `GET` the file key. `NotFound` → `Ok(None)`. A missing ETag → `Store("the object has no ETag")`. A header that `decode` refuses → `Store("object is not a memory file")` and count `reason = header`.
- `list(scope, dir)`: list `dir_path`. For each entry: `listed_key` against the dir path (`OutsideBase` → `Store("the store listed a key outside the scope")`); `Refused` → skip, count `key`; `content_size == None` → skip, count `size`; a missing ETag → `Store`. An `Err` from the stream → `Store(store_text("the memory listing failed", e))`, and when the class is `unparsable_key` count `unparsable`. Sort by path.
- `usage`: `files` and `bytes` over `listing(scope_path)`.
- `delete_scope`: a raw listing of the scope path (no skip, no count); delete each key with no condition, at most 8 at a time (`const DELETE_CONCURRENCY: usize = 8`); then list again. Any key left → `Store("keys remain after the scope delete")` (AG9). A listing error → `Store`.

- [ ] **Step 1: Write the failing tests (in `mod.rs` tests, over `ScriptedStore` in `Counter` mode, with objects written by `insert_raw(encode(..))`)**

- `get_reads_the_content_and_the_etag`: an encoded `/a` gives `content == "x"` and `version` equal to the stored ETag.
- `get_of_a_missing_file_is_none`.
- `a_foreign_object_is_a_store_error`: raw `plain text` at `/instructions.md` → `Store("object is not a memory file")`, and the skipped counter with `reason = header` is 1.
- `list_skips_short_and_refused_keys`: raw objects at `…/notes` (0 bytes) and `…/a~b` → not listed; counter `size` 1 and `key` 1.
- `an_empty_file_is_listed_with_size_zero` (Review Focus 2).
- `list_of_a_dir_does_not_return_a_sibling_prefix`: files `/a/x` and `/ab/y`, `list(/a)` returns only `/a/x` (AG6).
- `a_listed_key_outside_the_scope_is_a_store_error`: `inject_listed_key` with a key of another scope → `list` returns `Store("the store listed a key outside the scope")` (AG6).
- `an_unparsable_key_makes_the_scope_unreadable`: `fail_list_with_invalid_path` → `list` and `usage` return `Store` whose text holds `unparsable_key`; counter `unparsable` 1.
- `usage_sums_the_listed_files`.
- `delete_scope_removes_skipped_keys`: a folder marker and a refused key are removed; a second scope stays.
- `delete_scope_fails_when_a_key_remains`: a `ScriptedStore` whose delete does nothing (add `ignore_deletes(&self)` here, because this test is its first user) → `Store("keys remain after the scope delete")`.
- In `names.rs`: `the_agent_host_memory_s3_families_are_registered` asserts `ALL` contains the new name.
- Read each counter value with `DebuggingRecorder` and `metrics::set_default_local_recorder`, as `adapters/nats/consumer.rs:506` does. Assert the number, not the `# TYPE` line.

- [ ] **Step 2: Run the tests and see them fail**

Run: `cargo nextest run -p paigasus-agent-host -E 'test(memory_s3)'` and `cargo nextest run -p paigasus-observability -E 'test(memory_s3)'`
Expected: compile error.

- [ ] **Step 3: Implement the store type, the reads, `delete_scope`, `ScriptedStore`, the metric name and its `describe_counter!` line**

- [ ] **Step 4: Run the tests and the drift gate**

Run: the commands of Step 2. Expected: all PASS.
Run (repository root): `moon run repo:observability-drift`. Expected: pass.

- [ ] **Step 5: Commit**

Message: `feat(rs): add the S3 memory store reads and the scope delete (SMA-829)`.

---

### Task 5: `apply`, the writes, and the conformance suite without Docker

**Files:**
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/mod.rs` (`apply`)
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/testing.rs` (`EtagMode::ContentHash` is used here for the first time)

**Interfaces:**
- Consumes: Task 3 `Snapshot`, `precheck`, `result_of`; Task 4 `listing`. (`put_outcome` and `delete_outcome` get their first caller in Task 6.)
- Produces: the real `apply`. A private `const WRITE_CONCURRENCY: usize = 8`. A private `async fn run_phase(&self, scope: &MemoryScope, set: &MemoryCommitSet, indexes: &[usize], deadline: tokio::time::Instant, outcomes: &mut [Outcome])`.
- Each `Put` is a `put_opts` with `PutPayload::from(encode(&id, content))`, `PutMode::Create` for `Absent`, `PutMode::Update(UpdateVersion { e_tag: Some(v), version: None })` for `Version(v)`, and the attribute `Attribute::ContentType` = `text/plain; charset=utf-8`.
- Each `Delete { v }` is a `head`, then `Conflict` when it is absent or its ETag is not `v`, else a `delete`.

The flow (spec §6.5): `set.is_empty()` → `Ok(())` with no request. Then `listing(scope_path)` gives the `Snapshot` (an error is a bare `Store`), then `precheck`. Then the `Put` phase, then the `Delete` phase only when every `Put` is `Confirmed`. Inside a phase at most 8 writes run. In this task every write error goes straight to `Failed(Store(store_text(..)))`; Task 6 adds the check after an error, the deadline and the `Partial` counter. The result is `result_of`.

- [ ] **Step 1: Write the failing tests**

- `in_memory_passes_the_conformance_suite`: `conformance::run_all(&S3MemoryStore::new(Arc::new(InMemory::new()), prefix("mem"), Duration::from_secs(10))).await`.
- `content_hash_etags_pass_the_conformance_suite`: the same over `ScriptedStore::new(EtagMode::ContentHash)`. This is the no-Docker proof of the write ID (`InMemory` takes each ETag from one counter, `memory.rs:209-217`, so it hides an ABA bug).
- `a_rename_writes_the_put_before_the_delete`: `[Delete /old, Put /new]` over a `ScriptedStore` that records the call order: the `PUT` of `/new` comes before the `DELETE` of `/old`.
- `the_stored_object_is_the_envelope`: after `[Put /tasks/t1.md "x"]`, the raw object at `mem/<org>/<agent>/<principal>/tasks/t1.md` starts with `paigasus-memory/1 `, is 52 bytes, and has the content type `text/plain; charset=utf-8`.

- [ ] **Step 2: Run the tests and see them fail**

Run: `cargo nextest run -p paigasus-agent-host -E 'test(memory_s3)'`
Expected: the conformance tests FAIL with "the S3 memory store cannot apply yet".

- [ ] **Step 3: Implement `apply` and the call record of `ScriptedStore`**

- [ ] **Step 4: Run the tests and see them pass**

Run: the command of Step 2. Expected: all PASS, and both conformance tests run in under 5 s.

- [ ] **Step 5: Commit**

Message: `feat(rs): apply S3 memory change sets with puts before deletes (SMA-829)`.

---

### Task 6: The failure path of `apply`: the checks, `Partial`, the deadline

**Files:**
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/mod.rs`
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/testing.rs` (hooks and faults)
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs`, `rs/crates/services/paigasus-agent-host/src/main.rs` (the second counter)

**Interfaces:**
- Produces: `pub const AGENT_HOST_MEMORY_S3_PARTIAL_APPLIES_TOTAL: &str = "agent_host_memory_s3_partial_applies_total";` (no label). HELP: "S3 memory change sets that were partly applied. The store may hold part of the set."
- Produces (`testing.rs`): `pub enum Hook { BeforeFirstHead, BeforeFirstPut, BeforeFirstDelete }` and `ScriptedStore::on(&self, hook: Hook, f: impl FnOnce(&ScriptedStore) + Send + 'static)` (the other writer); `pub enum Fault { Fail(object_store::Error), WriteThenFail(object_store::Error), Stall }` and `ScriptedStore::fault_put(&self, nth: usize, fault: Fault)`, `fault_delete(&self, nth: usize, fault: Fault)`, `fail_next_get(&self)`. `Stall` waits on `std::future::pending()`.
- Adds to `apply`: after a `Put` error, a `get` of the key and `put_outcome` with `Seen` (`Mine` when the decoded write ID is the ID of this `Put`, `Foreign` when `decode` refuses); after a `DELETE` error, a `head` and `delete_outcome`. After the first `Failed` or `Unknown` in a phase, no new write starts; the writes in flight finish; the rest stay `NotStarted`.
- Adds the deadline: `tokio::time::Instant::now() + apply_deadline`, read once at the start of `apply`. The listing runs under `timeout_at(deadline, …)` (expired: a bare `Store("the apply deadline expired (timeout)")`). In a phase, when the deadline expires: no new write starts; the in-flight futures are dropped and become `Unknown(Store("the apply deadline expired (timeout)"))`; each write that did not start becomes `Failed` with the same text. So an expiry before the first write is a bare `Store`.
- Adds on a `Partial` result: `tracing::warn!(applied, cause = cause_class(&cause), "the S3 memory change set was partly applied")` and the counter +1. A private `fn cause_class(e: &MemoryApplyError) -> &str` gives `conflict`, `cap`, `partial`, or for `Store(s)` the text `s` itself (safe: every adapter `Store` text is a fixed sentence and a class, per the Global Constraints).
- Adds the module doc of `mod.rs`: the five races of spec §6.7, one sentence each, and "A `Put` has no race: the store checks its condition."

- [ ] **Step 1: Write the failing tests**

Interleaving (spec §7), each over `ScriptedStore` in `ContentHash` mode:
- `a_racing_create_is_a_conflict`: `Hook::BeforeFirstPut` writes `/a` = `other`; `[Put /a Absent]` → `Conflict { /a }`, and `/a` holds `other`.
- `a_racing_update_is_a_conflict`: `/a` exists at version `v`; the hook overwrites `/a`; `[Put /a Version(v)]` → `Conflict { /a }`, and `/a` holds the other content.
- `a_racing_change_before_the_delete_head_is_a_conflict` (the stale-delete test): `Hook::BeforeFirstHead` overwrites `/a`; `[Delete /a v]` → `Conflict { /a }`; `/a` holds the newer content.
- `a_racing_change_between_head_and_delete_is_lost` (race 3, documented): `Hook::BeforeFirstDelete` overwrites `/a`; `[Delete /a v]` → `Ok(())`, and `/a` is absent. The test comment names spec §6.7 item 3.
- `a_later_racing_write_gives_partial`: `[Put /a, Put /b Absent]` with the hook on the second `PUT` creating `/b` → `Partial { applied: 1, failed_at: /b, cause: Conflict { /b } }`.

Faults:
- `the_second_put_fails`: `[Put /a, Put /b, Delete /c]`, `fault_put(2, Fail(Generic))` → `Partial { applied: 1, failed_at: /b }`; `/a` is new; `/c` exists; no `DELETE` was sent (the call record).
- `a_refused_rename_put_keeps_the_old_file`: `[Put /new, Delete /old]`, `fault_put(1, Fail(AlreadyExists))` with `/new` absent → bare `Conflict { /new }`; `/old` exists.
- `a_failed_rename_delete_keeps_both`: `fault_delete(1, Fail(Generic))` → `Partial { applied: 1, failed_at: /old }`; `/new` and `/old` exist.
- `a_retried_put_that_landed_is_ok`: `fault_put(1, WriteThenFail(AlreadyExists))` → `Ok(())`.
- `a_timed_out_put_that_landed_is_ok`: `fault_put(1, WriteThenFail(Generic with HttpError Timeout))` → `Ok(())`.
- `a_timed_out_put_with_a_failed_check_is_partial`: `WriteThenFail(timeout)` and `fail_next_get()` → `Partial { applied: 0, failed_at: /a }`.
- `a_timed_out_put_that_did_not_land_is_a_bare_store_error`: `fault_put(1, Fail(timeout))` → `Store(..)` whose text holds `timeout`.
- `the_deadline_bounds_apply` (`#[tokio::test(start_paused = true)]`): deadline 2 s, `fault_put(2, Stall)` on `[Put /a, Put /b]` (both start at once; `/a` lands, `/b` stalls). The call is wrapped in an outer `tokio::time::timeout(Duration::from_secs(60), …)`, and the test asserts that the outer timeout does not expire, so a missing deadline fails and does not hang. Result: `Partial { applied: 1, failed_at: /b }` whose cause text holds `timeout`, and the elapsed paused time is `>= 2 s` and `< 2.1 s`.
- `an_expired_deadline_before_the_first_write_is_a_bare_store_error`: a `ScriptedStore` whose `list` stalls (add `stall_list(&self)`), deadline 1 s, the same outer timeout → bare `Store` with `timeout`.
- `partial_increments_the_counter`: the `the_second_put_fails` set under a `DebuggingRecorder` → `agent_host_memory_s3_partial_applies_total == 1`; a bare `Conflict` → 0.
- `names.rs`: add the new constant to `the_agent_host_memory_s3_families_are_registered`.

- [ ] **Step 2: Run the tests and see them fail**

Run: `cargo nextest run -p paigasus-agent-host -E 'test(memory_s3)' --no-fail-fast`
Expected: compile error (no `Hook`, no `Fault`, no counter constant).

- [ ] **Step 3: Implement the test-store hooks and faults first, run Step 2 again, and see the behaviour tests fail**

Expected: the "landed is ok", "failed check is partial", deadline and counter tests FAIL. The four interleaving tests already PASS: the store checks the condition. They become proof in Task 12, rows 4 to 6.

- [ ] **Step 4: Implement the checks, the deadline, the warn line, the counter and the module doc**

- [ ] **Step 5: Run the tests, the conformance tests and the drift gate**

Run: the command of Step 2, then `moon run repo:observability-drift`. Expected: all PASS.

- [ ] **Step 6: Commit**

Message: `feat(rs): check every failed S3 memory write and bound apply by a deadline (SMA-829)`.

---

### Task 7: Config: `memory.backend = "s3"` and `memory.s3`

**Files:**
- Modify: `rs/crates/services/paigasus-agent-host/src/config.rs` (`MemoryConfig` ~157-170, `ConfigError` ~361-410, `validate` ~639-716, the test `memory_backend_s3_is_refused_for_now` ~1125)
- Modify: every `MemoryConfig { … }` literal (today only `src/adapters/memory_commit.rs:47`)
- Modify: `rs/crates/services/paigasus-agent-host/README.md` (the `memory.*` rows ~40-44 and a new "S3 memory backend" section)
- Modify: `rs/crates/services/paigasus-agent-host/agent-host.toml.example` (the `[memory]` block ~59-65)
- Modify: `rs/CLAUDE.md` (the CA knob entry ~101-110: "four" becomes "five", and name `memory.s3.extra_ca_bundle_path` as an ADD knob)

**Interfaces:**
- Consumes: Task 1 `KeyPrefix::parse`, `PrefixError`.
- Produces: `MemoryBackend::S3` (`#[serde(rename_all = "lowercase")]` gives `"s3"`). It stays `Copy`.
- Produces: `MemoryConfig { pub backend, pub caps, #[serde(default)] pub s3: Option<MemoryS3Config> }`.
- Produces: `pub struct MemoryS3Config { pub bucket: String, pub prefix: String, #[serde(default = "…")] pub region: String /* "us-east-1" */, #[serde(default)] pub endpoint: Option<String>, #[serde(default)] pub path_style: Option<bool>, #[serde(default)] pub allow_http: bool, #[serde(default)] pub extra_ca_bundle_path: Option<String>, #[serde(default = "…")] pub apply_deadline_secs: u64 /* 10 */ }` (`Debug, Clone, PartialEq, Eq, Deserialize, Serialize`). The doc of `extra_ca_bundle_path` says that it ADDs to the platform roots and that the bundle must hold roots only.
- Produces: `MemoryS3Config::path_style(&self) -> bool` (`self.path_style.unwrap_or(self.endpoint.is_some())`) and `MemoryS3Config::prefix(&self) -> KeyPrefix` (call only after `validate`; it `expect`s with a text that names `validate`).
- Produces: `ConfigError` variants, one per refusal: `MemoryS3Missing`, `MemoryS3Unused`, `MemoryS3Bucket(String)`, `MemoryS3Prefix(PrefixError)`, `MemoryS3EndpointUnparsable`, `MemoryS3EndpointScheme`, `MemoryS3EndpointUserinfo`, `MemoryS3EndpointQuery`, `MemoryS3EndpointFragment`, `MemoryS3HttpNotAllowed`, `MemoryS3CaPathBlank`, `MemoryS3DeadlineOutOfRange { deadline: u64, ttl: u64 }`. No endpoint variant holds the endpoint text (it can hold userinfo, AG5).

The rules (spec §6.9): bucket 3 to 63 characters of `[a-z0-9.-]`, starts and ends with a letter or digit, no `..`. Prefix by `KeyPrefix::parse`. Endpoint parsed by `reqwest::Url::parse`: scheme `http` or `https`; empty `username()` and no `password()`; no query; no fragment; `http` needs `allow_http`. CA path: `Some` and empty or whitespace-padded → `MemoryS3CaPathBlank` (the IAM rule, `paigasus-iam/src/config.rs:1343-1348`). `apply_deadline_secs` is `>= 1` and `< lease.ttl_secs`. `backend = "s3"` with `s3 == None` → `MemoryS3Missing`; `backend = "postgres"` with `s3 == Some` → `MemoryS3Unused`. The figment chain does not change.

- [ ] **Step 1: Replace the old test and write the failing tests**

Delete `memory_backend_s3_is_refused_for_now`. Add, with `figment::Jail`:
- `memory_s3_loads_from_toml`: an `agent-host.toml` with `[memory] backend = "s3"` and `[memory.s3] bucket = "paigasus-mem"`, `prefix = "mem"`, `endpoint = "http://127.0.0.1:9000"`, `allow_http = true` loads and validates; `region == "us-east-1"`, `apply_deadline_secs == 10`, `path_style() == true`.
- `memory_s3_loads_from_the_environment`: `AGENT_HOST_MEMORY__BACKEND=s3`, `AGENT_HOST_MEMORY__S3__BUCKET=paigasus-mem`, `AGENT_HOST_MEMORY__S3__PREFIX=mem` load; `path_style() == false` (no endpoint).
- `an_aws_variable_changes_no_setting`: with `AWS_REGION=eu-west-1` and `AWS_ENDPOINT_URL=http://evil` set in the jail, `region == "us-east-1"` and `endpoint == None`.
- `memory_s3_refusals` (a table over a valid base config): no section → `MemoryS3Missing`; a section with `postgres` → `MemoryS3Unused`; buckets `ab`, 64 characters, `Ab-c`, `-abc`, `abc-`, `a..b`, `a_b` → `MemoryS3Bucket`; prefix `a//b` → `MemoryS3Prefix(EmptySegment)`; endpoint `not a url` → `Unparsable`; `ftp://h` → `Scheme`; `https://u:p@h` → `Userinfo`; `https://h?x=1` → `Query`; `https://h#f` → `Fragment`; `http://h` without `allow_http` → `HttpNotAllowed`; CA path `""` and `" /x"` → `CaPathBlank`; `apply_deadline_secs = 0` and `= 30` with `ttl_secs = 30` → `DeadlineOutOfRange { deadline, ttl: 30 }`.
- `no_endpoint_error_text_holds_the_endpoint`: the `Display` of `MemoryS3EndpointUserinfo` does not contain `u:p`.
- `an_endpoint_with_a_trailing_slash_is_valid` (Review Focus 5): `http://minio.local:9000/` with `allow_http` validates.
- `memory_defaults` (existing, ~850) also asserts `cfg.memory.s3 == None`.

- [ ] **Step 2: Run the tests and see them fail**

Run: `cargo nextest run -p paigasus-agent-host -E 'test(config::)'`
Expected: compile error (no `MemoryS3Config`).

- [ ] **Step 3: Implement the config, then write the docs**

README: add the eight `memory.s3.*` rows with their `AGENT_HOST_MEMORY__S3__*` names and defaults, and change the `memory.backend` row to `postgres` or `s3`. Add a section "S3 memory backend" that says: a backend switch does not move data; the bucket must not keep versions (the boot refuses it); each object starts with a 51-byte header line, so objects are not plain Markdown; do not write objects under the prefix, and delete a foreign object or an unparsable key (for example `a//b`) with an S3 tool; the prefixes of two deployments on one bucket must not nest; the store must give strong read-after-write consistency; the credential variables that the host reads (the ten names of Task 8); the proxy variables that reqwest reads (Task 8 result); with no credential variable, the client asks the instance metadata endpoint, and off AWS the probe fails with `unauthenticated` or `timeout`; with `path_style = false` and an endpoint, the endpoint must hold the bucket; and the five races of spec §6.7 in one paragraph. Example file: a commented `[memory.s3]` block with every key.

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo nextest run -p paigasus-agent-host -E 'test(config::) | test(memory_commit)'`. Expected: all PASS. If Task 1 added a `reqwest` machete ignore entry, remove it now and run `moon run repo:machete` (expected: pass).

- [ ] **Step 5: Commit**

Message: `feat(rs): add the s3 memory backend config (SMA-829)`.

---

### Task 8: The client

**Files:**
- Create: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/client.rs`
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/mod.rs` (`pub mod client;`)

**Interfaces:**
- Consumes: Task 7 `MemoryS3Config`.
- Produces: `pub const ENV_ALLOWLIST: [&str; 10] = ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN", "AWS_WEB_IDENTITY_TOKEN_FILE", "AWS_ROLE_ARN", "AWS_ROLE_SESSION_NAME", "AWS_ENDPOINT_URL_STS", "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", "AWS_CONTAINER_CREDENTIALS_FULL_URI", "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE"];`
- Produces: `pub const REQUEST_TIMEOUT: Duration = 10 s`, `CONNECT_TIMEOUT = 5 s`, `MAX_RETRIES: usize = 3`, `RETRY_TIMEOUT = 10 s`.
- Produces: `pub fn s3_builder(cfg: &MemoryS3Config, env: impl IntoIterator<Item = (String, String)>) -> Result<AmazonS3Builder, ClientError>`.
- Produces: `pub fn build_store(cfg: &MemoryS3Config, env: impl IntoIterator<Item = (String, String)>) -> Result<Arc<dyn ObjectStore>, ClientError>`. It calls `let _ = rustls::crypto::ring::default_provider().install_default();` first.
- Produces: `pub enum ClientError { CaRead { path: String }, CaEmpty { path: String }, CaParse { path: String }, Build { class: String } }` (`thiserror`). No variant holds a credential.

The builder (spec §6.8): start from `AmazonS3Builder::new()`. For each `(name, value)` of `env`: when `name` is in `ENV_ALLOWLIST` and `value` is not empty, `with_config(name.to_ascii_lowercase().parse()?, value)`. Then from the config: `with_bucket_name`, `with_region`, `with_endpoint` (when set), `with_virtual_hosted_style_request(!cfg.path_style())`, `with_allow_http(cfg.allow_http)`, `with_conditional_put(S3ConditionalPut::ETagMatch)`, `with_disable_bulk_delete(true)`, `with_retry(RetryConfig { max_retries: MAX_RETRIES, retry_timeout: RETRY_TIMEOUT, ..Default::default() })`, and one `with_client_options(ClientOptions::new().with_timeout(REQUEST_TIMEOUT).with_connect_timeout(CONNECT_TIMEOUT).with_allow_http(cfg.allow_http)` plus one `with_root_certificate` per certificate of `Certificate::from_pem_bundle`). An empty bundle → `CaEmpty`. The config values are set AFTER the environment loop, so a config value always wins. Never call `with_client_options` twice: it replaces every option.

- [ ] **Step 1: Write the failing tests**

- `every_allowlisted_name_is_a_config_key`: each name of `ENV_ALLOWLIST`, lower-cased, parses as `AmazonS3ConfigKey`.
- `the_environment_cannot_change_the_client`: env `AWS_ENDPOINT_URL_S3=http://evil`, `AWS_ENDPOINT=http://evil`, `AWS_ENDPOINT_URL=http://evil`, `AWS_ALLOW_INVALID_CERTIFICATES=true`, `AWS_SKIP_SIGNATURE=true`, `AWS_DISABLE_SYSTEM_CERTIFICATES=true`, `AWS_CONDITIONAL_PUT=disabled`, `AWS_REGION=eu-west-1`, and a config with endpoint `https://s3.example`. `get_config_value` gives: `Endpoint` = `https://s3.example`; `S3Endpoint` = `None`; `SkipSignature` = `false`; `ConditionalPut` = the `ETagMatch` text; `Region` = `us-east-1`; `DisableBulkDelete` = `true`; `Client(ClientConfigKey::AllowInvalidCertificates)` = `false`.
- `allowlisted_credentials_reach_the_builder`: env `AWS_ACCESS_KEY_ID=AKIDTEST`, `AWS_SECRET_ACCESS_KEY=secret`, `AWS_SESSION_TOKEN=tok` → `AccessKeyId`, `SecretAccessKey`, `Token` hold them.
- `an_empty_allowlisted_variable_is_not_copied` (Review Focus 4): `AWS_ACCESS_KEY_ID=""` → `AccessKeyId` is `None`.
- `an_empty_ca_bundle_is_an_error` and `a_missing_ca_bundle_is_an_error`: a temp file with no PEM block → `CaEmpty`; a path that does not exist → `CaRead`.
- `a_ca_bundle_adds_roots`: a temp file with one self-signed PEM certificate → `s3_builder` is `Ok` and `build_store` is `Ok`.
- `no_client_error_text_holds_a_credential`: the `Display` of each `ClientError` built from an env with `AWS_SECRET_ACCESS_KEY=secret-value` holds no `secret-value`.

- [ ] **Step 2: Run the tests and see them fail**

Run: `cargo nextest run -p paigasus-agent-host -E 'test(memory_s3::client)'`. Expected: compile error.

- [ ] **Step 3: Implement `client.rs`**

- [ ] **Step 4: Measure the proxy variables (the sixth Review Focus item)**

This is a measurement, not a test. Read the locked reqwest 0.13 source (`~/.cargo/registry/src/*/reqwest-0.13.*/src/async_impl/client.rs`): find whether `ClientBuilder` reads the system proxy variables when no `proxy` and no `no_proxy` is set. Record the result, with the file and line, in the doc comment of `build_store`. If reqwest reads `HTTPS_PROXY` by default, write the proxy variables into the README section of Task 7 as variables that the client reads. Sven decided to keep them (2026-11-22). Do not add a work-around that turns off the proxy lookup.

- [ ] **Step 5: Run the tests and machete**

Run: the command of Step 2, then `moon run repo:machete`. Expected: all PASS. Remove a `rustls` machete ignore entry if Task 1 added one.

- [ ] **Step 6: Commit**

Message: `feat(rs): build the S3 memory client from config and a credential allowlist (SMA-829)`.

---

### Task 9: The boot probe

**Files:**
- Create: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/probe.rs`
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/mod.rs` (`pub mod probe;`)
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/testing.rs` (the misbehaviour switches)

**Interfaces:**
- Consumes: Task 1 `probe_path`, `probe_dir`; Task 2 `class`, `is_refusal`.
- Produces: `pub async fn run(store: &dyn ObjectStore, prefix: &KeyPrefix, endpoint: &str, bucket: &str) -> Result<ProbeReport, ProbeError>`. `endpoint` is the config endpoint, or `the AWS default endpoint` when none is set.
- Produces: `pub struct ProbeReport { pub file_and_dir_of_one_name: bool }`.
- Produces: `pub struct ProbeError { pub endpoint: String, pub bucket: String, pub step: u8, pub failure: ProbeFailure }` with a `Display` per step, and `pub enum ProbeFailure { VersionedBucket, CreateNotRefused, EtagRepeats, StaleUpdateNotRefused, EtagFormDiffers, ListedEtagRefused, DeleteKept, Request { class: String } }`.
- Produces (`testing.rs`): `ScriptedStore` switches `ignore_conditions()`, `ignore_if_match()` (it refuses a `Create` over a present key but accepts every `Update`), `EtagMode::SizeOnly`, `return_version_id()`, `refuse_update_with_listed_etag()`, `flat_namespace()` (it refuses a `PUT` of `K` when `K/…` exists, and the reverse). `ignore_deletes()` exists from Task 4.

The steps (spec §6.8), with `K = probe_path(prefix, Uuid::new_v4())` and `A`, `B` of one length:
1. `PUT K = A` `Create`. It must succeed, and `PutResult::version` must be `None`, else `VersionedBucket`.
2. `PUT K = A` `Create`. It must be refused (`is_refusal`), else `CreateNotRefused`.
3. `PUT K = B` `Update(etag(A))`. It must succeed, and `etag(B) != etag(A)`, else `EtagRepeats`.
4. `PUT K` `Update(etag(A))`. It must be refused, else `StaleUpdateNotRefused`.
5. List `probe_dir`, find `K`. The listed ETag, the `get` ETag and the `head` ETag must equal `etag(B)` after `version_of`, else `EtagFormDiffers`. Then `PUT K` `Update(listed ETag)` must succeed, else `ListedEtagRefused`.
6. `DELETE K`, then `HEAD K` must be `NotFound`, else `DeleteKept`.
7. `PUT K/x` and `PUT K` (both `Create`), then delete both. If the second `PUT` fails, `file_and_dir_of_one_name = false` and `tracing::warn!` with the exact text: "The memory store at {endpoint} cannot hold a file and a directory of one name. A memory change set that replaces a file with a directory of the same name will fail." Step 7 never fails the probe.

The texts: for steps 2 and 4, and for a `status=501` class at steps 1, 3 and 4: "The memory store at {endpoint}, bucket {bucket}, does not support conditional writes: it accepted a {create-only | conditional} put that it must refuse. The S3 memory backend needs If-None-Match and If-Match on PutObject." (Use "create-only" for steps 1 and 2, "conditional" for steps 3 and 4.) Any other request error: "The memory store at {endpoint}, bucket {bucket}, failed the boot probe at step {n}: {class}." Write one fixed sentence each for `VersionedBucket`, `EtagRepeats`, `EtagFormDiffers`, `ListedEtagRefused` and `DeleteKept`, with the endpoint, the bucket and the step.

- [ ] **Step 1: Write the failing tests (over `ScriptedStore`)**

- `a_good_store_passes`: `Counter` mode → `Ok(ProbeReport { file_and_dir_of_one_name: true })`, and no key is left under `mem/_probe/`.
- `ignored_conditions_fail_at_step_2`: `ignore_conditions()` → `step == 2`, `CreateNotRefused`, and the `Display` contains `does not support conditional writes` and `create-only`.
- `a_store_that_ignores_if_match_fails_at_step_4`: `ignore_if_match()` → `step == 4`, and the text contains `conditional put`.
- `a_size_etag_fails_at_step_3`: `SizeOnly` → `step == 3`, `EtagRepeats`.
- `a_versioned_bucket_fails_at_step_1`: `return_version_id()` → `step == 1`, `VersionedBucket`.
- `a_refused_listed_etag_fails_at_step_5`: `refuse_update_with_listed_etag()` → `step == 5`, `ListedEtagRefused`.
- `a_delete_that_does_nothing_fails_at_step_6`: `ignore_deletes()` → `step == 6`, `DeleteKept`.
- `a_flat_namespace_passes_with_a_warn`: `flat_namespace()` → `Ok(ProbeReport { file_and_dir_of_one_name: false })`.
- `a_probe_error_holds_no_credential`: each `Display` above contains the endpoint and the bucket, and no `AKID` or `secret` text.

- [ ] **Step 2: Run the tests and see them fail**

Run: `cargo nextest run -p paigasus-agent-host -E 'test(memory_s3::probe)'`. Expected: compile error.

- [ ] **Step 3: Implement `probe.rs` and the switches**

- [ ] **Step 4: Run the tests and see them pass**

Run: the command of Step 2. Expected: all PASS.

- [ ] **Step 5: Commit**

Message: `feat(rs): add the S3 memory boot probe (SMA-829)`.

---

### Task 10: `MemoryCommit::External` and the boot

Keep this task small and in one commit. SMA-790 (another worktree) can change `memory_commit.rs` and `AppState`.

**Files:**
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/memory_commit.rs`
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/http/mod.rs` (`AppState` ~37-41)
- Modify: `rs/crates/services/paigasus-agent-host/src/main.rs` (`boot()` ~234-279)
- Modify: the other `AppState { … }` literals: `src/adapters/boot.rs:146`, `src/adapters/http/mod.rs:107`, `tests/ingress_http.rs:50` and `:419`, `tests/ingress_pg.rs:33`

**Interfaces:**
- Consumes: Task 4 `S3MemoryStore::new`, Task 7 `MemoryS3Config`, Task 8 `build_store`, Task 9 `probe::run`.
- Produces: `pub enum MemoryCommit { InTransaction(PgMemoryStore), External(Arc<dyn MemoryStore>) }`, `Clone`, with a manual `Debug` that prints only `InTransaction` or `External`.
- Produces: `pub async fn build(cfg: &MemoryConfig, db: DatabaseConnection, env: impl IntoIterator<Item = (String, String)>) -> Result<MemoryCommit, MemoryBootError>`. It replaces `from_config`. For `S3`: `build_store`, then `probe::run`, then `External(Arc::new(S3MemoryStore::new(store, s3.prefix(), Duration::from_secs(s3.apply_deadline_secs))))`. Log `info` with the bucket and `file_and_dir_of_one_name`.
- Produces: `pub enum MemoryBootError { MissingS3Section, Client(#[from] ClientError), Probe(#[from] ProbeError) }` (`thiserror`).
- Produces: `AppState { …, pub memory: MemoryCommit }`. The tests set `memory: MemoryCommit::InTransaction(PgMemoryStore::new(DatabaseConnection::default()))`. A `pub` field of a `pub` struct in the library is not dead code.
- `boot()`: after the NATS checks and before `slot.install`, `let memory = MemoryCommit::build(&config.memory, db.clone(), utf8_env()).await?;`. `utf8_env()` is a private function of `main.rs`: `std::env::vars_os()` filtered to pairs where both parts convert to `String`.

- [ ] **Step 1: Write the failing tests (in `memory_commit.rs`)**

- `postgres_backend_builds_the_in_transaction_mode` (now `#[tokio::test]`, through `build`).
- `s3_backend_without_a_section_is_an_error`: `backend = S3`, `s3 = None` → `Err(MemoryBootError::MissingS3Section)`.
- `the_debug_text_names_only_the_variant`: `format!("{:?}", MemoryCommit::External(store))` is `External`.
- `the_external_store_is_the_same_arc`: `Arc::ptr_eq` of `store()` and the inner `Arc`.
- `s3_backend_with_a_bad_ca_path_is_a_client_error`: a CA path that does not exist → `Err(MemoryBootError::Client(ClientError::CaRead { .. }))` (no network).

- [ ] **Step 2: Run the tests and see them fail**

Run: `cargo nextest run -p paigasus-agent-host -E 'test(memory_commit)'`. Expected: compile error.

- [ ] **Step 3: Implement `build`, the `AppState` field, the literals and `boot()`; change `store()` so `External` returns the `Arc` clone**

- [ ] **Step 4: Run the crate tests and the lint**

Run: `cargo nextest run -p paigasus-agent-host --lib --test ingress_http` and `cargo clippy -p paigasus-agent-host --all-targets -- -D warnings` (the lint compiles `ingress_pg.rs` too). Expected: PASS and no warning.
Run: `cargo build -p paigasus-agent-host` (no dev features; a missing normal `reqwest` would show here). Expected: success.

- [ ] **Step 5: Commit**

Message: `feat(rs): build the S3 memory commit mode at boot (SMA-829)`.

---

### Task 11: The container tests

**Files:**
- Create: `rs/crates/services/paigasus-agent-host/tests/memory_s3.rs`
- Modify: `rs/.config/nextest.toml` (the agent-host filter ~191: add `or binary(memory_s3)`; it inherits `test-group = 'docker-containers'`)
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/mod.rs` (`version_of`, only if Step 1 shows a difference)

**Interfaces:**
- Consumes: Task 4 `S3MemoryStore`, Task 7 `MemoryS3Config`, Task 8 `build_store`, Task 9 `probe::run`, Task 10 `MemoryCommit::build`, `conformance::run_all`.
- Produces in the test file: `const RUSTFS: (&str, &str) = ("rustfs/rustfs", "1.0.6"); // sha256:ee71…044a` and the same for the two versitygw tags. `async fn start_rustfs() -> Option<S3Node>`, `async fn start_versitygw(tag: &str) -> Option<S3Node>`, and `struct S3Node { node: ContainerAsync<GenericImage>, endpoint: String, bucket: String }`. `fn env(node) -> Vec<(String, String)>` gives `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` (the injected environment; never `set_var`). `fn s3_config(node) -> MemoryS3Config` with `allow_http = true` and a unique prefix per test.

- [ ] **Step 1: Measure and record the start of the images**

Already known from `docker image inspect` and the entrypoint (2026-11-22): `rustfs/rustfs:1.0.6` exposes `9000/tcp`, has the volume `/data`, runs as user `rustfs`, reads `RUSTFS_ACCESS_KEY` and `RUSTFS_SECRET_KEY` (default `rustfsadmin`), and ships the CLI `/usr/bin/rc`. The versitygw images have `/bin/sh`; `v1.11.0` has the entrypoint `/usr/local/bin/docker-entrypoint.sh`, `v1.0.17` has `/app/versitygw`. Measure the rest by hand, and write the result into the doc comment of each start function:
- rustfs: `docker run --rm -d -p 9000 -e RUSTFS_ACCESS_KEY=… -e RUSTFS_SECRET_KEY=… rustfs/rustfs:1.0.6`; the readiness check is a signed request from inside the container: `rc alias set local http://127.0.0.1:9000 <key> <secret>` and then `rc bucket list local` exits 0. Record the `rc alias set` syntax and the time to ready. The bucket is `rc bucket create local/<bucket>`. Use `ContainerAsync::exec` with `ExecCommand` and its exit code, in a loop bounded by 30 s.
- versitygw: started as `--port :7070 posix <dir>` with `ROOT_ACCESS_KEY` and `ROOT_SECRET_KEY`, the directory inside the container. Measure whether `mkdir <dir>/<bucket>` by `exec` makes a bucket, or whether the start command must make it (`sh -c 'mkdir -p /gw/<bucket> && exec <entrypoint> --port :7070 posix /gw'`). Record the readiness check (the first successful `list` through `build_store`, bounded by 30 s).
- The raw keys: measure whether `rc pipe local/<bucket>/<key>` writes the keys `…/notes/` and `…/a//b` unchanged.

- [ ] **Step 2: Write the tests**

On `rustfs` (each one skips through `start_or_skip` and runs with `PAIGASUS_REQUIRE_DOCKER=1`):
- `rustfs_passes_the_conformance_suite`: `conformance::run_all(&S3MemoryStore::new(build_store(&cfg, env)?, prefix, 10 s))`.
- `rustfs_passes_the_boot_probe`: `probe::run(..)` is `Ok`; assert `file_and_dir_of_one_name` equals the value measured on the first run, with a comment that gives the date (a file system store is expected to give `false`).
- `rustfs_stores_the_envelope_under_the_scope_key`: after a create of `/tasks/t1.md`, the raw `get` of `<prefix>/<org>/<agent>/<principal>/tasks/t1.md` starts with `paigasus-memory/1 `.
- `rustfs_version_is_equal_for_list_get_and_head`: for one object, the listed ETag, the `get` ETag and the `head` ETag are byte-equal (spec §6.3). If they differ, change `version_of` to one normal form, add the unit test `version_of_normalizes_<form>` that pins it, and record the form.
- `rustfs_folder_marker_is_skipped_and_erased`: a raw `…/notes/` key; `list(root)` does not show it; `delete_scope` removes it; the raw listing of the scope is empty.
- `rustfs_empty_segment_key_makes_list_fail`: a raw `…/a//b` key; if the store keeps it, `list` returns `Store` with `unparsable_key`; if the store normalizes it, assert the measured behaviour and record it in the test doc.
- `rustfs_file_and_directory_of_one_name`: `/notes` present; `[Put /notes/x Absent, Delete /notes v]`. Assert the measured result: on a flat-namespace store a bare `Store` error with `/notes` still present (the `Delete` is not sent); on a key-value store `Ok(())`.
- `rustfs_delete_scope_removes_more_than_one_listing_page` (Review Focus 3): 1,001 raw objects under one scope (through the `ObjectStore`, 16 at a time), then `delete_scope` is `Ok` and the raw listing is empty.

On the negative image:
- `versitygw_1_0_17_fails_the_boot_probe_at_step_2`: `MemoryCommit::build` with `backend = S3` → `Err(MemoryBootError::Probe(e))` with `e.step == 2` and the text `does not support conditional writes`.

On the fallback image:
- `versitygw_fallback_passes_conformance_and_probe`: when `std::env::var_os("PAIGASUS_S3_FALLBACK")` is not `Some("1")`, print `SKIP[s3-fallback] set PAIGASUS_S3_FALLBACK=1 to run` and return. Else `run_all` and `probe::run` must pass. The switch to make the fallback the primary image is the one image constant.

- [ ] **Step 3: Add the binary to the nextest filter**

- [ ] **Step 4: Run the container tests**

Run: `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-agent-host --test memory_s3 --no-fail-fast`
Expected: all PASS (the fallback test prints the skip line).
Run: `PAIGASUS_REQUIRE_DOCKER=1 PAIGASUS_S3_FALLBACK=1 cargo nextest run -p paigasus-agent-host --test memory_s3 -E 'test(fallback)'`
Expected: PASS. Record the result in the commit body.
Run (repository root): `moon run repo:iam-docker-policy-single-site`. Expected: pass.

- [ ] **Step 5: Commit**

Message: `test(rs): run the S3 memory store against rustfs and versitygw (SMA-829)`, with the measured start commands, the step-7 result, the version-form result and the fallback result in the body.

---

### Task 12: The mutation proof

**Files:**
- Modify (temporarily, then restore): the files that each row names. No change stays.

The repo rule: "red-first is not proof — delete the feature". Each mutation must COMPILE under `-D warnings` (keep the removed code reachable, or add `let _ = …;`), else it proves nothing. Mark each insert with the comment `// MUTATION-SMA-829`. Restore by deleting the marked insert and putting back the original lines from the diff, NOT by `git checkout --` (it also discards a fix that a failed row made you write). After each row, `git diff --stat` must be empty.

Run each row with: `cargo nextest run -p paigasus-agent-host <filter> --no-fail-fast` (add `PAIGASUS_REQUIRE_DOCKER=1` for a container row).

| # | Mutation (file, change) | Filter | Must go red |
|---|---|---|---|
| 1 | `envelope.rs`: `WriteId::random` returns 32 zeros | `-E 'test(content_hash_etags_pass)'` and `--test memory_s3 -E 'test(rustfs_passes_the_conformance)'` | `aba_never_matches` in both |
| 2 | `mod.rs` `apply`: replace the `precheck` call by a `WritePlan` of every change, unchecked | `-E 'test(memory_s3)'` | `apply_is_all_or_nothing` and the guard checks of `run_all` |
| 3 | `plan.rs`: skip the guard loop | `-E 'test(memory_s3)'` | `a_guard_conflict_wins` and the SMA-843 checks of `run_all` |
| 4 | `mod.rs`: `PutMode::Update(..)` → `PutMode::Overwrite` | `-E 'test(a_racing_update)'` | `a_racing_update_is_a_conflict` |
| 5 | `mod.rs`: `PutMode::Create` → `PutMode::Overwrite` | `-E 'test(a_racing_create)'` | `a_racing_create_is_a_conflict` |
| 6 | `mod.rs`: drop the `HEAD` ETag compare of a delete | `-E 'test(before_the_delete_head)'` | `a_racing_change_before_the_delete_head_is_a_conflict` |
| 7 | `mod.rs`: run the `Delete` phase before the `Put` phase | `-E 'test(rename)'` | `a_rename_writes_the_put_before_the_delete`, `a_refused_rename_put_keeps_the_old_file` |
| 8 | `mod.rs`: after a `Put` error, skip the `get` (always `Seen::Absent`) | `-E 'test(landed_is_ok)'` | `a_retried_put_that_landed_is_ok`, `a_timed_out_put_that_landed_is_ok` |
| 9 | `plan.rs` `put_outcome`: `ReadFailed` → `Failed` | `-E 'test(failed_check_is_partial)'` | `a_timed_out_put_with_a_failed_check_is_partial` |
| 10 | `mod.rs`: no deadline (`timeout_at` removed) | `-E 'test(deadline)'` | `the_deadline_bounds_apply` (its outer 60 s paused-time timeout expires; it does not hang) |
| 11 | `probe.rs`: remove step N, for N = 1 to 6, one at a time | `-E 'test(memory_s3::probe)'` and, for N = 2, `--test memory_s3 -E 'test(1_0_17)'` | the probe test of step N, and the `v1.0.17` test for step 2 |
| 12 | `client.rs`: the env loop copies every `AWS_*` pair that parses as a config key (the rule of `from_env`, `aws/builder.rs:606-618`), not only `ENV_ALLOWLIST`. A literal `from_env()` reads the process environment, not the injected one, so it cannot red an injected-environment test. | `-E 'test(memory_s3::client)'` | `the_environment_cannot_change_the_client` (`SkipSignature` or `ConditionalPut` changes; `Endpoint` stays because the config is set after the loop) |
| 13 | `errors.rs`: `store_text` returns `e.to_string()` | `-E 'test(no_class_text_holds)'` | `no_class_text_holds_the_path_or_the_uri` |
| 14 | `mod.rs` `delete_scope`: skip the second listing | `-E 'test(delete_scope_fails_when)'` | `delete_scope_fails_when_a_key_remains` |

- [ ] **Step 1: Run rows 1 to 14 one at a time; for each, record red or green**
- [ ] **Step 2: For a row that stays green, stop: the test does not bite. Fix the test (not the mutation), re-run the whole battery from row 1, and record why**
- [ ] **Step 3: After the last row, check that `git status --porcelain` is empty and that `cargo nextest run -p paigasus-agent-host -E 'test(memory_s3)'` passes**
- [ ] **Step 4: Record the 14 results in the PR description (no commit is needed if no test changed; else commit `test(rs): make the S3 memory tests bite (SMA-829)`)**

---

### Task 13: The full gate graph

- [ ] **Step 1: Rebase on `origin/main`**

Run: `git fetch origin && git rebase origin/main`. If `memory_commit.rs`, `AppState` or the `boot()` lines conflict (SMA-790), keep both changes: SMA-790's fields and the `memory` field of Task 10. Then re-run `cargo nextest run -p paigasus-agent-host -E 'test(memory)'`.

- [ ] **Step 2: Re-measure the hazards in the real workspace**

Run the three `cargo tree` commands of Task 1 Step 1. Expected: the same results.

- [ ] **Step 3: Run the crate and repo gates**

Run: `cargo fmt --check`, `cargo clippy -p paigasus-agent-host --all-targets -- -D warnings`, `cargo build -p paigasus-agent-host`, `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-agent-host`.
Run (repository root): `moon run repo:deny repo:osv repo:machete repo:observability-drift repo:iam-docker-policy-single-site`.
Expected: all pass.

- [ ] **Step 4: Run the full `ci-targets` command**

Run the command between the `ci-targets` markers of the root `CLAUDE.md`, unchanged. Do not copy the markers into any file. On this Mac, pick one bash for the whole run, then re-run the gates that need the other bash directly, as the root `CLAUDE.md` section "This development Mac only" says. Read the `repo:actionlint` pipe preflight line before you trust its verdict.
Expected: every target passes, or each failure is a known host artifact that the root `CLAUDE.md` names and that the direct re-run clears.

- [ ] **Step 5: Hand over**

The first CI run of the PR is the first `rustfs` measurement on a runner (spec §7). If `rustfs` fails there and the fallback passes, change the one image constant and record why. Report the Task 8 proxy result to the coordinator.
