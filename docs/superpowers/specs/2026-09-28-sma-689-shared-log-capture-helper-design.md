# SMA-689: one shared log-capture helper for tests that assert on tracing output

- Linear: SMA-689 (milestone "CI & Tooling", labels `area:testing`, `Improvement`, priority Low)
- Related: SMA-686 (code review finding 10 found the duplication), SMA-635 (gateway copies),
  SMA-698 (IAM integration-test copy), SMA-707 (IAM `src/log_capture.rs`)
- Status: **approved by Sven on 2026-09-28** (section 12 records the decisions). Challenged in
  Stage 2 (APPROVE WITH CHANGES, findings folded). Re-checked against `origin/main` 213bc99d on
  2026-09-28 (section 13).

## 1. The problem

The issue names three copies of one test helper. On `origin/main` (83d446fc, and still on
213bc99d) there are **four**.
Each copy defines a `LogBuffer` that implements `tracing_subscriber::fmt::MakeWriter`, and a
`capture_logs()` that installs a thread-local subscriber with `tracing::subscriber::set_default`.

| # | File | Level | Visibility | Callers |
|---|---|---|---|---|
| C1 | `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs:792-828` (tests module) | `TRACE` | private | the `auth.rs` unit tests (row 9 needs TRACE) |
| C2 | `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs:45-84` | `INFO` | private | `chat_proxy.rs:500`, `:519` |
| C3 | `rs/crates/services/paigasus-iam/src/log_capture.rs` (whole file, `#[cfg(test)] mod log_capture;` in `src/lib.rs:13-14`) | `TRACE` | `pub(crate)` | `adapters/oidc/validator.rs:367`, `application/authenticate_token.rs:378` |
| C4 | `rs/crates/services/paigasus-iam/tests/support/mod.rs:956-998` | `TRACE` | `pub`, `#[allow(dead_code)]` | `authn_identities.rs:172`, `grpc_whoami.rs:150,362,393`, `http_authn.rs:338,492,520` |

The issue text still points at the SMA-686 copy in `validator.rs`. SMA-707 later moved that copy
into C3 and added the C4 copy for the integration tests. The problem is the same: every copy can
drift (level, ANSI setting, format), and every new crate or test binary that asserts on a log line
adds one more copy.

All four copies set `with_ansi(false)` and use the default text `fmt` format. Only the level
differs: C2 uses `INFO`, the others use `TRACE`.

Both service crates already depend on `paigasus-logging` as a normal dependency
(`paigasus-gateway/Cargo.toml:19`, `paigasus-iam/Cargo.toml:20`). Both `moon.yml` files already
list `paigasus-logging-rs` in `dependsOn` and its sources in `fileGroups.upstreams`.

## 2. Acceptance

From the issue, made exact:

1. **One helper exists.** The issue text is: "One helper exists. No crate defines its own
   `LogBuffer` or `capture_logs`." After the change,
   `git grep -nE 'struct LogBuffer|fn capture_logs' -- rs` returns matches only in
   `rs/crates/libs/paigasus-logging/src/test_support.rs`.
   `rs/crates/services/paigasus-iam/src/log_capture.rs` does not exist.
2. **All tests that used a copy pass under `cargo nextest`.** That is every test named in the
   "Callers" column of section 1, run with `cargo nextest run -p paigasus-gateway -p paigasus-iam`
   (the IAM Docker-backed tests need Docker, as today). All seven C4 call sites are in
   Docker-gated suites, which skip quietly without Docker (`rs/CLAUDE.md`). So the run for AC 2
   sets `PAIGASUS_REQUIRE_DOCKER=1` (section 5.2); a silent skip does not satisfy AC 2. The new
   helper tests in `paigasus-logging` also pass.
3. **`repo:machete` and `repo:deny` stay green.** No new external dependency enters the tree.
   The `tracing-subscriber` dev-dependency of each service crate is removed, because no test uses
   it directly any more (section 4.4).
4. **Behavior is unchanged per test.** The `auth.rs` tests, the IAM unit tests and the IAM
   integration tests still capture at `TRACE`. `chat_proxy.rs` still captures at `INFO`.
5. **The runner limit is written down.** The helper's module doc states that it supports
   `cargo nextest` (one process per test) and that a log assertion can flake under `cargo test`
   (section 6).
6. **The production binaries do not contain the helper.** The build of the `paigasus-gateway`
   and `paigasus-iam` binaries compiles `paigasus-logging` without the `test-support` feature.
   The proof is the real compiler call (section 5.3).

