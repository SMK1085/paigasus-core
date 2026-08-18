# SMA-538 — one Docker-skip policy for `paigasus-iam` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace 11 hand-rolled copies of the Docker-skip decision in `paigasus-iam`'s tests with one policy in `tests/support/docker.rs`, make a container failure on a reachable daemon a hard failure, and add a canary test so a Docker-less run can never report a silent green.

**Architecture:** All policy lives in the existing standalone `tests/support/docker.rs`. Skip-versus-fail is decided by matching testcontainers' error *types* — never its message text — splitting transport failures (daemon never answered) from daemon-answered ones. A new one-test binary `tests/docker_preflight.rs` hard-fails when the daemon is unreachable, so the 56 quiet skips are always accompanied by one loud red.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), `testcontainers` 0.27.3 + `testcontainers-modules`, `cargo nextest` 0.9.136, Moon 2.3.2.

**Spec:** `docs/superpowers/specs/2026-08-17-sma-538-iam-docker-skip-policy-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- `rs/Cargo.toml` sets `[workspace.lints.rust] warnings = "deny"`, so **`dead_code` is a hard compile error**. Every item added to `tests/support/docker.rs` MUST carry `#[allow(dead_code)]` — that file is compiled into 57+ separate test binaries and most use only part of it.
- `tests/support/docker.rs` must not depend on `tests/support/mod.rs`. Five test files reach it via `#[path = "support/docker.rs"] mod docker;` and have no `mod support;`.
- Items used from a `#[path]`-included module must be `pub` — the including crate root is the module's *parent*, and Rust privacy is "defining module and descendants".
- Bash tool PATH lacks proto CLIs. Prefix commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- All work happens in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-538` on branch `feature/sma-538-iam-consolidate-docker-skip-policy`. Do not `cd` to the main checkout.
- Commit messages: conventional commits with workspace scope (`fix(rs):`, `docs(repo):`). Subject starts lowercase, ≤100 chars. **Body lines ≤100 chars.** No `#NNN` issue refs in the body (commitlint reads them as a footer). Do not use `--no-verify`.
- Never name a file with a Windows reserved base name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`).

---

### Task 1: The policy primitives — env parsing and error classification

Adds the decision logic and its Docker-free unit tests. Nothing calls it yet; that is deliberate, so this task is reviewable on its own.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/support/docker.rs` (append; also fix the module doc at `:5`)
- Create: `rs/crates/services/paigasus-iam/tests/support_docker_policy.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub fn env_flag(raw: Option<&OsStr>) -> bool`
  - `pub fn skip_docker() -> bool`
  - `pub fn require_docker() -> bool`
  - `pub fn is_daemon_unreachable(e: &TestcontainersError) -> bool`

- [ ] **Step 1: Write the failing tests**

Create `rs/crates/services/paigasus-iam/tests/support_docker_policy.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Docker-free unit tests for `support::docker`'s skip policy (SMA-538).
//!
//! Lives in its own test binary, included via `#[path]`, for the same reasons
//! `support_docker_retry.rs` does: a `#[cfg(test)]` module inside `docker.rs` would be
//! silently compiled out (`cfg(test)` is not set when rustc builds an integration-test
//! binary), and bare `#[tokio::test]` functions inside `docker.rs` would run once per
//! including binary, duplicating these assertions ~57 times.

#[path = "support/docker.rs"]
mod docker;

use docker::{env_flag, is_daemon_unreachable};
use std::ffi::OsStr;
use std::io::{Error as IoError, ErrorKind};
use testcontainers::bollard::errors::Error as BollardError;
use testcontainers::core::error::{ClientError, TestcontainersError};

// ---------------------------------------------------------------- env_flag

#[test]
fn env_flag_accepts_the_three_documented_truthy_spellings() {
    for on in ["1", "true", "yes", "TRUE", "Yes", "  true  "] {
        assert!(env_flag(Some(OsStr::new(on))), "{on:?} must parse as on");
    }
}

#[test]
fn env_flag_rejects_everything_else_including_zero_and_unset() {
    assert!(!env_flag(None), "unset must be off");
    for off in ["0", "", "no", "false", "maybe", "2", "on"] {
        assert!(!env_flag(Some(OsStr::new(off))), "{off:?} must parse as off");
    }
}

// ------------------------------------------------- is_daemon_unreachable

/// F1 row 1: the socket file is absent. Observed as
/// `failed to initialize a docker client: Socket not found: /nonexistent/docker.sock`.
#[test]
fn missing_socket_is_unreachable() {
    let e = TestcontainersError::Client(ClientError::Init(BollardError::SocketNotFoundError("/nonexistent/docker.sock".to_string())));
    assert!(is_daemon_unreachable(&e));
}

#[test]
fn connection_refused_on_the_transport_is_unreachable() {
    let e = TestcontainersError::Client(ClientError::CreateContainer(BollardError::IOError {
        err: IoError::new(ErrorKind::ConnectionRefused, "connection refused"),
    }));
    assert!(is_daemon_unreachable(&e));
}

