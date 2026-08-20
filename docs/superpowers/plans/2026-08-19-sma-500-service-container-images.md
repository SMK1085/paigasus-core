# SMA-500 Container Images Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship multi-stage container images for `paigasus-iam` and `paigasus-gateway` on a chiseled Ubuntu 24.04 base, verified end-to-end in CI, establishing the conventions the console images will inherit.

**Architecture:** One parameterized `rs/Dockerfile` (`ARG BIN`) with three stages — a pinned `rust:1.95.0-bookworm` builder, an `ubuntu:24.04` stage that `chisel cut`s a 5.3 MB rootfs, and a `FROM scratch` final stage. The images are shell-less, so each service binary probes itself via a `healthcheck` subcommand backed by one shared implementation in `paigasus-observability`. A dedicated `images.yml` workflow builds and smoke-tests both images; nothing is published.

**Tech Stack:** Rust 1.95.0 (edition 2024), Docker/BuildKit + buildx, chisel v1.4.2, Ubuntu 24.04, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-08-19-sma-500-service-container-images-design.md` — read it before Task 1.

## Global Constraints

- Every new source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0` (`#` for shell, Dockerfile, YAML).
- Rust is **edition 2024, rust-version 1.95**. Workspace lints are `[workspace.lints.rust] warnings = "deny"` and `[workspace.lints.clippy] all = "warn"`, and `lint` runs `cargo clippy --locked --all-targets -- -D warnings`. **Dead code is a hard compile error**, so never add an unused item "to wire up later".
- Bash tool PATH lacks proto CLIs. Every command below assumes: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Run cargo from **inside `rs/`** (`rs/.cargo/config.toml` is found by walking up from the cwd).
- Branch: `feature/sma-500-service-container-images`. Conventional commits with a workspace scope, **lowercase subject**, header ≤ 100 chars. Never put a bare `#NNN` in a commit body (commitlint reads it as a footer). Do **not** use `--no-verify`.
- Pinned values, already resolved — copy verbatim, do not re-derive:
  - `rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1`
  - `ubuntu:24.04@sha256:d78ab76437b1afc5f01e223d6bf0172763f404bb166441328845adbef44518cb`
  - chisel `v1.4.2` amd64 sha384: `8e5e8df4dc783dcfa827ca9990ba871af350738de67c51706b3c06bfd4725ab0edbddd9ad4110d1047ecfdc586f7dac6`
  - chisel `v1.4.2` arm64 sha384: `216f10d4cc461411558fa4ac03fc24e104589126f87657457877389ae8015e1eac4299fcd8557c0dfea3d33342aa3297`
- Chisel slices, exactly: `base-files_base base-files_release-info libc6_libs libgcc-s1_libs ca-certificates_data`. `libgcc-s1_libs` is **required** (Rust panic unwinding links `libgcc_s.so.1`); omitting it fails at container start, not build.
- The runtime rootfs has **no `/etc/passwd`** — `USER` must be numeric (`65532:65532`).
- Exit-code contract for the probe: **0 = healthy, 1 = unhealthy, 2 = usage error**.

## File Structure

| File | Responsibility |
| --- | --- |
| `rs/crates/libs/paigasus-observability/src/health.rs` | **New.** The one implementation of the HTTP probe and the argv dispatch both services share |
| `rs/crates/libs/paigasus-observability/src/lib.rs` | **Modify.** Declare + re-export the module |
| `rs/crates/services/paigasus-iam/src/main.rs` | **Modify.** Dispatch before the tokio runtime starts |
| `rs/crates/services/paigasus-gateway/src/main.rs` | **Modify.** Same |
| `rs/Dockerfile` | **New.** One parameterized three-stage build for both services |
| `rs/.dockerignore` | **New.** Keep `target/` out of the build context |
| `ci/images/run.sh` | **New.** `{build,smoke,all}` — the single entry point humans and CI both call |
| `.github/workflows/images.yml` | **New.** Build + smoke, no publish |
| `.github/dependabot.yml` | **Modify.** Add the `docker` ecosystem so the digests cannot rot |
| `docs/ops/RUNBOOK-containers.md` | **New.** Env contract, probe contract, image names, operational rules |
| `CLAUDE.md` | **Modify.** Gotchas future sessions need |

---

### Task 1: The HTTP probe

**Files:**
- Create: `rs/crates/libs/paigasus-observability/src/health.rs`
- Modify: `rs/crates/libs/paigasus-observability/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `paigasus_observability::health::probe(addr: SocketAddr, path: &str, deadline: Duration) -> std::io::Result<bool>`. `Ok(true)` iff the response status is 2xx. Used by Task 3.

- [ ] **Step 1: Write the failing tests**

Create `rs/crates/libs/paigasus-observability/src/health.rs` containing ONLY the SPDX header, the module doc, and this test module (the implementation arrives in Step 3 — writing it now would make Step 2 meaningless):

```rust
// SPDX-License-Identifier: Apache-2.0

//! The liveness/readiness probe the container images' `HEALTHCHECK` runs.
//!
//! The images are shell-less (`FROM scratch` over a chiseled Ubuntu rootfs, SMA-500), so
//! `curl`/`wget` do not exist and each service binary probes itself instead. One
//! implementation lives here rather than in each `main.rs`, the same single-site discipline
//! `repo:redis-connect-single-site` enforces elsewhere.

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpListener};
    use std::thread;
    use std::time::Duration;

    /// Serve exactly one request with `status_line`, then close. Returns the bound address.
    fn serve_once(status_line: &'static str) -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let _ = sock.write_all(format!("{status_line}\r\nContent-Length: 0\r\n\r\n").as_bytes());
            }
        });
        addr
    }

    #[test]
    fn a_2xx_response_is_healthy() {
        let addr = serve_once("HTTP/1.1 200 OK");
        assert!(probe(addr, "/healthz", Duration::from_secs(2)).expect("probe ran"));
    }

    #[test]
    fn a_503_response_is_unhealthy_but_not_an_error() {
        let addr = serve_once("HTTP/1.1 503 Service Unavailable");
        assert!(!probe(addr, "/readyz", Duration::from_secs(2)).expect("probe ran"));
    }

    #[test]
    fn a_refused_connection_is_an_error() {
        // Bind then drop, so the port is almost certainly closed.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        drop(listener);
        assert!(probe(addr, "/healthz", Duration::from_secs(2)).is_err());
    }

    #[test]
    fn an_unspecified_address_is_probed_on_loopback() {
        // Services bind 0.0.0.0 by default, which is NOT a destination. The probe must
        // rewrite it to loopback rather than dial it.
        let addr = serve_once("HTTP/1.1 200 OK");
        let unspecified = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED), addr.port());
        assert!(probe(unspecified, "/healthz", Duration::from_secs(2)).expect("probe ran"));
    }

    #[test]
    fn a_server_that_accepts_but_never_responds_hits_the_total_deadline() {
        // The regression this pins: a connect-only timeout would block here forever.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        thread::spawn(move || {
            let held = listener.accept();
            thread::sleep(Duration::from_secs(30));
            drop(held);
        });
        let started = std::time::Instant::now();
        assert!(probe(addr, "/healthz", Duration::from_millis(300)).is_err());
        assert!(started.elapsed() < Duration::from_secs(5), "probe must honour its deadline, took {:?}", started.elapsed());
    }

    #[test]
    fn a_malformed_status_line_is_an_error() {
        let addr = serve_once("NOT-HTTP");
        assert!(probe(addr, "/healthz", Duration::from_secs(2)).is_err());
    }
}
```

Add to `rs/crates/libs/paigasus-observability/src/lib.rs`, after the existing `pub mod grpc;` line so the module list stays alphabetical:

```rust
pub mod health;
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-observability health:: 2>&1 | tail -20
```

Expected: FAIL to compile — `cannot find function 'probe' in this scope`.

- [ ] **Step 3: Write the implementation**

Insert above the `#[cfg(test)] mod tests` block in `health.rs`:

```rust
use std::io::{self, BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// Probe `path` on `addr` over plaintext HTTP/1.1, within a TOTAL `deadline`.
///
/// `Ok(true)` iff the response status is 2xx; `Ok(false)` for any other status (a 503 from
/// `/readyz` is a healthy *answer*, not a failure); `Err` if the service could not be reached,
/// answered nothing, or answered something that is not HTTP.
///
/// Neither service terminates TLS (`axum-server` is a dev-dependency only), so plaintext is
/// correct today; both images require a TLS-terminating ingress.
pub fn probe(addr: SocketAddr, path: &str, deadline: Duration) -> io::Result<bool> {
    let started = Instant::now();
    let target = connectable(addr);

    let mut stream = TcpStream::connect_timeout(&target, remaining(deadline, started)?)?;

    // `deadline` is a TOTAL budget, not a connect timeout. A server that has accepted but
    // wedged (a saturated axum, a blocked handler) would otherwise block this call forever.
    stream.set_write_timeout(Some(remaining(deadline, started)?))?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {target}\r\nUser-Agent: paigasus-healthcheck\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    stream.set_read_timeout(Some(remaining(deadline, started)?))?;
    let mut status_line = String::new();
    BufReader::new(&stream).read_line(&mut status_line)?;
    status_is_success(&status_line)
}

/// The budget left, or `TimedOut` once it is gone. Never returns `Duration::ZERO`: std reads a
/// zero timeout as "no timeout at all", which would silently remove the bound this enforces.
fn remaining(deadline: Duration, started: Instant) -> io::Result<Duration> {
    let left = deadline.saturating_sub(started.elapsed());
    if left.is_zero() {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "health probe deadline exceeded"));
    }
    Ok(left)
}

/// Both services default to `0.0.0.0`, which is an unspecified address, not a destination.
fn connectable(addr: SocketAddr) -> SocketAddr {
    match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port()),
        IpAddr::V6(ip) if ip.is_unspecified() => SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), addr.port()),
        _ => addr,
    }
}

fn status_is_success(status_line: &str) -> io::Result<bool> {
    let code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, format!("malformed HTTP status line: {status_line:?}"))
        })?;
    Ok((200..300).contains(&code))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd rs && cargo test -p paigasus-observability health:: 2>&1 | tail -20
cargo clippy -p paigasus-observability --all-targets -- -D warnings
```

Expected: 6 passed. Clippy clean.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-observability/src/health.rs rs/crates/libs/paigasus-observability/src/lib.rs
git commit -m "feat(rs): add the shared container health probe (SMA-500)"
```

---

### Task 2: Argv dispatch

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/health.rs`

**Interfaces:**
- Consumes: Task 1's module.
- Produces: `health::Mode` (`Healthcheck { path: String }` | `Serve`), `health::dispatch<I, S>(args: I) -> Result<Mode, String>`, `health::DEFAULT_PROBE_PATH`, `health::USAGE`. Used by Task 3.

- [ ] **Step 1: Write the failing tests**

Append inside the existing `mod tests` block in `health.rs`:

```rust
    #[test]
    fn no_arguments_means_serve() {
        assert_eq!(dispatch(Vec::<String>::new()).expect("valid"), Mode::Serve);
    }

    #[test]
    fn bare_healthcheck_defaults_to_healthz() {
        assert_eq!(
            dispatch(["healthcheck"]).expect("valid"),
            Mode::Healthcheck { path: "/healthz".to_string() }
        );
    }

    #[test]
    fn path_flag_selects_readyz() {
        assert_eq!(
            dispatch(["healthcheck", "--path", "/readyz"]).expect("valid"),
            Mode::Healthcheck { path: "/readyz".to_string() }
        );
    }

    #[test]
    fn path_flag_without_a_value_is_a_usage_error() {
        assert!(dispatch(["healthcheck", "--path"]).is_err());
    }

    #[test]
    fn a_path_without_a_leading_slash_is_a_usage_error() {
        assert!(dispatch(["healthcheck", "--path", "healthz"]).is_err());
    }

    #[test]
    fn an_unknown_argument_never_falls_through_to_serve() {
        // The regression this pins: a typo'd `healthchek` in a HEALTHCHECK or a k8s exec probe
        // must NOT boot a second full service. For IAM that would run Database::connect and
        // Migrator::up on every probe interval.
        assert!(dispatch(["healthchek"]).is_err());
        assert!(dispatch(["serve"]).is_err());
        assert!(dispatch(["healthcheck", "--nope"]).is_err());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cd rs && cargo test -p paigasus-observability health:: 2>&1 | tail -20
```

Expected: FAIL to compile — `cannot find function 'dispatch'`, `cannot find type 'Mode'`.

- [ ] **Step 3: Write the implementation**

Insert into `health.rs` above the `probe` function:

```rust
/// The default liveness path. `/readyz` is reachable via `healthcheck --path /readyz`, which is
/// what a Kubernetes `exec` readiness probe uses — the image has no shell to curl with.
pub const DEFAULT_PROBE_PATH: &str = "/healthz";

/// Printed on a usage error, which exits 2 (0 = healthy, 1 = unhealthy, 2 = usage).
pub const USAGE: &str = "usage: <service> [healthcheck [--path <path>]]";

/// What the argv dispatch selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Probe `path` and exit with the health verdict.
    Healthcheck { path: String },
    /// Normal service startup.
    Serve,
}

/// Parse the arguments after the binary name.
///
/// Anything unrecognised is an ERROR rather than a fall-through to [`Mode::Serve`]: a typo'd
/// `healthchek` in a `HEALTHCHECK`, a compose file or a Kubernetes exec probe would otherwise
/// silently start a second full service on every probe interval (SMA-500 D4).
pub fn dispatch<I, S>(args: I) -> Result<Mode, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Ok(Mode::Serve);
    };
    if command.as_ref() != "healthcheck" {
        return Err(format!("unknown argument {:?}\n{USAGE}", command.as_ref()));
    }

    let mut path = DEFAULT_PROBE_PATH.to_string();
    while let Some(flag) = args.next() {
        match flag.as_ref() {
            "--path" => {
                let value = args.next().ok_or_else(|| format!("--path requires a value\n{USAGE}"))?;
                let value = value.as_ref();
                if !value.starts_with('/') {
                    return Err(format!("--path must start with '/', got {value:?}\n{USAGE}"));
                }
                path = value.to_string();
            }
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    Ok(Mode::Healthcheck { path })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd rs && cargo test -p paigasus-observability health:: 2>&1 | tail -20
cargo clippy -p paigasus-observability --all-targets -- -D warnings
```

Expected: 12 passed. Clippy clean.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-observability/src/health.rs
git commit -m "feat(rs): add healthcheck argv dispatch for the service images (SMA-500)"
```

---

### Task 3: Wire both services

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/main.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/main.rs`

**Interfaces:**
- Consumes: `health::{dispatch, probe, Mode, USAGE}` from Tasks 1-2.
- Produces: the `healthcheck` subcommand on both binaries. Task 4's `HEALTHCHECK` line depends on it.

The existing `#[tokio::main] async fn main()` in each file is **renamed to `serve`** and keeps its attribute and its entire body. A new plain `fn main` dispatches first, so a probe never builds a multi-threaded runtime for one blocking request.

- [ ] **Step 1: Restructure `paigasus-iam/src/main.rs`**

Change the existing signature at line 21-22 from:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
```

to:

```rust
#[tokio::main]
async fn serve() -> anyhow::Result<()> {
```

Then add this new entry point immediately above it:

```rust
/// Dispatch before any runtime is built. `healthcheck` is what the image's `HEALTHCHECK` runs:
/// the images are shell-less, so the binary probes itself (SMA-500 D4).
///
/// Exit codes: 0 healthy, 1 unhealthy, 2 usage error.
fn main() -> std::process::ExitCode {
    match paigasus_observability::health::dispatch(std::env::args().skip(1)) {
        Ok(paigasus_observability::health::Mode::Serve) => match serve() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Error: {error:?}");
                std::process::ExitCode::FAILURE
            }
        },
        Ok(paigasus_observability::health::Mode::Healthcheck { path }) => healthcheck(&path),
        Err(usage) => {
            eprintln!("{usage}");
            std::process::ExitCode::from(2)
        }
    }
}

/// `load()` but deliberately NOT `validate()`: the probe needs only `http_addr`, and
/// `IamConfig::validate` rejects a config with no configured issuers — which would fail the
/// healthcheck for a reason that has nothing to do with health.
///
/// The error text is never printed. Docker retains the last five health-check outputs in
/// `State.Health.Log`, and a `figment::Error` names config keys and can carry values from the
/// `IAM_*` env layer.
fn healthcheck(path: &str) -> std::process::ExitCode {
    let Ok(config) = IamConfig::load() else {
        eprintln!("healthcheck: config load failed");
        return std::process::ExitCode::FAILURE;
    };
    match paigasus_observability::health::probe(config.http_addr, path, std::time::Duration::from_secs(2)) {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) | Err(_) => std::process::ExitCode::FAILURE,
    }
}
```

- [ ] **Step 2: Apply the identical restructure to `paigasus-gateway/src/main.rs`**

Same change at its line 21-22 (`async fn main` -> `async fn serve`), and the same two functions, with `IamConfig` replaced by `GatewayConfig`:

```rust
/// Dispatch before any runtime is built. `healthcheck` is what the image's `HEALTHCHECK` runs:
/// the images are shell-less, so the binary probes itself (SMA-500 D4).
///
/// Exit codes: 0 healthy, 1 unhealthy, 2 usage error.
fn main() -> std::process::ExitCode {
    match paigasus_observability::health::dispatch(std::env::args().skip(1)) {
        Ok(paigasus_observability::health::Mode::Serve) => match serve() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Error: {error:?}");
                std::process::ExitCode::FAILURE
            }
        },
        Ok(paigasus_observability::health::Mode::Healthcheck { path }) => healthcheck(&path),
        Err(usage) => {
            eprintln!("{usage}");
            std::process::ExitCode::from(2)
        }
    }
}

/// `load()` but deliberately NOT `validate()`: the probe needs only `http_addr`, and
/// `GatewayConfig::validate` rejects an empty OpenAI API key — which would fail the healthcheck
/// for a reason that has nothing to do with health.
///
/// The error text is never printed (see the IAM counterpart: `State.Health.Log` retains it).
fn healthcheck(path: &str) -> std::process::ExitCode {
    let Ok(config) = GatewayConfig::load() else {
        eprintln!("healthcheck: config load failed");
        return std::process::ExitCode::FAILURE;
    };
    match paigasus_observability::health::probe(config.http_addr, path, std::time::Duration::from_secs(2)) {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) | Err(_) => std::process::ExitCode::FAILURE,
    }
}
```

- [ ] **Step 3: Build and prove the subcommand end-to-end**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-gateway --bin paigasus-gateway
# No service is listening, so the probe must report UNHEALTHY (1), not crash and not hang.
GATEWAY_UPSTREAM__OPENAI__API_KEY=sk-smoke-not-a-real-key ./target/debug/paigasus-gateway healthcheck; echo "healthcheck exit=$?"
# A typo must be a USAGE error (2) and must NOT boot a server.
./target/debug/paigasus-gateway healthchek; echo "typo exit=$?"
```

Expected: `healthcheck exit=1` returning promptly (well under 5s), and `typo exit=2` with the usage line. Neither starts a server.