## 3. Decisions

**D1. Put the helper behind a `test-support` feature of `paigasus-logging`, not in a new crate.**
The issue names both candidates. The feature wins on every point that matters here:

| | `paigasus-logging` feature (chosen) | new dev-only crate |
|---|---|---|
| New `members` line in `rs/Cargo.toml` | no | yes (A9 of `repo:affected-smoke`) |
| New `moon.yml`, `dependsOn` edge, `fileGroups.upstreams` in both services | no (already present) | yes (A6 asserts strict equality) |
| CODEOWNERS regeneration | no | yes |
| New-crate ADR/guideline step | no | yes |
| Topic fit | the crate owns the logging dependencies (`tracing`, `tracing-subscriber`), so it can own a test view of them | neutral |

Note: the helper captures the text `fmt` format, not the JSON shape that `paigasus_logging::init`
writes. Captured text is therefore not evidence of the production line format. The module doc
says so.

The external `tracing-test` crate is also rejected. It adds a new external dependency (a
`repo:deny` and `repo:osv` surface), it installs a global subscriber, and it changes the call
shape to an attribute macro, so all 40 call sites change more than an import path.

A third option, one shared `.rs` file included with `#[path = ...]` from each crate, is rejected.
It bypasses the crate boundary, it hides the dependency from Moon and from cargo, and each include
still compiles a separate type.

**D2. The module is `paigasus_logging::test_support`, compiled under
`#[cfg(any(test, feature = "test-support"))]`.** The `test` half makes the crate's own unit tests
compile and lint the module under `cargo clippy --all-targets` (the Moon `lint` task,
`.moon/tasks/rust.yml:79`) without a self dev-dependency. The feature half is what the services
turn on. The feature has no dependencies of its own: `tracing` and `tracing-subscriber` are
already normal dependencies of `paigasus-logging`, and `tracing-subscriber`'s default features
(which include `fmt` and `ansi`) are on at the workspace level.

**D3. The services turn the feature on in `[dev-dependencies]` only.** Each service lists
`paigasus-logging` a second time, under `[dev-dependencies]`, with
`features = ["test-support"]` and the same `path`/`version`. Cargo allows a dependency in both
tables. With `resolver = "3"` (resolver-2 feature rules), a dev-dependency feature is activated
only when a test, bench or example target is built. This is the same pattern the gateway already
uses for figment's `test` feature (`paigasus-gateway/Cargo.toml:85-89`).

**D4. The API keeps the current call shape.** Callers write `let (logs, _guard) = capture_logs();`
today, in 40 places (1 in `auth.rs`, 2 in `chat_proxy.rs`, 8 in `validator.rs`, 22 in `authenticate_token.rs`, 7 in the IAM integration tests). The new API keeps that tuple:

```rust
#[doc(hidden)]
pub struct LogBuffer(/* Arc<Mutex<Vec<u8>>> */);
impl LogBuffer { pub fn text(&self) -> String; }
#[must_use = "bind the guard to a named variable, or the capture stops at once"]
pub fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard);            // TRACE
#[must_use = "bind the guard to a named variable, or the capture stops at once"]
pub fn capture_logs_at(level: tracing::Level) -> (LogBuffer, tracing::subscriber::DefaultGuard);
```

`#[must_use]` catches a discarded return value. It does not catch `let (logs, _) = capture_logs();`,
which drops the guard at once and has no lint. A struct that owns the guard would remove that risk
fully. This spec keeps the tuple anyway, to limit the churn to an import path at 40 call sites.
The module doc states the rule (D6).

`capture_logs()` is `capture_logs_at(Level::TRACE)`. TRACE is the default because three of four
copies use it, and a "never logged" assertion is only sound at TRACE. `chat_proxy.rs` calls
`capture_logs_at(Level::INFO)`, which keeps its current behavior (AC 4). The `LogBuffer` field
stays private; `text()` is the only reader, as in every copy today. `LogBuffer` also keeps its
`Clone`, `Default`, `std::io::Write` and `MakeWriter` impls, because the subscriber needs them.

`LogBuffer` carries `#[doc(hidden)]` (Sven, 2026-09-28). It stays `pub`, because it is part of
the return type, but it is an implementation detail, not a supported extension point. A caller
that builds its own subscriber on it is not supported.

No other options are added (no JSON format, no ANSI switch, no filter string). YAGNI: no caller
needs them.