/// THE REGRESSION TEST FOR THIS ISSUE'S OWN FIRST DRAFT (spec F3).
///
/// A healthy daemon that cannot reach the registry relays the registry's text verbatim
/// through `DockerResponseServerError`. Any classifier that substring-matched
/// "connection refused" would skip here — with Docker running — silently disabling every
/// Postgres/NATS/Keycloak suite. It must be a hard failure.
#[test]
fn registry_unreachable_through_a_healthy_daemon_is_not_unreachable() {
    let e = TestcontainersError::Client(ClientError::PullImage {
        descriptor: "redis:latest".to_string(),
        err: BollardError::DockerResponseServerError {
            status_code: 500,
            message: r#"Get "https://registry-1.docker.io/v2/": dial tcp 1.2.3.4:443: connect: connection refused"#.to_string(),
        },
    });
    assert!(!is_daemon_unreachable(&e), "a daemon that ANSWERED must never be classified as unreachable");
}

/// `client.rs:259` maps a genuine container-START failure to `ClientError::Init`. The
/// classifier must not be fooled by the variant name — the daemon answered, so this is hard.
#[test]
fn mis_tagged_container_start_failure_is_not_unreachable() {
    let e = TestcontainersError::Client(ClientError::Init(BollardError::DockerResponseServerError {
        status_code: 409,
        message: "container already started".to_string(),
    }));
    assert!(!is_daemon_unreachable(&e));
}

/// A socket we are not allowed to open is a misconfiguration worth seeing, not a skip.
#[test]
fn permission_denied_on_the_socket_is_not_unreachable() {
    let e = TestcontainersError::Client(ClientError::CreateContainer(BollardError::IOError {
        err: IoError::new(ErrorKind::PermissionDenied, "permission denied"),
    }));
    assert!(!is_daemon_unreachable(&e));
}

/// `WaitContainer` is the variant that carries container LOG output, which can contain
/// anything the server logged — including the words a naive classifier looks for. It is not a
/// `Client(_)` error at all, so it can never reach the transport check.
#[test]
fn wait_container_errors_are_never_unreachable() {
    let e = TestcontainersError::WaitContainer(testcontainers::core::error::WaitContainerError::StartupTimeout);
    assert!(!is_daemon_unreachable(&e));
}

/// A fixture file that `with_copy_to` cannot read is a real failure of the test's own setup.
#[test]
fn copy_to_container_failure_is_not_unreachable() {
    let e = TestcontainersError::Client(ClientError::CopyToContainerError(testcontainers::core::CopyToContainerError::IoError(IoError::new(
        ErrorKind::NotFound,
        "no such file or directory",
    ))));
    assert!(!is_daemon_unreachable(&e));
}
```

> Every type path above was verified against testcontainers 0.27.3:
> `WaitContainerError::StartupTimeout` at `src/core/error.rs:66`, and
> `CopyToContainerError::IoError(std::io::Error)` at `src/core/copy.rs:144` (re-exported as
> `testcontainers::core::CopyToContainerError`). They compile as written.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test support_docker_policy
```

Expected: compile FAILS — `env_flag` and `is_daemon_unreachable` are not defined in `docker`.

- [ ] **Step 3: Implement the policy in `docker.rs`**

Append to `rs/crates/services/paigasus-iam/tests/support/docker.rs`, and add these imports at the top alongside the existing `use std::time::Duration;`:

```rust
use std::ffi::OsStr;
use std::io::ErrorKind;
use testcontainers::bollard::errors::Error as BollardError;
use testcontainers::core::error::{ClientError, TestcontainersError};
```