- [ ] **Step 4: Verify the whole Rust graph still builds and lints**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check 2>&1 | tail -20
cargo clippy --locked --all-targets -- -D warnings 2>&1 | tail -20
cargo nextest run --no-tests=pass -p paigasus-observability -p paigasus-iam -p paigasus-gateway 2>&1 | tail -10
```

Expected: fmt clean (it prints nothing), clippy clean, tests pass.

`cargo fmt --check` is a **separate CI gate** (`moon ci :fmt`) from clippy, and clippy passing says
nothing about it. Tasks 1-2 learned this the hard way: their verification listed only test + clippy,
and rustfmt wanted the multi-line `assert_eq!` calls collapsed, so the branch carried a red `:fmt`
until a fix round caught it.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/main.rs rs/crates/services/paigasus-gateway/src/main.rs
git commit -m "feat(rs): add a healthcheck subcommand to both service binaries (SMA-500)"
```

---

### Task 4: The Dockerfile

**Files:**
- Create: `rs/.dockerignore`
- Create: `rs/Dockerfile`

**Interfaces:**
- Consumes: the `healthcheck` subcommand from Task 3.
- Produces: an image buildable as `docker build -f rs/Dockerfile --build-arg BIN=<crate> rs/`. The binary is installed at the fixed path `/usr/local/bin/paigasus-service`. Task 5 wraps this; Task 6 smoke-tests it.

- [ ] **Step 1: Create `rs/.dockerignore`**

```
# SPDX-License-Identifier: Apache-2.0
# Keep the build context small. `target/` alone is gigabytes on any machine that has ever run
# `cargo build`, and every byte of it is uploaded to the daemon on every build.
target/
**/*.node
**/*.wasm
.git/
```

- [ ] **Step 2: Create `rs/Dockerfile`**

```dockerfile
# syntax=docker/dockerfile:1
# SPDX-License-Identifier: Apache-2.0
#
# One parameterized image for both service binaries (SMA-500 D9). Build with:
#   docker build -f rs/Dockerfile --build-arg BIN=paigasus-iam rs/
# `ci/images/run.sh build` is the supported entry point and adds the pins and labels.

ARG BIN

# Builder glibc MUST be <= runtime glibc (bookworm 2.36 <= noble 2.39). Inverting this produces
# `GLIBC_2.4x not found` at CONTAINER START, not at build time — invisible on a PR, since the
# image workflow is not a required check.
FROM rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 AS builder
# `rust-toolchain.toml` is inside the build context, so rustup would honour it over this image's
# own toolchain and could fetch a DIFFERENT channel over the network — the `FROM` pin above
# would be decorative. Pin explicitly (SMA-500 D3); ci/images/run.sh asserts the two agree.
ENV RUSTUP_TOOLCHAIN=1.95.0
ARG BIN
WORKDIR /src
COPY . .
# The binary is copied OUT of the cache mount inside this same RUN: a cache mount is not part of
# the resulting layer, so `COPY --from=builder /src/target/...` in a later stage finds nothing.
# Distinct cache ids per binary — the default sharing mode is `shared`, so two concurrent builds
# would otherwise contend on cargo's lock.
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=target-${BIN},target=/src/target \
    cargo build --release --locked -p "${BIN}" --bin "${BIN}" \
 && mkdir -p /out && cp "/src/target/release/${BIN}" /out/service

FROM ubuntu:24.04@sha256:d78ab76437b1afc5f01e223d6bf0172763f404bb166441328845adbef44518cb AS rootfs
ARG CHISEL_VERSION=v1.4.2
# One pinned checksum PER ARCHITECTURE: the release publishes a distinct tarball and a distinct
# sha384 per arch, so a single checksum would either break arm64 or silently drop the check.
ARG CHISEL_SHA384_amd64=8e5e8df4dc783dcfa827ca9990ba871af350738de67c51706b3c06bfd4725ab0edbddd9ad4110d1047ecfdc586f7dac6
ARG CHISEL_SHA384_arm64=216f10d4cc461411558fa4ac03fc24e104589126f87657457877389ae8015e1eac4299fcd8557c0dfea3d33342aa3297
RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      amd64) sha="${CHISEL_SHA384_amd64}" ;; \
      arm64) sha="${CHISEL_SHA384_arm64}" ;; \
      *) echo "no pinned chisel checksum for architecture ${arch}" >&2; exit 1 ;; \
    esac; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl; \
    curl -sSLf -o /tmp/chisel.tar.gz \
      "https://github.com/canonical/chisel/releases/download/${CHISEL_VERSION}/chisel_${CHISEL_VERSION}_linux_${arch}.tar.gz"; \
    echo "${sha}  /tmp/chisel.tar.gz" | sha384sum -c -; \
    tar -xzf /tmp/chisel.tar.gz -C /usr/local/bin; \
    mkdir -p /rootfs; \
    chisel cut --release ubuntu-24.04 --root /rootfs \
      base-files_base base-files_release-info \
      libc6_libs libgcc-s1_libs ca-certificates_data

FROM scratch
ARG BIN
COPY --from=rootfs /rootfs /
# Installed at a FIXED path: exec-form ENTRYPOINT/HEALTHCHECK do NOT expand ARG or ENV, so
# ["/usr/local/bin/${BIN}"] would look for a literal ${BIN}. Naming it once is what makes one
# parameterized Dockerfile possible; logs and metrics take the service name from the binary
# itself (paigasus_logging::init), not from argv[0].
COPY --from=builder /out/service /usr/local/bin/paigasus-service
# `FROM scratch` leaves Config.Env empty. Docker injects a default PATH; containerd/CRI does not
# reliably, so a Kubernetes exec probe using a bare command name would fail there without this.
ENV PATH=/usr/local/bin
# Numeric: the chiseled rootfs has no /etc/passwd, so a named user cannot resolve.
USER 65532:65532
# Both binaries install SIGTERM handlers and drain (IAM a JoinSet of relays and maintainers).
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/paigasus-service"]
# 60s start period because IAM runs Migrator::up before it binds.
HEALTHCHECK --interval=30s --timeout=3s --start-period=60s --retries=3 \
  CMD ["/usr/local/bin/paigasus-service", "healthcheck"]
```