**D5. Callers import from `paigasus_logging::test_support` directly.** No crate keeps a local
re-export or wrapper. The IAM integration tests change `support::capture_logs()` to
`paigasus_logging::test_support::capture_logs()` (or a `use` at the top of the file). A local
re-export in `tests/support/mod.rs` would need `#[allow(unused_imports)]` in every test binary that
does not call it, and it would read as a fifth definition to the next reader.

**D6. The helper doc keeps the two usage rules that the copies state today.** Bind the guard to a
named variable (`_guard`), never to `_`, or it drops at once. `#[tokio::test]` is current-thread,
so the thread-local subscriber sees every task of the test, spawned tasks included. A
`multi_thread` runtime test would not see events from other worker threads.

## 4. Design

### 4.1 `rs/crates/libs/paigasus-logging/Cargo.toml`

Add:

```toml
[features]
# Test-only log capture (`test_support` module). Turn it on from [dev-dependencies] only (SMA-689).
test-support = []
```

`publish = false` stays. No dependency change.

### 4.2 `rs/crates/libs/paigasus-logging/src/test_support.rs` (new)

SPDX header. The module doc states: the purpose; the two usage rules of D6; the runner limit of
section 6 (supported runner: `cargo nextest`); and that the captured text is the `fmt` text
format, not the production JSON line format of `init`. The body is the C3 code, made `pub`, plus
`capture_logs_at`. `LogBuffer` gets `#[doc(hidden)]` (D4). It uses `tracing_subscriber::fmt().with_writer(buffer.clone()).with_ansi(false)
.with_max_level(level).finish()`, exactly as the copies do.

In `src/lib.rs`: `#[cfg(any(test, feature = "test-support"))] pub mod test_support;` and one line
in the crate doc that names the feature.

### 4.3 The four call sites

- **C1** `paigasus-gateway/src/adapters/http/auth.rs`: delete the `LogBuffer` struct, its impls
  and `capture_logs` (and the section comment that says a second copy exists). Add
  `use paigasus_logging::test_support::capture_logs;` in the tests module. Keep the note that row 9
  needs TRACE, next to row 9 or on the `use` line. `Arc` and `Mutex` stay imported: the tests
  module uses them elsewhere (`auth.rs:536`, `:545`).
- **C2** `paigasus-gateway/tests/chat_proxy.rs`: delete the helper block (lines 45-84). Call
  `capture_logs_at(tracing::Level::INFO)` at lines 500 and 519. Keep `use std::sync::Arc;`
  (line 24): `chat_proxy.rs:206-207` still uses `Arc::new`.
- **C3** `paigasus-iam`: delete `src/log_capture.rs` and the `#[cfg(test)] mod log_capture;` lines
  in `src/lib.rs`. Change `use crate::log_capture::capture_logs;` to
  `use paigasus_logging::test_support::capture_logs;` in `validator.rs:367` and
  `authenticate_token.rs:378`. Remove any comment that says a crate cannot share a `cfg(test)`
  helper.
- **C4** `paigasus-iam/tests/support/mod.rs`: delete the SMA-698 log-capture block (the
  `LogBuffer` struct, impls and `capture_logs`). Keep both constants, `JIT_FAILURE_LINE` (SMA-698)
  and `JIT_DISABLED_LINE` (SMA-707, used at `grpc_whoami.rs:164`). The block header comment at
  `tests/support/mod.rs:956` sits above both; rewrite it so that it does not refer to the deleted
  helper. Keep `use std::sync::{Arc, RwLock};` (line 52): `Arc` is still used at `:171` and
  `:267`.
  Change the seven `support::capture_logs()` calls in `authn_identities.rs`, `grpc_whoami.rs` and
  `http_authn.rs`.
- **C3, new callers from SMA-646 (conditional).** PR 348 (SMA-646, open on 2026-09-28) adds two
  more `use crate::log_capture::capture_logs;` imports, in
  `paigasus-iam/src/application/api_keys.rs` and `paigasus-iam/src/application/service_accounts.rs`
  (one `capture_logs()` call in each, per the PR diff on 2026-09-28). If PR 348 merges before this
  branch, rebase onto `origin/main` and migrate those call sites the same way as C3. AC 1 already
  covers them: after the rebase, `src/log_capture.rs` is still deleted and the `git grep` of AC 1
  still returns only `test_support.rs`. Re-count the call sites of D4 after the rebase. If PR 348
  merges after this branch, SMA-646 must migrate its own call sites, because `crate::log_capture`
  no longer exists.