```rust
/// Parses a human-typed on/off environment variable: `1`, `true` or `yes`, case-insensitively
/// and ignoring surrounding whitespace. Everything else — including `0`, the empty string and
/// unset — is off.
///
/// Deliberately NOT the presence-based form the adjacent `CI` check uses. `CI` is set by a
/// platform and any value it carries means "in CI"; these two are typed by a human, for whom
/// `PAIGASUS_REQUIRE_DOCKER=0` silently meaning "on" would be a footgun.
///
/// Takes the raw value rather than reading the environment itself so it can be unit-tested
/// without `unsafe { std::env::set_var(..) }` (unsafe under edition 2024) and without assuming
/// anything about process isolation between tests.
#[allow(dead_code)]
pub fn env_flag(raw: Option<&OsStr>) -> bool {
    let Some(v) = raw.and_then(OsStr::to_str) else {
        return false;
    };
    matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")
}

/// Whether the caller has explicitly accepted skipping the Docker-backed suites.
///
/// `CI` outranks it: a stray `PAIGASUS_SKIP_DOCKER` in a workflow file must not be able to
/// green a CI run that tested nothing.
#[allow(dead_code)]
pub fn skip_docker() -> bool {
    std::env::var_os("CI").is_none() && env_flag(std::env::var_os("PAIGASUS_SKIP_DOCKER").as_deref())
}

/// Whether a missing daemon must be a hard failure rather than a skip. `CI` implies it.
#[allow(dead_code)]
pub fn require_docker() -> bool {
    std::env::var_os("CI").is_some() || env_flag(std::env::var_os("PAIGASUS_REQUIRE_DOCKER").as_deref())
}

/// Whether a failed `start()` means the Docker daemon could not be reached at all, as opposed
/// to a container that genuinely failed with a healthy daemon.
///
/// **Classifies by TYPE, never by message text.** An earlier draft of SMA-538 substring-matched
/// the rendered error for markers like `connection refused`, which fails OPEN: bollard's
/// `DockerResponseServerError` interpolates daemon-authored free text into its `Display`, and
/// `async_runner.rs:343-358` pulls an uncached image whenever `create_container` returns 404,
/// so a healthy daemon that cannot reach the registry relays the registry's own
/// `connect: connection refused` and every suite would have skipped with Docker running.
///
/// It also removes any need to know that `client.rs:259` mis-maps a container-START failure to
/// `ClientError::Init`: that error carries a daemon RESPONSE, so it lands on the `false` side
/// structurally.
#[allow(dead_code)]
pub fn is_daemon_unreachable(e: &TestcontainersError) -> bool {
    let TestcontainersError::Client(client) = e else {
        // WaitContainer (which carries container LOG output), PortNotExposed, Exec, MissingInfo,
        // Io and Other are all failures of a daemon that answered us.
        return false;
    };

    // EXHAUSTIVE on purpose — no `_` arm. `ClientError` is not `#[non_exhaustive]`, so a
    // testcontainers upgrade that adds a variant becomes a COMPILE ERROR here rather than a
    // silent reclassification. If rustc reports a missing variant, decide which side it belongs
    // on: does it carry a raw transport error, or did the daemon answer?
    let bollard: &BollardError = match client {
        // The daemon never answered — these wrap a raw transport error.
        ClientError::Init(b)
        | ClientError::ListContainers(b)
        | ClientError::CreateContainer(b)
        | ClientError::RemoveContainer(b)
        | ClientError::StartContainer(b)
        | ClientError::StopContainer(b)
        | ClientError::PauseContainer(b)
        | ClientError::UnpauseContainer(b)
        | ClientError::InspectContainer(b)
        | ClientError::CreateNetwork(b)
        | ClientError::InspectNetwork(b)
        | ClientError::ListNetworks(b)
        | ClientError::RemoveNetwork(b)
        | ClientError::InitExec(b)
        | ClientError::InspectExec(b)
        | ClientError::UploadToContainerError(b) => b,

        // The daemon ANSWERED, or we never reached it for a reason of our own making. Never a
        // skip — `PullImage` in particular is where the fail-open lived.
        ClientError::PullImage { .. }
        | ClientError::BuildImage { .. }
        | ClientError::Configuration(_)
        | ClientError::InvalidDockerHost(_)
        | ClientError::PortMapping(_)
        | ClientError::CopyToContainerError(_)
        | ClientError::CopyFromContainerError(_) => return false,
    };

    match bollard {
        BollardError::SocketNotFoundError(_) => true,
        // `is_connect()` separates a genuine connect failure from a post-connect protocol error.
        BollardError::HyperLegacyError { err } => err.is_connect(),
        BollardError::IOError { err } => matches!(
            err.kind(),
            ErrorKind::NotFound | ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
        ),
        BollardError::RequestTimeoutError => true,
        // NOT exhaustive here, unlike the match above: several bollard variants are
        // `#[cfg(feature = ...)]`-gated (ssl_providerless, websocket, http, ssh, pipe), so an
        // exhaustive match would stop compiling whenever a feature toggles anywhere in the
        // workspace. `false` fails CLOSED — an unrecognised bollard error reds, never skips.
        // Notably this is where `DockerResponseServerError` lands.
        _ => false,
    }
}
```

Also update the module doc at `docker.rs:5`, which currently claims the file "Deliberately depends on NOTHING else in `support/`" — that is still true, but Task 2 will add a `testcontainers-modules` import, so restate it now as:

```rust
//! Deliberately depends on nothing else in `support/`, so the test files that have no
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test support_docker_policy
```

Expected: PASS, 9 tests.

- [ ] **Step 5: Verify the whole crate still compiles under `warnings = "deny"`**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: clean. A `dead_code` error here means an `#[allow(dead_code)]` was missed.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/support/docker.rs rs/crates/services/paigasus-iam/tests/support_docker_policy.rs
git commit -m "fix(rs): classify iam's docker failures by error type, not message text (SMA-538)"
```

---

### Task 2: `start_or_skip` / `start_redis_or_skip`, and the six Redis call sites

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/support/docker.rs` (append)
- Modify: `rs/crates/services/paigasus-iam/tests/redis_jwks_cache.rs:28-43`
- Modify: `rs/crates/services/paigasus-iam/tests/api_key_cache_redis.rs:~30-45`
- Modify: `rs/crates/services/paigasus-iam/tests/authz_cache_redis.rs:~34-49`
- Modify: `rs/crates/services/paigasus-iam/tests/authz_generations_redis.rs:~35-50`
- Modify: `rs/crates/services/paigasus-iam/tests/authz_acceptance.rs:74-90`
- Modify: `rs/crates/services/paigasus-iam/tests/api_key_cache_connection.rs:33-51`

**Interfaces:**
- Consumes: `skip_docker`, `require_docker`, `is_daemon_unreachable` from Task 1.
- Produces:
  - `pub async fn start_or_skip<T, I>(image: T, what: &str) -> Option<ContainerAsync<I>> where T: Into<ContainerRequest<I>> + Send, I: Image`
  - `pub async fn start_redis_or_skip(what: &str) -> Option<(ContainerAsync<Redis>, String)>`