- [ ] **Step 3: Build both images and prove they run**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-500
docker build -f rs/Dockerfile --build-arg BIN=paigasus-gateway -t paigasus-gateway:dev rs/
docker build -f rs/Dockerfile --build-arg BIN=paigasus-iam     -t paigasus-iam:dev     rs/
docker run --rm -d --name gw-t -p 18088:8088 -e GATEWAY_UPSTREAM__OPENAI__API_KEY=sk-smoke-not-a-real-key paigasus-gateway:dev
sleep 3
curl -fsS http://127.0.0.1:18088/healthz; echo
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:18088/readyz
docker logs gw-t 2>&1 | tail -5
docker rm -f gw-t
```

Expected: both builds succeed; `/healthz` returns `{"status":"ok"}`; `/readyz` returns **503** (no IAM reachable). If the container exits immediately with a loader error, the slice list is wrong — re-read the Global Constraints.

- [ ] **Step 4: Commit**

```bash
git add rs/Dockerfile rs/.dockerignore
git commit -m "feat(repo): add the parameterized service container image (SMA-500)"
```

---

### Task 5: `ci/images/run.sh build`

**Files:**
- Create: `ci/images/run.sh`

**Interfaces:**
- Consumes: `rs/Dockerfile` from Task 4.
- Produces: `ci/images/run.sh build [iam|gateway|all]`, tagging `paigasus-{iam,gateway}:<git-sha>` plus a `:dev` alias. Task 6 appends `smoke`; Task 7's workflow calls it.

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# SMA-500 — build and smoke-test the service container images.
#
# Deliberately NOT a Moon task: a `repo:*` task would have to join ci.yml's `T=(…)` array (a
# --release build on every affected PR, against a 30-minute timeout and the ~14 GB disk that
# cedar-policy has already overflowed once) or become a T_EXEMPT entry. It runs from
# .github/workflows/images.yml instead.
#
# usage: ci/images/run.sh {build|smoke|all} [iam|gateway]
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
REGISTRY="${PAIGASUS_IMAGE_REGISTRY:-ghcr.io/paigasus}"
REVISION="$(git -C "$ROOT" rev-parse HEAD)"

crate_for() {
  case "$1" in
    iam)     echo "paigasus-iam" ;;
    gateway) echo "paigasus-gateway" ;;
    *) echo "unknown service: $1" >&2; return 1 ;;
  esac
}

# The pins in rs/Dockerfile are only as good as their agreement with the repo's own toolchain
# pin. `FROM rust:X.Y.Z` does NOT decide which compiler runs — rust-toolchain.toml is inside the
# build context and rustup honours it — so a channel bump would leave the Dockerfile looking
# pinned and being nothing of the sort (SMA-500 D3).
assert_pins() {
  local dockerfile="$ROOT/rs/Dockerfile"
  local channel from_version
  channel="$(grep -E '^channel[[:space:]]*=' "$ROOT/rs/rust-toolchain.toml" | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
  from_version="$(grep -oE '^FROM rust:[0-9]+\.[0-9]+\.[0-9]+' "$dockerfile" | head -1 | sed 's/^FROM rust://')"
  if [ "$channel" != "$from_version" ]; then
    echo "::error::rs/Dockerfile builds on rust:${from_version} but rs/rust-toolchain.toml pins ${channel}." >&2
    echo "  Bump the FROM line (and its digest) together with the toolchain, or the image ships a different compiler." >&2
    return 1
  fi
  # Builder glibc must be <= runtime glibc. bookworm is 2.36, noble (ubuntu:24.04) is 2.39.
  # Inverting it fails at CONTAINER START with `GLIBC_2.4x not found`, not at build time.
  if ! grep -qE '^FROM rust:[0-9.]+-bookworm@sha256:' "$dockerfile"; then
    echo "::error::the builder base must stay a digest-pinned -bookworm tag (glibc 2.36 <= the runtime's 2.39)." >&2
    return 1
  fi
  # AC-2: nothing deployment-varying may be baked. Config reaches the container through
  # IAM_*/GATEWAY_* env at RUNTIME only.
  if grep -nE '^[[:space:]]*ENV[[:space:]]+(IAM_|GATEWAY_)' "$dockerfile"; then
    echo "::error::rs/Dockerfile bakes service config into the image; configure at runtime via env instead." >&2
    return 1
  fi
  echo "  pins OK: rustc ${channel}, bookworm builder, no baked service config"
}

build_one() {
  local service="$1" crate tag
  crate="$(crate_for "$service")"
  tag="${REGISTRY}/${crate}:${REVISION}"
  echo "== build ${crate} =="
  # --progress=plain so chisel's `Fetching pool/...` lines are capturable below: they name the
  # exact archive package versions this image resolved, which `chisel cut` re-resolves against
  # the LIVE archive on every build (SMA-500 limitation 2).
  docker build \
    --progress=plain \
    -f "$ROOT/rs/Dockerfile" \
    --build-arg "BIN=${crate}" \
    --label "org.opencontainers.image.title=${crate}" \
    --label "org.opencontainers.image.description=Paigasus ${service} service" \
    --label "org.opencontainers.image.source=https://github.com/paigasus/paigasus-core" \
    --label "org.opencontainers.image.revision=${REVISION}" \
    --label "org.opencontainers.image.licenses=Apache-2.0" \
    -t "$tag" -t "${crate}:dev" \
    "$ROOT/rs" 2>&1 | tee "/tmp/paigasus-build-${service}.log"
  grep -oE 'Fetching pool/[^ ]+\.deb' "/tmp/paigasus-build-${service}.log" | sort -u > "$ROOT/chisel-manifest-${service}.txt" || true
  echo "  built ${tag}"
}

cmd="${1:?usage: ci/images/run.sh {build|smoke|all} [iam|gateway]}"
target="${2:-all}"
services=("iam" "gateway")
[ "$target" != "all" ] && services=("$target")

case "$cmd" in
  build) assert_pins; for s in "${services[@]}"; do build_one "$s"; done ;;
  *) echo "unknown command: $cmd" >&2; exit 1 ;;
esac
```

- [ ] **Step 2: Make it executable and run it**

```bash
chmod +x ci/images/run.sh
./ci/images/run.sh build gateway 2>&1 | tail -15
```

Expected: `pins OK: ...` then a successful build, and `chisel-manifest-gateway.txt` listing `.deb` versions including `libc6_2.39-...`.

- [ ] **Step 3: Prove each guard actually fires**

A guard that cannot fail is not a guard. Verify all three, restoring the file after each:

```bash
cp rs/Dockerfile /tmp/Dockerfile.orig
# 1. toolchain disagreement
sed -i '' 's/^FROM rust:1\.95\.0-bookworm/FROM rust:1.94.0-bookworm/' rs/Dockerfile
./ci/images/run.sh build gateway; echo "expect non-zero: $?"
cp /tmp/Dockerfile.orig rs/Dockerfile
# 2. baked service config
printf '\nENV IAM_DATABASE_URL=postgres://baked\n' >> rs/Dockerfile
./ci/images/run.sh build gateway; echo "expect non-zero: $?"
cp /tmp/Dockerfile.orig rs/Dockerfile
# 3. non-bookworm builder
sed -i '' 's/-bookworm@sha256:/-noble@sha256:/' rs/Dockerfile
./ci/images/run.sh build gateway; echo "expect non-zero: $?"
cp /tmp/Dockerfile.orig rs/Dockerfile
git diff --exit-code rs/Dockerfile && echo "Dockerfile restored cleanly"
```

Expected: all three print an `::error::` line and exit non-zero, and the final `git diff` is clean.

- [ ] **Step 4: Add the manifest artifacts to .gitignore and commit**

```bash
printf '\n# SMA-500: chisel package manifests are CI artifacts, not tracked files.\nchisel-manifest-*.txt\n' >> .gitignore
git add ci/images/run.sh .gitignore
git commit -m "feat(repo): add the container image build script and its pin guards (SMA-500)"
```

---

### Task 6: `ci/images/run.sh smoke`

**Files:**
- Modify: `ci/images/run.sh`

**Interfaces:**
- Consumes: images tagged `{crate}:dev` by Task 5.
- Produces: `ci/images/run.sh smoke` and `all`. Task 7's workflow calls `all`.

- [ ] **Step 1: Add the smoke function**

Insert before the `cmd=` line in `ci/images/run.sh`:

```bash
NET="paigasus-smoke-$$"
cleanup() {
  docker rm -f smoke-iam smoke-gw smoke-pg >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
}

# Poll until the container's own HEALTHCHECK reports healthy. This is the ONLY assertion that
# exercises the probe binary INSIDE the shell-less image — an outside-the-container curl passes
# even when HEALTHCHECK is broken.
wait_healthy() {
  local name="$1" i status
  for i in $(seq 1 60); do
    status="$(docker inspect --format '{{.State.Health.Status}}' "$name" 2>/dev/null || echo missing)"
    [ "$status" = "healthy" ] && { echo "  $name is healthy (in-image probe, ${i}s)"; return 0; }
    [ "$status" = "missing" ] && { echo "::error::$name is gone; logs follow" >&2; docker logs "$name" 2>&1 | tail -30 >&2; return 1; }
    sleep 1
  done
  echo "::error::$name never became healthy (last status: $status)" >&2
  docker logs "$name" 2>&1 | tail -30 >&2
  return 1
}

expect_status() {
  local label="$1" url="$2" want="$3" got
  got="$(docker run --rm --network "$NET" curlimages/curl:8.11.1 -s -o /dev/null -w '%{http_code}' "$url" || echo 000)"
  if [ "$got" != "$want" ]; then
    echo "::error::${label}: expected HTTP ${want}, got ${got}" >&2
    return 1
  fi
  echo "  ${label}: HTTP ${got}"
}

# The base must stay the base. Without these a future `FROM ubuntu:24.04` "just to debug
# something" would pass every other assertion in this suite.
assert_base_intact() {
  local image="$1" certs size
  if docker run --rm --entrypoint /bin/sh "$image" -c true >/dev/null 2>&1; then
    echo "::error::${image} has a shell; the runtime base must stay chiseled/scratch." >&2
    return 1
  fi
  docker create --name certprobe "$image" >/dev/null
  certs="$(docker cp certprobe:/etc/ssl/certs/ca-certificates.crt - 2>/dev/null | tar -xO 2>/dev/null | grep -c 'BEGIN CERTIFICATE' || true)"
  docker rm -f certprobe >/dev/null
  if [ "${certs:-0}" -lt 100 ]; then
    echo "::error::${image} carries ${certs} CA certificates; the trust bundle is missing or truncated." >&2
    return 1
  fi
  size="$(docker image inspect --format '{{.Size}}' "$image")"
  if [ "$size" -gt 209715200 ]; then
    echo "::error::${image} is ${size} bytes, over the 200 MB ceiling — the runtime base has probably grown." >&2
    return 1
  fi
  echo "  ${image}: no shell, ${certs} CA certs, $((size / 1024 / 1024)) MB"
}

smoke() {
  trap cleanup EXIT
  cleanup
  docker network create "$NET" >/dev/null

  echo "== gateway: standalone =="
  # Runtime-only config (AC-2): env vars ONLY, no mounted file, no --env-file. Success IS the
  # proof. The key is a literal dummy and must never be a real one.
  docker run -d --name smoke-gw --network "$NET" \
    -e GATEWAY_UPSTREAM__OPENAI__API_KEY=sk-smoke-not-a-real-key \
    paigasus-gateway:dev >/dev/null
  wait_healthy smoke-gw
  expect_status "gateway /healthz" "http://smoke-gw:8088/healthz" 200
  # The NEGATIVE case is the point: no IAM is reachable, so a /readyz returning 200 is lying.
  expect_status "gateway /readyz (no IAM)" "http://smoke-gw:8088/readyz" 503
  # `if !` rather than `cmd; [ $? -eq 1 ]`: under `set -e` a bare non-zero command aborts the
  # script, and exiting 1 here is the EXPECTED result (the gateway is unready without IAM).
  if docker exec smoke-gw /usr/local/bin/paigasus-service healthcheck --path /readyz; then
    echo "::error::gateway readyz probe reported healthy with no IAM reachable" >&2
    return 1
  fi
  echo "  gateway readyz probe exits non-zero while unready (in-image, --path works)"
  assert_base_intact paigasus-gateway:dev

  echo "== iam: with postgres, reached BY HOSTNAME =="
  docker run -d --name smoke-pg --network "$NET" \
    -e POSTGRES_PASSWORD=smoke -e POSTGRES_DB=iam postgres:16-alpine >/dev/null
  sleep 8
  # `smoke-pg`, never 127.0.0.1: this is what exercises glibc name resolution inside the
  # chiseled rootfs. An IP literal would bypass NSS entirely and the assertion would go vacuous.
  docker run -d --name smoke-iam --network "$NET" \
    -e IAM_DATABASE_URL="postgres://postgres:smoke@smoke-pg:5432/iam" \
    -e IAM_AUTHN__ISSUERS='[{issuer="https://idp.example.com",audiences=["paigasus"]}]' \
    paigasus-iam:dev >/dev/null
  wait_healthy smoke-iam
  expect_status "iam /healthz" "http://smoke-iam:8080/healthz" 200
  expect_status "iam /readyz"  "http://smoke-iam:8080/readyz"  200
  assert_base_intact paigasus-iam:dev

  echo "== runs as the non-root uid it claims =="
  # `docker top`, not `docker inspect .Config.User`: the latter reads IMAGE config, so a
  # `--user 0` invocation would still pass it.
  for c in smoke-gw smoke-iam; do
    uid="$(docker top "$c" -o user 2>/dev/null | tail -1 | tr -d ' ')"
    [ "$uid" = "65532" ] || { echo "::error::$c runs as ${uid}, expected 65532" >&2; return 1; }
    echo "  $c runs as uid ${uid}"
  done
  echo "SMOKE OK"
}
```