### 4.4 The service `Cargo.toml` files

In both `paigasus-gateway/Cargo.toml` and `paigasus-iam/Cargo.toml`:

- Add under `[dev-dependencies]`:
  `paigasus-logging = { path = "../../libs/paigasus-logging", version = "0.0.0", features = ["test-support"] }`,
  with a comment that names SMA-689 and D3. The comment also says that `version = "0.0.0"` is
  required: `rs/deny.toml:51` sets `wildcards = "deny"`, and a path dependency with no `version`
  counts as a wildcard. Nobody may remove it as redundant.
- Remove the `tracing-subscriber = { workspace = true }` dev-dependency and its comment
  (gateway lines 105-107, IAM lines 176-177). After section 4.3 no test in either crate names
  `tracing_subscriber` in code (the hits in `tests/boot_lifecycle_pg.rs:46,193` are comments).

The control for this removal is the `git grep`, not `repo:machete`. Before removal, the
implementer re-runs `git grep -n "tracing_subscriber" -- rs/crates/services/<crate>` and confirms
that only comments remain. For the gateway, a kept unused dev-dependency most probably reds
`repo:machete`. For IAM it most probably does not: cargo-machete matches `tracing_subscriber::`
in raw text, comments included, and `boot_lifecycle_pg.rs:46` has such a comment. So a green
machete does not prove the IAM removal is correct.

**`rs/Cargo.lock` changes.** The lock records dev-dependencies. Today `"tracing-subscriber",`
is in the `dependencies` list of `paigasus-gateway` (line 3187) and of `paigasus-iam`
(line 3240). The removal above deletes those two lines. The new `paigasus-logging` dev entry adds
no line, because `paigasus-logging` is already listed as a normal dependency, and features are not
recorded in the lock. The expected diff is exactly two removed `"tracing-subscriber",` lines and
nothing else. The implementer must commit this lock change. Without it, every `--locked` Moon
task and the `ci/cargo-lock-integrity` step in `ci.yml` go red.

### 4.5 What does not change

- `rs/Cargo.toml` `members`, both `moon.yml` files, and `.github/CODEOWNERS`.
  (`rs/Cargo.lock` DOES change; see section 4.4.)
- The Moon project graph, most probably. No in-tree crate is listed in both `[dependencies]` and
  `[dev-dependencies]` today, and Moon resolves `path` dependencies on its own
  (`source=implicit`). Nobody has measured the effect on the edge scope of `paigasus-logging-rs`.
  The implementer compares the unpiped `moon query projects` output for both services before and
  after the change, and records the result in the PR.
- The 40 `let (logs, _guard) = …` lines, except for the call path.

## 5. Tests

### 5.1 New unit tests in `paigasus-logging/src/test_support.rs`

1. `capture_logs_sees_trace`: `capture_logs()`, then `tracing::trace!("probe-trace")`; `text()`
   contains `probe-trace`.
2. `capture_logs_at_info_drops_debug`: `capture_logs_at(Level::INFO)`, then `debug!("d-probe")`
   and `info!("i-probe")`; `text()` contains `i-probe` and does not contain `d-probe`.
3. `capture_has_no_ansi_escape`: remove `NO_COLOR` from the environment first, then capture and
   log one `info!`; `text()` contains no `\u{1b}` byte. `tracing-subscriber` 0.3.23 (the lock)
   does not check for a terminal writer. The ANSI default comes from the `ansi` feature and the
   `NO_COLOR` env var. Without the removal, the test result depends on the runner's environment.
   Edition 2024 makes `std::env::remove_var` `unsafe`; the test uses the same static-lock pattern
   as the `RUST_LOG` tests in `lib.rs` (`ENV_LOCK`).
4. `guard_drop_stops_capture`: capture, log `before`, drop the guard, log `after`; `text()`
   contains `before` and not `after`.

These tests pin the behavior every caller relies on: level, no ANSI, scope of the guard.

The `paigasus-logging` `lib.rs` tests deliberately support thread-parallel `cargo test`
(`ENV_LOCK`). The four new tests install scoped subscribers at different levels, which is the
flake of section 6 under `cargo test`. So all four tests take one static `Mutex` for their whole
body. The env-mutating test 3 takes the same lock. Then the crate's tests stay correct under both
runners.

### 5.2 The existing tests