- [ ] **Step 1: Add the starters to `docker.rs`**

Add these imports:

```rust
use testcontainers::core::ContainerRequest;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::redis::Redis;
```

```rust
/// The SINGLE definition of what happens when a container will not start (SMA-538).
///
/// Ordered rules, first match wins:
///   1. `Ok`                          -> `Some(node)`
///   2. `CI` present                  -> panic. Docker is mandatory in CI, and `CI` outranks
///                                       `PAIGASUS_SKIP_DOCKER` so no workflow-file env var can
///                                       green a run that tested nothing.
///   3. `PAIGASUS_SKIP_DOCKER` on     -> skip. The escape hatch for a Docker Hub rate limit or a
///                                       daemon restart; it outranks REQUIRE because it is the
///                                       recourse of last resort.
///   4. `PAIGASUS_REQUIRE_DOCKER` on  -> panic.
///   5. daemon unreachable            -> skip.
///   6. otherwise                     -> panic. A container that failed with a REACHABLE daemon
///                                       is a real failure, not a reason to skip (AC 2).
///
/// A skip emits `SKIP[docker-unavailable] {what}: {e}` on stderr. That line is discarded by
/// nextest on the passing path (`success-output` defaults to `never`) and again by Moon
/// (`buffer-only-failure`), which is exactly why `tests/docker_preflight.rs` exists: it turns a
/// Docker-less run into one loud red instead of 56 quiet passes.
#[allow(dead_code)]
pub async fn start_or_skip<T, I>(image: T, what: &str) -> Option<ContainerAsync<I>>
where
    T: Into<ContainerRequest<I>> + Send,
    I: Image,
{
    match image.start().await {
        Ok(node) => Some(node),
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("{what}: Docker is required in CI: {e}");
            }
            if skip_docker() {
                eprintln!("SKIP[docker-unavailable] {what}: {e}");
                return None;
            }
            if require_docker() {
                panic!("{what}: PAIGASUS_REQUIRE_DOCKER is set and Docker is unusable: {e}");
            }
            if is_daemon_unreachable(&e) {
                eprintln!("SKIP[docker-unavailable] {what}: {e}");
                return None;
            }
            panic!("{what}: the Docker daemon is reachable but the container failed to start, which is a real failure, not a reason to skip. Set PAIGASUS_SKIP_DOCKER=1 to skip anyway: {e}");
        }
    }
}

/// An ephemeral Redis plus its connection URL — the shape six suites each hand-rolled.
#[allow(dead_code)]
pub async fn start_redis_or_skip(what: &str) -> Option<(ContainerAsync<Redis>, String)> {
    let node = start_or_skip(Redis::default(), what).await?;
    let port = mapped_port(&node, 6379, "redis").await;
    Some((node, format!("redis://127.0.0.1:{port}")))
}
```

- [ ] **Step 2: Verify it compiles before touching any call site**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 3: Replace the four `#[path]`-including Redis sites**

In each of `redis_jwks_cache.rs`, `api_key_cache_redis.rs`, `authz_cache_redis.rs`, `authz_generations_redis.rs`: delete the whole local `async fn start_redis() -> Option<(ContainerAsync<Redis>, String)> { … }` body and replace it with a one-line delegation. For `redis_jwks_cache.rs` the result is:

```rust
/// Starts an ephemeral Redis container, returning its connection URL. The skip-versus-fail
/// decision lives once, in `support/docker.rs` (SMA-538).
async fn start_redis() -> Option<(ContainerAsync<Redis>, String)> {
    docker::start_redis_or_skip("redis_jwks_cache").await
}
```

Use the file's own name as the `what` label in each: `"api_key_cache_redis"`, `"authz_cache_redis"`, `"authz_generations_redis"`.

Then delete now-unused imports from each file — typically `use testcontainers_modules::testcontainers::runners::AsyncRunner;` and possibly `Redis`/`ContainerAsync` if the signature no longer names them. `cargo clippy -D warnings` will tell you exactly which.

Also update each file's module doc, which says "In CI (`CI` env set) a missing Docker daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note — same gating pattern as `tests/support/mod.rs::start_migrated_postgres`." Replace with:

```rust
//! Runs against an ephemeral Redis in Docker. The Docker-unavailable policy lives once in
//! `tests/support/docker.rs` (SMA-538): a container failure with a reachable daemon is a hard
//! failure, an unreachable daemon skips locally and reds in CI.
```

- [ ] **Step 4: Replace the two `mod support;` Redis sites**

`authz_acceptance.rs` and `api_key_cache_connection.rs` reach the module through `support::docker` rather than a bare `docker`:

```rust
async fn start_redis() -> Option<(ContainerAsync<Redis>, String)> {
    support::docker::start_redis_or_skip("authz_acceptance").await
}
```

and `"api_key_cache_connection"` for the other. Same import cleanup and doc update.