Extend the dispatch at the bottom:

```bash
case "$cmd" in
  build) assert_pins; for s in "${services[@]}"; do build_one "$s"; done ;;
  smoke) smoke ;;
  all)   assert_pins; for s in "${services[@]}"; do build_one "$s"; done; smoke ;;
  *) echo "unknown command: $cmd" >&2; exit 1 ;;
esac
```

- [ ] **Step 2: Run the full suite locally**

```bash
./ci/images/run.sh all 2>&1 | tail -30
```

Expected, ending in `SMOKE OK`: gateway healthy with `/healthz` 200 and `/readyz` **503**; IAM healthy with both 200 (proving DNS to `smoke-pg` and migrations); both images shell-less with ≥100 CA certs and under the size ceiling; both running as uid 65532.

If IAM never becomes healthy, read `docker logs smoke-iam` — a `IAM_AUTHN__ISSUERS` parse failure is a config problem, not an image problem.

- [ ] **Step 3: Prove the negative case is real**

The suite must fail when the thing it asserts is false:

```bash
# Point the gateway at an IAM that does not exist and assert /readyz still reports 503 —
# already covered. Instead prove the shell check bites, using a base that HAS one:
docker build -q -t paigasus-fake:dev - <<'EOF'
FROM ubuntu:24.04
RUN useradd -u 65532 svc
USER 65532:65532
ENTRYPOINT ["/bin/sleep"]
EOF
docker run --rm --entrypoint /bin/sh paigasus-fake:dev -c true && echo "control: a shell-ful image is detectable (assert_base_intact would reject it)"
docker rmi -f paigasus-fake:dev
```

Expected: the control prints, confirming `assert_base_intact`'s shell probe distinguishes the two.

- [ ] **Step 4: Commit**

```bash
git add ci/images/run.sh
git commit -m "feat(repo): smoke-test the service images end to end (SMA-500)"
```

---

### Task 7: The workflow and Dependabot

**Files:**
- Create: `.github/workflows/images.yml`
- Modify: `.github/dependabot.yml`

**Interfaces:**
- Consumes: `ci/images/run.sh all` from Tasks 5-6.
- Produces: CI coverage (AC-4). No later task depends on it.

- [ ] **Step 1: Write the workflow**

`branches:` and `paths:` MUST be block sequences — `repo:actionlint` fails all four keys loudly on the inline `[main]` form. Every wildcard-free `paths:` entry must be an exactly-tracked file, which is why the Dockerfile and `.dockerignore` were committed in Task 4.