All callers in section 1 run unchanged, apart from the import path. They are the behavior proof
for AC 2 and AC 4. Run
`PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-logging -p paigasus-gateway -p paigasus-iam`.
Use `--locked`: an unlocked run silently rewrites `rs/Cargo.lock` first and hides a missing lock
commit. If `PAIGASUS_REQUIRE_DOCKER` is not honoured by a suite, record instead that
`docker_preflight` passed. A silent skip of a C4 caller does not count as a pass.

Before this run, check the lock: `git diff rs/Cargo.lock` shows exactly the two removed
`"tracing-subscriber",` lines (section 4.4), and `cargo metadata --locked --format-version 1 >
/dev/null` exits 0.

### 5.3 The helper does not reach the production build (AC 6)

`cargo tree -e features` alone adds `normal,build,dev` edges, and with dev edges on, cargo
resolves features with dev units active, so `test-support` appears. That command cannot prove
AC 6. Use these instead, for both `paigasus-gateway` and `paigasus-iam`:

1. `cargo tree --locked -p <crate> -e features,no-dev -i paigasus-logging`: `test-support` must
   NOT appear.
2. `cargo tree --locked -p <crate> -e features,dev -i paigasus-logging`: `test-support` MUST
   appear, so the dev edge is real.
3. The stronger proof, the real compiler call:
   `cargo build --locked -p <crate> --bin <crate> -v`. The rustc line with
   `--crate-name paigasus_logging` must have no `feature="test-support"`. `rs/Dockerfile:36`
   runs `cargo auditable build --release --locked -p "${BIN}" --bin "${BIN}"`. The package and
   target selection is the same, so feature resolution is the same as for the container image;
   only the profile differs, and the profile does not change features.
   (A `-v` build of an already-built crate prints `Fresh`; run `cargo clean -p paigasus-logging`
   first so the rustc line prints.)

Record all outputs in the PR.

### 5.4 Proof that the tests bite

- Change `capture_logs()` to `Level::INFO` in `test_support.rs`: test 5.1.1 must go red. The
  gateway `auth.rs` row 9 test (`auth.rs:1321`) stays green: it asserts on a `WARN` line and on
  two "never logged" checks, and a "never logged" check passes at any level. `validator.rs` and
  `authenticate_token.rs` have no `debug!` or `trace!` events. So test 5.1.1 is the ONLY guard of
  the TRACE default. If the default regresses, every negative caller assertion passes without
  testing anything. Record this in the PR. Restore by reverting the edit, not with
  `git checkout --` over an uncommitted change.
- Change `capture_logs_at` to ignore `level` (always TRACE): test 5.1.2 must go red.
- Remove `with_ansi(false)`: test 5.1.3 must go red, because the test removes `NO_COLOR` and the
  `ansi` feature is on (section 5.1). If it stays green, record the result and do not claim that
  5.1.3 guards the setting.

Each mutation must compile (`-D warnings` applies), or it proves nothing. Run with
`--no-fail-fast` and re-run the whole battery after any later fix.

### 5.5 Gates

`moon ci` with the full target list from the root `CLAUDE.md`, `--base origin/main
--include-relations`. Because `rs/Cargo.lock` changes (section 4.4), the selection is much larger
than the edited crates. Expected selection (confirm with `moon query tasks --affected`, parsed
per the root `CLAUDE.md`, one target per `tasks[project][task]`):

- every Rust crate's `lint` (`/rs/Cargo.lock` is a `lint` input), plus `:build`, `:test`, `:fmt`
  for the edited crates;
- `paigasus-kernel-ts:{build,test}` and `paigasus-kernel-py:test`;
- `repo:deny`, `repo:machete`, `repo:affected-smoke`, `repo:publish-metadata`,
  `repo:version-lockstep`, `repo:wasm-getrandom-free`, `repo:nats-permissions`,
  `repo:pyo3-stub-drift`, and `repo:actionlint` if selected.

Bash needs on the development Mac (root `CLAUDE.md`, "This development Mac only"):

| Gate | Bash |
|---|---|
| `repo:affected-smoke` | system `/bin/bash` 3.2 |
| `repo:publish-metadata`, `repo:version-lockstep`, `repo:nats-permissions` | bash 4+ (`/opt/homebrew/bin/bash`) |
| `repo:actionlint` | bash 5 and a pipe preflight that passes (65536 bytes, floor 8192) |

Pick one bash for the `moon ci` run, then re-run the gates that need the other bash directly
(`<bash> ci/<gate>/run.sh`) and read those results.