- [ ] **Step 5: Verify the crate compiles and the Redis suites still pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy -p paigasus-iam --all-targets -- -D warnings
CI=1 cargo nextest run -p paigasus-iam --test redis_jwks_cache --test api_key_cache_redis --test authz_cache_redis --test authz_generations_redis --test authz_acceptance --test api_key_cache_connection
```

Expected: clippy clean; all six suites PASS with real containers. **`CI=1` is mandatory here** — without it a broken migration would skip and report a false green.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/
git commit -m "fix(rs): route iam's six redis suites through one start_redis_or_skip (SMA-538)"
```

---

### Task 3: The remaining five call sites

`nats_permissions.rs` gains its first reference to `support/docker.rs`, which is the exact condition AC 5 attaches to — so the `repo:nats-permissions` input change ships in this same commit. Splitting them would leave a window where a `docker.rs` edit changes the compiled binary without re-keying the gate that proves the committed NATS permission set has not been loosened, and Moon would serve a cached PASS.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs:70-80` and `:150-160`
- Modify: `rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs:80-90`
- Modify: `rs/crates/services/paigasus-iam/tests/nats_publisher.rs:25-38`
- Modify: `rs/crates/services/paigasus-iam/tests/nats_permissions.rs:~136-146` (+ add the `#[path]` include)
- Modify: `moon.yml:210-221` (`repo:nats-permissions` comment + inputs)

**Interfaces:**
- Consumes: `start_or_skip` from Task 2.
- Produces: nothing new.

- [ ] **Step 1: Replace both Postgres starters in `support/mod.rs`**

In `start_migrated_postgres` (`:70`), replace the `match … start().await { … }` block with:

```rust
pub async fn start_migrated_postgres() -> Option<(ContainerAsync<Postgres>, DatabaseConnection)> {
    let node = docker::start_or_skip(Postgres::default().with_tag("16-alpine"), "start_migrated_postgres").await?;

    let url = connection_url(&node).await;
    let db = connect_when_ready(&url).await;
    Migrator::up(&db, None).await.unwrap();

    Some((node, db))
}
```

And in `start_raw_postgres` (`:150`):

```rust
pub async fn start_raw_postgres() -> Option<(ContainerAsync<Postgres>, DatabaseConnection)> {
    let node = docker::start_or_skip(Postgres::default().with_tag("16-alpine"), "start_raw_postgres").await?;
    // Through `connection_url`, not a second inline `format!` — one definition of the URL, as
    // that helper's doc claims (CodeRabbit SMA-489 round 1).
    let mut opts = ConnectOptions::new(connection_url(&node).await);
    opts.max_connections(1).min_connections(1);
    // Same startup race as `start_migrated_postgres` — see `connect_when_ready`'s doc.
    let db = connect_when_ready(opts).await;
    Some((node, db))
}
```

Update the module doc at `support/mod.rs:6-9` to point at the single policy instead of restating it.

- [ ] **Step 2: Replace the Keycloak site**

In `keycloak_e2e.rs`, replace the `match image.start().await { … }` block (`:80`) with:

```rust
    let Some(keycloak) = support::docker::start_or_skip(image, "keycloak_e2e").await else {
        return;
    };
```

- [ ] **Step 3: Replace the two NATS sites**

`nats_publisher.rs`:

```rust
async fn start_nats() -> Option<(ContainerAsync<Nats>, String)> {
    let cmd = NatsServerCmd::default().with_jetstream();
    let node = support::docker::start_or_skip(Nats::default().with_cmd(&cmd), "nats_publisher").await?;
    let url = url_of(&node).await;
    Some((node, url))
}
```

`nats_permissions.rs` — add the include near the top, after the `use` block:

```rust
// This file has no `mod support;` — including `support/docker.rs` directly keeps it that way,
// pulling in one small standalone file rather than the whole support surface (SMA-538).
#[path = "support/docker.rs"]
mod docker;
```

and replace its `match image.start().await { … }` block:

```rust
    let node = docker::start_or_skip(image, "nats_permissions").await?;
```

- [ ] **Step 4: Re-key the `repo:nats-permissions` gate**

In `moon.yml`, replace the "NOT included" comment block (currently at `:210-213`) with:

```yaml
    # NOT included: 'tests/support/**/*' (~748 lines of shared auth/tenancy/audit/API-key test
    # infra) — nats_permissions.rs still has no `mod support;`, so that surface is not a
    # dependency and including it would trigger this Docker-backed, TLS-cert-minting suite on
    # unrelated test-fixture changes. Its ONE standalone file IS a dependency since SMA-538
    # (`#[path = "support/docker.rs"] mod docker;` for the shared skip policy) and is listed
    # below individually — without it a policy edit would change this binary while leaving the
    # gate that proves the committed permission set has not been loosened on a cached PASS.
```

and add to `inputs:`:

```yaml
      - 'rs/crates/services/paigasus-iam/tests/support/docker.rs'
```

- [ ] **Step 5: Verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy -p paigasus-iam --all-targets -- -D warnings
CI=1 cargo nextest run -p paigasus-iam --test keycloak_e2e --test nats_publisher --test nats_permissions --test roundtrip --test audit_log_partition_pg
```

Expected: clippy clean; all five PASS. (`roundtrip` exercises `start_migrated_postgres`, `audit_log_partition_pg` exercises `start_raw_postgres`.)