```yaml
# SPDX-License-Identifier: Apache-2.0
name: images

on:
  workflow_dispatch:

  # POST-merge verification. `rs/**` because any service or dependency change can break an
  # image build, and this is the only place that is checked.
  push:
    branches:
      - main
    paths:
      - 'rs/**'
      - 'ci/images/**'
      - '.github/workflows/images.yml'

  # PRE-merge verification of the BUILD INPUTS only. `rs/**` is deliberately absent: most PRs
  # here touch it, and two cold --release builds on every one of them would raise the bill
  # (SMA-520). These are the inputs that can actually break an image build — a new dependency,
  # a toolchain bump, or the build machinery itself.
  pull_request:
    branches:
      - main
    paths:
      - 'rs/Cargo.lock'
      - 'rs/Cargo.toml'
      - 'rs/rust-toolchain.toml'
      - 'rs/Dockerfile'
      - 'rs/.dockerignore'
      - 'ci/images/**'
      - '.github/workflows/images.yml'

# Build-and-verify only: no registry, no credentials, nothing is pushed (SMA-500 D1).
permissions:
  contents: read

concurrency:
  # `event_name` is in the GROUP so a manual dispatch cannot cancel a running push job.
  group: images-${{ github.workflow }}-${{ github.ref }}-${{ github.event_name }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

jobs:
  images:
    name: build + smoke
    runs-on: ubuntu-latest
    # Two cold --release builds of a tree that has already overflowed the runner disk once.
    # Deliberately ONE job, not a per-service matrix: the smoke suite needs BOTH images present,
    # and matrix legs run on separate runners with no shared image store — a matrix would have to
    # rebuild the other service inside one leg to smoke anything.
    timeout-minutes: 60
    steps:
      # Same reclaim as ci.yml: this builds the cedar-policy tree in --release on a ~14 GB disk.
      - name: Reclaim runner disk (drop unused preinstalled toolchains)
        run: |
          df -h /
          sudo rm -rf /usr/local/lib/android /usr/share/dotnet /opt/ghc /usr/local/.ghcup \
            /opt/hostedtoolcache/CodeQL || true
          df -h /

      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          persist-credentials: false

      - name: Set up Buildx
        uses: docker/setup-buildx-action@e468171a9de216ec08956ac3ada2f0791b6bd435  # v3.11.1

      - name: Build + smoke both images
        run: ci/images/run.sh all

      - name: Upload chisel package manifests
        if: always()
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: chisel-manifests
          path: chisel-manifest-*.txt
          if-no-files-found: ignore
          retention-days: 14
```

- [ ] **Step 2: Add the docker ecosystem to Dependabot**

Append to `.github/dependabot.yml`, matching the indentation and schedule style of the existing four entries:

```yaml
  # SMA-500: the container images pin `rust:` and `ubuntu:` by digest. A pinned-and-never-updated
  # base on a security-audited product is the mirror image of the floating-tag risk the design
  # rejected Chainguard over, so the pins are kept fresh here. NOTE the chisel version and its two
  # sha384 checksums in rs/Dockerfile are NOT covered by any updater and are bumped by hand.
  - package-ecosystem: docker
    directory: /rs
    schedule:
      interval: weekly
```

- [ ] **Step 3: Lint the workflow through the repo's own gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint 2>&1 | tail -25
```

Expected: PASS. A failure naming `branches`/`paths` means a filter was written inline instead of as a block sequence, or a wildcard-free path entry names a file that is not tracked — commit the file first.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/images.yml .github/dependabot.yml
git commit -m "ci(repo): build and smoke-test the service images in CI (SMA-500)"
```

---

### Task 8: Documentation

**Files:**
- Create: `docs/ops/RUNBOOK-containers.md`
- Modify: `CLAUDE.md`

**Interfaces:**
- Consumes: everything above.
- Produces: the probe/env/naming contract SMA-513 inherits.

- [ ] **Step 1: Write the runbook**

Create `docs/ops/RUNBOOK-containers.md` covering exactly these sections — this is the contract SMA-513 reads, so nothing here may be vague:

1. **Build locally** — `ci/images/run.sh all`, and the single-service form.
2. **Image names** — `ghcr.io/paigasus/paigasus-{iam,gateway}:<git-sha>`. Publishing is deferred; the names are fixed so the Helm chart has something to inherit.
3. **Runtime configuration** — env only: defaults `<` optional TOML `<` `IAM_*`/`GATEWAY_*`, with `__` for nesting (`IAM_API_KEYS__PEPPER` -> `api_keys.pepper`). Nothing is baked. A mounted TOML still layers, because that is figment's behaviour.
4. **Probe contract** — the table from spec §4.6: liveness `GET /healthz`, readiness `GET /readyz`, startup `GET /healthz` with a generous `failureThreshold` because IAM migrates before binding. Commands use absolute paths and the exec form (no shell). `/readyz` via `/usr/local/bin/paigasus-service healthcheck --path /readyz`. Exit codes 0/1/2.
5. **Operational rules that are NOT image properties but bite operators first:**
   - IAM runs `Migrator::up` on every start with no advisory lock — migrate with a single replica (`replicas: 1`, `strategy.rollingUpdate.maxSurge: 0`) or a pre-install Job.
   - Gateway `/readyz` issues a real gRPC introspect to IAM per poll, and both its health routes are metered — keep `periodSeconds` at 30s or above and filter `route!~"/healthz|/readyz"` on dashboards.
   - Neither service terminates TLS; both require a TLS-terminating ingress.
   - A private-CA IdP is **not** supported: IAM's reqwest path carries compiled-in webpki roots, so mounting a CA does not help.
6. **Conventions the console images must follow** — chiseled/distroless base with no shell, numeric non-root `USER`, a self-contained probe entrypoint plus a `HEALTHCHECK` that uses it, runtime env config only, digest-pinned bases covered by Dependabot.

- [ ] **Step 2: Add the CLAUDE.md gotchas**

Append these to the `## Gotchas` section — each records something that cost real time to discover:

```markdown
- Container images (SMA-500) live behind `ci/images/run.sh {build,smoke,all}` and
  `.github/workflows/images.yml`, **not** Moon — a `repo:*` task would have to join `ci.yml`'s
  `T=(…)` array (a `--release` build on every affected PR) or become a `T_EXEMPT` entry. The
  workflow is **not a required check**, so a broken image build reds `main`, not the PR;
  `workflow_dispatch` it on the branch before merging anything that touches `rs/Dockerfile`.
- The runtime base is a `chisel cut` of Ubuntu 24.04 into `FROM scratch`. Four traps, all
  measured: `libgcc-s1_libs` is REQUIRED (Rust panic unwinding links `libgcc_s.so.1`) and its
  absence fails at container START, not build; `ca-certificates_data` is the right variant
  (`-with-certs` adds ~120 PEMs nothing reads); there is **no `/etc/passwd`**, so `USER` must be
  numeric; and `chisel cut --root DIR` does not create `DIR`. `/etc/nsswitch.conf` is also absent
  and that is FINE — glibc falls back to a compiled-in `files dns` default and the NSS modules
  ship in `libc6_libs` (DNS verified end-to-end for a container hostname and a public name).
- `FROM rust:X.Y.Z` does **not** pin the compiler: `rust-toolchain.toml` is inside the build
  context and rustup honours it over the image, so a channel bump silently changes the compiler
  behind a pinned-looking `FROM`. `rs/Dockerfile` sets `RUSTUP_TOOLCHAIN` and
  `ci/images/run.sh` asserts the two agree. The related invariant — builder glibc ≤ runtime
  glibc (bookworm 2.36 ≤ noble 2.39) — is also asserted there; inverting it fails at container
  start with `GLIBC_2.4x not found`.
- Exec-form `ENTRYPOINT`/`HEALTHCHECK` do **not** expand `ARG`/`ENV`, which is why one
  parameterized `rs/Dockerfile` installs both binaries to the fixed path
  `/usr/local/bin/paigasus-service`. Service identity comes from `paigasus_logging::init`, not
  `argv[0]`.
```

- [ ] **Step 3: Commit**

```bash
git add docs/ops/RUNBOOK-containers.md CLAUDE.md
git commit -m "docs(repo): document the container build and probe contract (SMA-500)"
```

---

### Task 9: Full-graph verification

**Files:** none — this task verifies, it does not change behaviour.

- [ ] **Step 1: Run the full CI graph exactly as CI does**

Per-project Moon tasks do NOT run the repo-level gates. Run the whole thing:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-500
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :promtool :observability-drift :nats-permissions :release-parity :release-parity-py \
  :release-parity-ts :publish-metadata --base origin/main --include-relations 2>&1 | tail -40
```

Expected: green. Three gates re-key and re-run, and none should fail:

| Gate | Why |
| --- | --- |
| `repo:observability-drift` | `inputs: rs/crates/libs/paigasus-observability/**/*` — the new `health.rs` |
| `repo:error-code-single-site` + `repo:machete` | `rs/crates/**/src/**/*.rs` and `rs/**/*.rs` — the two `main.rs` edits |
| `repo:actionlint` | `inputs: ['**/*']`, and it lints the new workflow |

`repo:publish-metadata` (`inputs: rs/crates/**/*`) does **not** re-key: D9 put the Dockerfile at
`rs/Dockerfile`, outside `rs/crates/`. The spec's § 2.7 table predates that decision.

- [ ] **Step 2: Diagnose any failure precisely**

`moon ci` reports an unattributed "1 failed". Find the actual task:

```bash
jq '.actions[] | select(.status=="failed") | .label' .moon/cache/ciReport.json
```

- [ ] **Step 3: Confirm the working tree is clean and the branch is pushed**

```bash
git status --short
git log --oneline origin/main..HEAD
```

Expected: no untracked or modified files (the `chisel-manifest-*.txt` artifacts are gitignored), and eight commits from Tasks 1-8 plus the two spec commits.