The lock change also starts `.github/workflows/images.yml` on the PR: its `pull_request` path
filter lists `rs/Cargo.lock`. Expect that workflow to run.

## 6. Known limits

- **Supported runner: `cargo nextest`.** `set_default` installs a thread-local dispatcher, and
  `tracing` caches callsite interest process-wide. Under `cargo test` (one process, many test
  threads) another thread can register a callsite while no capture subscriber is active on it, so
  the cached interest can make a capture miss a line. nextest runs one test per process, so the
  cache holds only this test's subscriber. The repo's Moon `test` task and CI use nextest
  (`.moon/tasks/rust.yml:54`). The helper does not add a global lock or a `rebuild_interest_cache`
  call to make `cargo test` safe; that is out of scope.
- **Current-thread only.** A test on a `multi_thread` Tokio runtime does not see events from other
  worker threads. No current caller uses one.
- **No gate prevents a fifth copy.** A new test can still define its own `LogBuffer`. The module
  doc and `rs/CLAUDE.md` (section 7) are the control. A `git grep` gate was considered and not
  added: the issue does not ask for one, and the duplication is low-risk.

## 7. Documentation changes

- `rs/CLAUDE.md`: add one short bullet: "Tests that assert on a log line use
  `paigasus_logging::test_support` (the `test-support` feature, turned on from
  `[dev-dependencies]`). Do not write a new `LogBuffer`. The helper supports `cargo nextest` only
  (SMA-689)."
- Remove the comments in C1, C2 and C4 that describe the duplication. They become false.

## 8. Files expected to change

| File | Change |
|---|---|
| `rs/crates/libs/paigasus-logging/Cargo.toml` | add `[features] test-support = []` |
| `rs/crates/libs/paigasus-logging/src/lib.rs` | declare `test_support` module; crate doc line |
| `rs/crates/libs/paigasus-logging/src/test_support.rs` | new: helper plus four unit tests |
| `rs/crates/services/paigasus-gateway/Cargo.toml` | dev-dep on `paigasus-logging` with the feature; drop `tracing-subscriber` dev-dep |
| `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs` | delete C1, import shared helper |
| `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs` | delete C2, call `capture_logs_at(INFO)` |
| `rs/crates/services/paigasus-iam/Cargo.toml` | dev-dep on `paigasus-logging` with the feature; drop `tracing-subscriber` dev-dep |
| `rs/crates/services/paigasus-iam/src/lib.rs` | remove `mod log_capture` |
| `rs/crates/services/paigasus-iam/src/log_capture.rs` | delete |
| `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` | import path |
| `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` | import path |
| `rs/crates/services/paigasus-iam/tests/support/mod.rs` | delete C4 |
| `rs/crates/services/paigasus-iam/tests/{authn_identities,grpc_whoami,http_authn}.rs` | call path (7 calls) |
| `rs/Cargo.lock` | exactly two removed `"tracing-subscriber",` lines (gateway, IAM dev-deps) |
| `rs/CLAUDE.md` | one bullet (section 7) |
| `rs/crates/services/paigasus-iam/src/application/{api_keys,service_accounts}.rs` | import path, ONLY if PR 348 (SMA-646) merges first (section 4.3) |

Branch: `feature/sma-689-shared-log-capture-helper`. Commit type and scope: `test(rs)`.

## 9. Out of scope

- Making log assertions safe under `cargo test`.
- A gate that reds on a new `LogBuffer` copy.
- JSON-format capture, or capture of the production `paigasus_logging::init` subscriber (the
  `boot_lifecycle_pg.rs` stdout-drain test keeps its own approach; it reads the child process's
  stdout and is not a copy of this helper).
- Any other test utility that could move to a shared crate.

## 10. Open questions

All open questions are answered (Sven, 2026-09-28). The answers are the approval decisions of
section 12.

1. **Resolved: C4 is in scope.** The issue AC 1 says "No crate defines its own `LogBuffer` or
   `capture_logs`" (checked against the Linear text on 2026-09-27). SMA-707 added C4 after the
   issue was written, so this spec includes it.
2. **Crate choice.** This spec picks the `paigasus-logging` feature (D1) over a new dev-only
   crate. The issue lists both. Confirm, or state if a separate `paigasus-test-support` crate is
   wanted for future test utilities.
   **Answer (Sven, 2026-09-28): the `test-support` feature in `paigasus-logging`. No new crate.
   The services turn it on from `[dev-dependencies]` only (D1, D3).**