Then confirm the gate actually re-keys on the new input:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd .. && moon query tasks --affected --base origin/main | grep nats-permissions
```

Expected: the task is listed, because `tests/support/docker.rs` changed.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/ moon.yml
git commit -m "fix(rs): route iam's postgres, keycloak and nats suites through start_or_skip (SMA-538)"
```

---

### Task 4: The canary

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/docker_preflight.rs`
- Modify: `rs/.config/nextest.toml` (new override, placed ABOVE the general `kind(test)` block)
- Modify: `moon.yml` (`repo:nats-permissions` script + one more input)

**Interfaces:**
- Consumes: `skip_docker`, `start_redis_or_skip` from Tasks 1–2.
- Produces: test binary `docker_preflight`.

- [ ] **Step 1: Write the canary**

```rust
// SPDX-License-Identifier: Apache-2.0

//! The canary that makes a Docker-less run of this crate impossible to miss (SMA-538).
//!
//! 57 of this crate's 60 integration binaries start a container, and each returns early when
//! Docker is unavailable — reporting PASS in under a second having executed nothing. The
//! `SKIP[docker-unavailable]` markers those suites print cannot fix that: nextest discards a
//! PASSING test's stderr (`success-output` defaults to `never`) and Moon discards a passing
//! TASK's output (`buffer-only-failure` in `.moon/tasks.yml`).
//!
//! So this test FAILS instead. A failure is shown by both. One red, named for the actual
//! problem, in place of 56 silent greens.
//!
//! It starts a real Redis rather than pinging the daemon: testcontainers exposes no ping, and
//! merely constructing a client is not a probe — that succeeds when the endpoint exists with
//! nothing listening. Redis is already pulled by five other suites, so this costs no new image.
//! Reusing `start_redis_or_skip` means the canary exercises the very policy it guards.

#[path = "support/docker.rs"]
mod docker;

#[tokio::test]
async fn docker_backed_suites_can_actually_run() {
    if docker::skip_docker() {
        eprintln!("SKIP[docker-unavailable] docker_preflight: PAIGASUS_SKIP_DOCKER is set");
        return;
    }

    assert!(
        docker::start_redis_or_skip("docker_preflight").await.is_some(),
        "Docker is unreachable, so 56 of this crate's 60 integration suites will report PASS \
         having executed nothing.\n  \
         Start the daemon, or re-run with PAIGASUS_SKIP_DOCKER=1 to accept the skips."
    );
}
```

- [ ] **Step 2: Run it with Docker up to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test docker_preflight
```

Expected: PASS in ~1s.

- [ ] **Step 3: Run it against a dead daemon to verify it actually bites**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && DOCKER_HOST=tcp://127.0.0.1:1 cargo nextest run -p paigasus-iam --test docker_preflight
```

Expected: **FAIL**, printing the "56 of this crate's 60 integration suites" message. This is the whole point of the issue — if it passes here, the classifier is wrong.

- [ ] **Step 4: Add the nextest override**

In `rs/.config/nextest.toml`, insert this **immediately above** the existing `[[profile.default.overrides]]` block whose filter is `'package(=paigasus-iam) and kind(test)'`. Order is load-bearing: nextest applies the first override that configures a given setting, top to bottom, so a block placed below the general one would never set `retries`.

```toml
[[profile.default.overrides]]
# The SMA-538 canary. `retries = 1`, not the general block's 2: when Docker is genuinely
# unreachable every attempt fails identically, so the extra attempt only adds ~45s of backoff to
# a run that is already going to red. One retry is still kept, because the policy now hard-fails
# a container that could not start with a REACHABLE daemon — exactly the transient class
# SMA-521's retry budget exists to absorb — and this test is a mandatory gate.
# Deliberately does NOT set `test-group`; it inherits `docker-containers` from the block below,
# the same per-setting precedence keycloak_e2e relies on.
filter = 'package(=paigasus-iam) and binary(docker_preflight)'
retries = 1
```

- [ ] **Step 5: Make `repo:nats-permissions` run the canary too**

That gate is a filtered run, so the canary would otherwise not protect it. `--test` is repeatable. In `moon.yml`, change the nextest line to:

```
      ( cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --test nats_permissions --test docker_preflight --profile iam-nats )
```

and add to its `inputs:`:

```yaml
      - 'rs/crates/services/paigasus-iam/tests/docker_preflight.rs'
```

- [ ] **Step 6: Verify the override took effect and the gate still passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && DOCKER_HOST=tcp://127.0.0.1:1 cargo nextest run -p paigasus-iam --test docker_preflight 2>&1 | grep -c "TRY 3"
```

Expected: `0` — two attempts only, so there is no third.