3. **Default level.** This spec makes `capture_logs()` capture at `TRACE` and moves `chat_proxy.rs`
   to an explicit `capture_logs_at(INFO)`. An alternative is one function at TRACE for every
   caller, which changes what `chat_proxy.rs` captures (its assertions use `find` on a known line,
   so they would most probably still pass). This spec keeps INFO because the issue says "keep the
   current behavior of each test".
   **Answer (Sven, 2026-09-28): as proposed. `capture_logs()` captures at `TRACE`; `chat_proxy.rs`
   calls `capture_logs_at(Level::INFO)` explicitly.**
4. **Guard gate.** Should a `repo:*` gate red on a new `struct LogBuffer` outside
   `paigasus-logging`? This spec says no (section 6).
   **Answer (Sven, 2026-09-28): no gate.**
5. **Feature convention.** This is the first `[features]` table in a workspace crate. Is the
   `rs/CLAUDE.md` bullet (section 7) enough, or does the "test-support feature" convention need an
   entry in the Notion Development Guidelines?
   **Answer (Sven, 2026-09-28): the `rs/CLAUDE.md` bullet is enough. No Notion entry.**
6. **`LogBuffer` visibility.** This spec keeps `LogBuffer` public, because it is part of the
   return type. Is it a supported extension point for a caller that builds its own subscriber
   (`Default` + `MakeWriter`), or an implementation detail that should carry `#[doc(hidden)]`?
   **Answer (Sven, 2026-09-28): an implementation detail. `LogBuffer` carries `#[doc(hidden)]`
   (D4, section 4.2).**
7. **Tuple or guard struct.** This spec keeps the `(LogBuffer, DefaultGuard)` tuple to limit
   churn (D4). A struct that owns the guard removes the `let (logs, _) = …` risk fully. Confirm
   the tuple.
   **Answer (Sven, 2026-09-28): keep the `(LogBuffer, DefaultGuard)` tuple.**

## 11. Challenge changelog

Spec-challenger verdict: **APPROVE WITH CHANGES** (2026-09-27).

Folded:

- BLOCKER, `rs/Cargo.lock` changes: verified at lock lines 3187 and 3240. Moved the lock from
  "What does not change" to section 4.4 and section 8, with the exact expected diff (two removed
  lines). Section 5.2 now checks `git diff rs/Cargo.lock` and `cargo metadata --locked`, and runs
  nextest with `--locked`. Section 5.5 lists the real, larger selection and `images.yml`.
- MAJOR, AC 6 proof includes dev edges: section 5.3 now uses `-e features,no-dev` and
  `-e features,dev`, plus the `cargo build -v` rustc line as the strong proof (`rs/Dockerfile:36`).
- MINOR, machete claim for IAM: the `git grep` is now the control; section 4.4 states that machete
  most probably stays green for IAM because of the comment at `boot_lifecycle_pg.rs:46`.
- MINOR, ANSI rationale: verified that `tracing-subscriber` 0.3.23 reads `NO_COLOR`. Test 5.1.3
  removes `NO_COLOR` under a static lock; the mutation must now red.
- MINOR, helper tests under `cargo test`: all four tests take one static `Mutex`.
- MINOR, only 5.1.1 guards the TRACE default: section 5.4 now says that row 9 stays green.
- MINOR, silent Docker skips: AC 2 and section 5.2 use `PAIGASUS_REQUIRE_DOCKER=1` (exists,
  `rs/CLAUDE.md:29`), or a recorded `docker_preflight` pass.
- MINOR, local gate plan: section 5.5 lists each selected gate with the bash it needs.
- MINOR, `version` field: the dev-dependency comment names `rs/deny.toml:51` `wildcards = "deny"`.
- MINOR, new Moon edge: section 4.5 asks for a before/after `moon query projects` comparison.
- MINOR, C4 edit precision: keep both constants (`JIT_DISABLED_LINE` too); `Arc` stays in
  `tests/support/mod.rs` (`:171`, `:267`) and `chat_proxy.rs` (`:206-207`), verified.
- MINOR, format misread: D1 and the module doc say captured text is not the production JSON
  format.
- MINOR, `tracing-test` alternative: D1 rejects it with reasons.
- MINOR, guard dropped early: `#[must_use]` on both functions; the tuple stays for less churn,
  stated in D4 and open question 7.