Count `TRY 3`, not `TRY 2`: nextest reprints the FINAL attempt's status line in its end-of-run
summary, so under a correct `retries = 1` the string `TRY 2` legitimately appears twice. `TRY 3`
is the unambiguous discriminator — it appears only if the general block's `retries = 2` won.
A second confirmation: a bare `retries = 1` has no backoff, so a correct run prints no `DELAY`
lines at all, while the general block's exponential backoff always does.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-538
moon run repo:nats-permissions
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/docker_preflight.rs rs/.config/nextest.toml moon.yml
git commit -m "fix(rs): add a canary so a docker-less iam run reds instead of passing silently (SMA-538)"
```

---

### Task 5: The single-site gate

Consolidation makes AC 1 true today; this is what keeps it true. The 11 copies accumulated across five issues precisely because nothing failed when a new one appeared.

**Files:**
- Modify: `moon.yml` (new `repo:iam-docker-policy-single-site` task)
- Modify: `CLAUDE.md` (add it to the full-graph gate list at `:~62`)

**Interfaces:**
- Consumes: nothing.
- Produces: Moon task `repo:iam-docker-policy-single-site`.

- [ ] **Step 1: Add the gate**

In `moon.yml`, alongside the other `repo:` gates (place it next to `redis-connect-single-site`, whose shape it follows):

```yaml
  iam-docker-policy-single-site:
    description: 'Assert the Docker-unavailable decision exists exactly once, in tests/support/docker.rs, so a new container-backed suite cannot hand-roll copy #12 (SMA-538).'
    # WHAT IS GATED: reading the `CI` environment variable anywhere under this crate's tests/
    # except in the one file that owns the policy. That read is the tell — every one of the 11
    # copies SMA-538 removed was `if std::env::var_os("CI").is_some() { panic!(..) }` followed by
    # an `eprintln!` and a `return None`.
    #
    # Modelled on repo:redis-connect-single-site, including its two portability lessons: do NOT
    # anchor paths on `^\./` (GNU grep emits the prefix, ugrep strips it), and filter comment
    # lines by CONTENT (`:[0-9]+:[[:space:]]*//`) so prose may still name the variable.
    #
    # The control — `expected` must be non-empty — is what catches a pattern typo or a rename:
    # without it both greps could go empty and the gate would pass while guarding nothing.
    script: |
      cd rs/crates/services/paigasus-iam
      hits="$(grep -rnE 'var_os\("CI"\)|env::var\("CI"\)' tests | grep -vE ':[0-9]+:[[:space:]]*//' || true)"
      expected="$(printf '%s\n' "$hits" | grep -E '^tests/support/docker\.rs:' || true)"
      offenders="$(printf '%s\n' "$hits" | grep -vE '^tests/support/docker\.rs:' | grep -v '^$' || true)"
      if [ -z "$expected" ]; then
        echo "no CI check found in tests/support/docker.rs — the guard is not guarding anything (moved? renamed?)" >&2
        exit 2
      fi
      if [ -n "$offenders" ]; then
        echo "Docker-skip policy hand-rolled outside tests/support/docker.rs (SMA-538 — call start_or_skip):" >&2
        printf '%s\n' "$offenders" >&2
        exit 1
      fi
    toolchain: 'system'
    # Narrow inputs — `repo` owns the whole tree, so without these the guard runs on every change.
    inputs:
      - 'rs/crates/services/paigasus-iam/tests/**/*'
```

- [ ] **Step 2: Verify the gate passes on the current tree**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-538
moon run repo:iam-docker-policy-single-site
```

Expected: PASS.

- [ ] **Step 3: Prove the gate actually bites**

Temporarily add `let _ = std::env::var_os("CI");` to `rs/crates/services/paigasus-iam/tests/health.rs`, then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:iam-docker-policy-single-site
```

Expected: **FAIL**, naming `tests/health.rs`. Then remove the line and re-run to confirm PASS.

> A gate that cannot report red is worse than no gate. Do not skip this step. Remove the temporary line with `git checkout -- rs/crates/services/paigasus-iam/tests/health.rs`, not by hand-editing, so no stray whitespace survives.

- [ ] **Step 4: Add it to the CLAUDE.md gate list**

In `CLAUDE.md`, the "Gotchas" bullet listing the full-graph command, append `:iam-docker-policy-single-site` to the target list so the documented invocation matches what CI runs.

- [ ] **Step 5: Commit**

```bash
git add moon.yml CLAUDE.md
git commit -m "feat(repo): gate the iam docker-skip policy to a single site (SMA-538)"
```

---

### Task 6: Documentation

**Files:**
- Modify: `CLAUDE.md:104-111`
- Modify: `docs/dev-setup.md:67-74`

**Interfaces:**
- Consumes: everything above.
- Produces: nothing.

- [ ] **Step 1: Rewrite the CLAUDE.md gotcha**

Replace the bullet at `CLAUDE.md:104-111` — which currently describes the silent skip as a live hazard and prescribes `CI=1` as the only defence — with:

```markdown
- `paigasus-iam`'s **Docker-backed** suites (57 of its 60 integration binaries) skip when the
  daemon is unreachable, and that skip is deliberately quiet — nextest discards a passing test's
  stderr and Moon discards a passing task's output, so no message can surface there. What makes
  it visible is `tests/docker_preflight.rs`, a canary that FAILS when Docker is unreachable: a
  Docker-less run yields exactly one red instead of 56 silent passes (SMA-538). The policy itself
  lives once, in `tests/support/docker.rs`, and `repo:iam-docker-policy-single-site` fails if a
  new suite hand-rolls its own copy. Two env vars, both parsing `1`/`true`/`yes` (anything else,
  including `0`, is off — unlike `CI`, which is presence-based):
  `PAIGASUS_REQUIRE_DOCKER=1` turns every suite's skip into a panic, which is what a FILTERED run
  (`--test relay_pg`, `-E 'test(foo)'`) needs, since the canary is not in that filter.
  `PAIGASUS_SKIP_DOCKER=1` restores skipping everywhere including the canary — it is a
  per-invocation escape hatch for a Docker Hub rate limit or a daemon restart, **not** a
  shell-profile setting, and a `moon run` that greened under it leaves a cached PASS that replays
  after Docker returns, so follow it with `moon run … --force`. `CI` outranks both, so no
  workflow-file env var can green a CI run that tested nothing. A container that fails with a
  REACHABLE daemon is now a hard failure everywhere — including `keycloak_e2e`'s 240s startup
  timeout, which used to be a fast local skip.
```

- [ ] **Step 2: Rewrite the dev-setup.md bullet**

Replace the bullet at `docs/dev-setup.md:67-74` with:

```markdown
- **`paigasus-iam`'s Docker-backed integration suites need Docker.** 57 of its 60 integration
  binaries start a container and skip when the daemon is unreachable. You will not miss it:
  `tests/docker_preflight.rs` fails in that case, so a Docker-less `cargo nextest run -p
  paigasus-iam` reports exactly one failure naming the problem, rather than a green run that
  executed nothing. Skips themselves print `SKIP[docker-unavailable] <suite>: <error>`, greppable
  if you re-run with `--success-output immediate`.
  - `PAIGASUS_REQUIRE_DOCKER=1` — every suite panics instead of skipping. Use it with a
    filtered run (`--test relay_pg`), which does not include the canary.
  - `PAIGASUS_SKIP_DOCKER=1` — everything skips, canary included. For a Docker Hub rate limit or
    a daemon restart. Per-invocation: putting it in your shell profile puts you straight back to
    green runs that tested nothing, and a `moon run` under it caches the green, so add
    `--force` on the next real run.
  - Both parse `1`/`true`/`yes` only; `0` and unset are off. `CI` is presence-based (any value
    means CI) and outranks both. If you carry a stray `CI=false`, use
    `env -u CI cargo nextest run -p paigasus-iam`.
  - A container that fails to start while the daemon IS reachable is a hard failure, not a skip.
```

- [ ] **Step 3: Verify the docs describe reality**

Re-read both bullets against `tests/support/docker.rs`'s `start_or_skip` doc comment. Every rule stated in prose must match the code's ordered rules 1–6. Fix any drift now.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs/dev-setup.md
git commit -m "docs(repo): document iam's docker canary and its two escape hatches (SMA-538)"
```

---

### Task 7: Full verification

No code changes. This is the gate that proves the whole thing behaves as specced.

**Files:** none.

**Interfaces:** consumes everything.

- [ ] **Step 1: Docker up — everything green, nothing skipped**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && CI=1 cargo nextest run -p paigasus-iam
```

Expected: **349** tests pass — the 339 that exist today, plus the canary, plus Task 1's 9 policy unit tests. Take the wall-clock time; it should stay within the range `rs/.config/nextest.toml` records (~84s at `max-threads = 8`).

- [ ] **Step 2: Daemon unreachable — exactly one red**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && DOCKER_HOST=tcp://127.0.0.1:1 cargo nextest run -p paigasus-iam
```

Expected: **exactly one failure**, `docker_preflight`. Every other suite passes (skipping). If any *other* suite fails, the classifier is mis-classifying a transport error as a container failure.

- [ ] **Step 3: The escape hatch works**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && DOCKER_HOST=tcp://127.0.0.1:1 PAIGASUS_SKIP_DOCKER=1 cargo nextest run -p paigasus-iam
```

Expected: **fully green**.

- [ ] **Step 4: `REQUIRE` escalates, and `CI` outranks `SKIP`**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
DOCKER_HOST=tcp://127.0.0.1:1 PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test relay_pg
DOCKER_HOST=tcp://127.0.0.1:1 PAIGASUS_SKIP_DOCKER=1 CI=1 cargo nextest run -p paigasus-iam --test relay_pg
```

Expected: **both FAIL**. The first proves `REQUIRE` covers a filtered run the canary cannot reach; the second proves a stray `PAIGASUS_SKIP_DOCKER` cannot green CI.

- [ ] **Step 5: Run the full gate graph exactly as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-538
moon ci :build :test :lint :fmt :deny :osv :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site \
  :iam-docker-policy-single-site :promtool :observability-drift :nats-permissions \
  :release-parity :release-parity-py :release-parity-ts :publish-metadata \
  --base origin/main --include-relations
```

Expected: all green. If Moon reports an unattributed failure, diagnose with
`jq '.actions[]|select(.status=="failed")' .moon/cache/ciReport.json`.

- [ ] **Step 6: Confirm no skip-policy copies remain**

```bash
grep -rnE 'var_os\("CI"\)|env::var\("CI"\)' rs/crates/services/paigasus-iam/tests
```

Expected: hits in `tests/support/docker.rs` **only**. This is AC 1, checked by hand as well as by the gate.