- MINOR, precision: AC 1 uses `git grep -nE`; the commit type is `test(rs)`.
- QUESTION, exact AC text: answered from Linear; open question 1 is resolved.
- QUESTIONS, Notion convention entry and `LogBuffer` visibility: added as open questions 5 and 6.

Rejected: none. Each finding matched the repo code, or was hedged ("most probably") in a way that
the spec now keeps.

## 12. Approval decisions (Sven, 2026-09-28)

These decisions override the defaults above where they differ. None of them widens the scope of
the issue.

- **A1.** A `test-support` feature in `paigasus-logging`, not a new crate. The services turn it
  on from `[dev-dependencies]` only (D1, D3; open question 2).
- **A2.** `capture_logs()` captures at `TRACE`. `chat_proxy.rs` moves to an explicit
  `capture_logs_at(Level::INFO)` (D4; open question 3).
- **A3.** No `repo:*` gate against a new `LogBuffer` copy outside `paigasus-logging` (section 6,
  section 9; open question 4).
- **A4.** The `rs/CLAUDE.md` bullet for the first `[features]` table and the `test-support`
  convention is enough. No Notion entry (section 7; open question 5).
- **A5.** `LogBuffer` carries `#[doc(hidden)]` (D4, section 4.2; open question 6). This is the
  only change to the design text.
- **A6.** Keep the `(LogBuffer, DefaultGuard)` tuple API (D4; open question 7).
- **A7.** SMA-646 (PR 348, open) adds uses of `crate::log_capture::capture_logs` in
  `paigasus-iam`. If it merges first, rebase and migrate its call sites too (section 4.3,
  section 8). This does not widen the scope: AC 1 already requires that no crate keeps its own
  copy.

## 13. Re-check against `origin/main` 213bc99d (2026-09-28)

The spec was written against 83d446fc. The branch starts at 213bc99d. Nine commits are in
between. Three of them touch files that this spec names or reads:

- `1a45803f` (chore: release) changes `rs/Cargo.lock`: only `version` lines of released crates.
  The `"tracing-subscriber",` lines of `paigasus-gateway` (3187) and `paigasus-iam` (3240) did not
  move.
- `5f6558f5` (SMA-713) adds a test to `paigasus-iam/tests/boot_lifecycle_pg.rs`. This moved the
  second `tracing_subscriber` comment from line 147 to line 193.
- `906de46c` (SMA-649) adds tests to other `paigasus-iam/tests/*.rs` files. None of them calls
  `capture_logs`: the seven C4 call sites are still the only ones.

No commit touches `paigasus-logging`, the four copies, or their callers. Every path and line
number was re-checked. Changes to the text:

- Section 1: the base commit now names 213bc99d too. The four copies, their line ranges and all
  40 call sites are unchanged.
- Section 4.3: added the conditional SMA-646 call sites (A7). Section 8: one conditional row.
- Section 4.4: the second `tracing_subscriber` comment in `tests/boot_lifecycle_pg.rs` is at line
  193, not 147.
- Section 5.3: `rs/Dockerfile:36` runs `cargo auditable build --release --locked`, not a plain
  `cargo build`. The text now states that the profile does not change feature resolution.
- D4 and section 4.2: `#[doc(hidden)]` on `LogBuffer` (A5).

Verified unchanged: `auth.rs:792-828`, `:536`, `:545`, `:1321-1322`; `chat_proxy.rs:24`,
`:45-84`, `:206-207`, `:500`, `:519`; `paigasus-iam/src/lib.rs:13-14`; `validator.rs:367`;
`authenticate_token.rs:378`; `tests/support/mod.rs:52`, `:171`, `:267`, `:956-998`;
`grpc_whoami.rs:164`; `paigasus-gateway/Cargo.toml:19`, `:85-89`, `:105-107`;
`paigasus-iam/Cargo.toml:20`, `:176-177`; `rs/Cargo.lock:3187` (gateway) and `:3240` (IAM);
`tracing-subscriber` 0.3.23; `rs/Cargo.toml` `resolver = "3"`; `rs/deny.toml:51`;
`.moon/tasks/rust.yml:54` (nextest) and `:79` (clippy `--all-targets`); `rs/CLAUDE.md:29`
(`PAIGASUS_REQUIRE_DOCKER`); the `ENV_LOCK` in `paigasus-logging/src/lib.rs`; no workspace crate
has a `[features]` table; both service `moon.yml` files list `paigasus-logging-rs`.
