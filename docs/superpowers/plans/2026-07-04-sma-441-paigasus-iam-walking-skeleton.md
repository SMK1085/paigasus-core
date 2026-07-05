# paigasus-iam M0 Walking Skeleton — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up a running, hexagonal `paigasus-iam` service skeleton — HTTP + gRPC health, a SeaORM/Postgres `Principal`/`User` round-trip, structured JSON logs, config, an `iam.proto` wired through buf, and green Moon CI.

**Architecture:** Hexagonal. A pure-domain libs crate (`paigasus-iam-core`) holds entities, value objects, and port traits; the service crate (`paigasus-iam`) provides adapters (http/grpc/persistence/id/clock) and a composition-root `main`. A shared `paigasus-logging` crate installs JSON tracing. The wire model lives in `contracts/`. IDs and PRNs come from `paigasus-kernel`.

**Tech Stack:** Rust (edition 2024, rustc 1.95), axum 0.8, tonic 0.14 + tonic-health, tower/tower-http, SeaORM 1 + sea-orm-migration (Postgres, rustls/ring), figment, chrono, uuid, rand, tracing/tracing-subscriber; testcontainers for the DB integration test; Moon for the task graph; buf for proto codegen.

## Global Constraints

Copied verbatim from the spec and repo conventions — every task implicitly includes these:

- **Rust edition `2024`, `rust-version 1.95`** on every new crate (`edition.workspace = true`, `rust-version.workspace = true`). Applies even though this predates the ID.
- **SPDX header on every source file**: `// SPDX-Identifier` line — exactly `// SPDX-License-Identifier: Apache-2.0` as the first line of every `.rs` and the proto (`// SPDX-License-Identifier: Apache-2.0` above `syntax = "proto3";`).
- **Never name a source file a Windows-reserved device name** (`con/prn/aux/nul/com1-9/lpt1-9`). None here are, but hold the rule.
- **Moon project id gets the `-rs` suffix** (`paigasus-iam-rs`, etc.) and a `layer:` field (`library` for libs, `application` for the service). `layer:`, not `type:`.
- **`cargo nextest` with `--no-tests=pass`** (the Moon `test` task already passes this) — a crate with no tests must not red.
- **Lint posture:** `#![lints] workspace = true` in every crate `Cargo.toml`; clippy runs `--all-targets -- -D warnings`; rustc `warnings = "deny"`. Generated proto code is excluded via `#![allow(clippy::all, warnings)]` at its `include!` site.
- **`uuid` carries NO rng feature; `getrandom` must not enter the wasm tree.** The service supplies entropy via `rand` (native-only; the service is not wasm-bound, so `wasm-getrandom-free` is untouched). Never add a `uuid` `v4`/`v7`/`rng` feature to the workspace.
- **TLS posture is rustls, never openssl/native-tls** (`sea-orm` uses `runtime-tokio-rustls` with the `ring` backend).
- **Generated proto code is committed and drift-gated** — after any `iam.proto` edit, run `moon run contracts:generate` and commit the rs/py/ts output.
- **Conventional Commits with a workspace scope** (`feat(rs): …`, `feat(contracts): …`). Do NOT put a bare `#NNN` issue ref in the commit body/footer (commitlint `footer-leading-blank`); reference `SMA-441` in the subject only.
- **Branch:** `feature/sma-441-m0-walking-skeleton-paigasus-iam-scaffold` (already checked out in the worktree).

## Environment setup (do once before Task 1)

The Moon/buf/uv/pnpm CLIs are proto-managed and off the default PATH. In every shell:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"   # shims FIRST (repo-pinned versions)
cd rs && cargo --version        # sanity; toolchain 1.95 auto-selected via rust-toolchain.toml
```

Install the JS toolchain so the lefthook `commit-msg` (commitlint) hook and codegen work:

```bash
proto install
pnpm --dir ts install --frozen-lockfile
```

If a commit's `commit-msg` hook still can't find `commitlint` in this fresh worktree, the message is CI-gated regardless — commit with `--no-verify` only as a fallback, keeping the subject Conventional-Commits-clean.

---

### Task 1: `paigasus-logging` shared crate

**Files:**
- Create: `rs/crates/libs/paigasus-logging/Cargo.toml`
- Create: `rs/crates/libs/paigasus-logging/moon.yml`
- Create: `rs/crates/libs/paigasus-logging/src/lib.rs`
- Modify: `rs/Cargo.toml` (add `tracing-subscriber` `json` feature)

**Interfaces:**
- Produces: `paigasus_logging::init(service: &str)` — installs a global JSON `tracing-subscriber`; `paigasus_logging::env_filter() -> tracing_subscriber::EnvFilter` (pure, testable helper defaulting to `info`).

- [ ] **Step 1: Add the `json` feature to the workspace `tracing-subscriber`**

In `rs/Cargo.toml`, change the line to:

```toml
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
```

- [ ] **Step 2: Write `Cargo.toml`**

```toml
[package]
name = "paigasus-logging"
version = "0.0.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
publish = false

[dependencies]
tracing.workspace = true
tracing-subscriber.workspace = true

[lints]
workspace = true
```

- [ ] **Step 3: Write `moon.yml`**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-logging-rs'
layer: 'library'
language: 'rust'
```

- [ ] **Step 4: Write the failing test + implementation in `src/lib.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! Structured JSON logging conventions shared by every Paigasus service.
//!
//! `init` installs a global JSON `tracing-subscriber` honoring `RUST_LOG`
//! (defaulting to `info`). Kept tiny and dependency-light so every service
//! shares one log shape (ADR-0005-adjacent; the first consumer is `paigasus-iam`).

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// The env-filter for logging: `RUST_LOG` if set, else `info`. Pure so it is unit-testable
/// without touching the process-global subscriber.
#[must_use]
pub fn env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Install the global JSON tracing subscriber for `service`. Call once at process start;
/// a second call is a no-op-with-error (the global subscriber is already set).
pub fn init(service: &str) {
    let _ = tracing_subscriber::registry()
        .with(env_filter())
        .with(fmt::layer().json().with_current_span(true).with_span_list(true))
        .try_init();
    tracing::info!(service, "logging initialized");
}

#[cfg(test)]
mod tests {
    use super::env_filter;

    #[test]
    fn env_filter_defaults_to_info_without_rust_log() {
        // SAFETY: single-threaded test; we remove RUST_LOG so the default branch runs.
        unsafe { std::env::remove_var("RUST_LOG") };
        assert_eq!(env_filter().to_string(), "info");
    }
}
```

- [ ] **Step 5: Verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-logging-rs:test paigasus-logging-rs:lint paigasus-logging-rs:fmt
```
Expected: all PASS (1 test).

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-logging rs/Cargo.toml rs/Cargo.lock
git commit -m "feat(rs): add paigasus-logging JSON tracing crate (SMA-441)"
```

---

### Task 2: `iam.proto` + buf codegen + `paigasus-proto` wiring

**Files:**
- Create: `contracts/proto/paigasus/iam/v1/iam.proto`
- Modify: `rs/crates/libs/paigasus-proto/src/lib.rs` (add `paigasus::iam::v1`, prost-only)
- Generated (committed): `rs/crates/libs/paigasus-proto/src/generated/paigasus/iam/v1/…`, `py/packages/paigasus-proto/src/paigasus_proto/generated/…`, `ts/packages/paigasus-proto/src/generated/…`

**Interfaces:**
- Produces: `paigasus_proto::paigasus::iam::v1` module (compiled by the proto crate; exercises the iam codegen path). Not consumed by the service in M0.

- [ ] **Step 1: Write `iam.proto`** (placeholder message + reserved-name comments; NO service, so no `.tonic.rs` is emitted)

```proto
// SPDX-License-Identifier: Apache-2.0
syntax = "proto3";

package paigasus.iam.v1;

// IAM v1 wire model — SCAFFOLD ONLY (SMA-441, M0 walking skeleton).
//
// M0 defines no RPCs: the service's health surface is the well-known
// grpc.health.v1.Health (served via tonic-health), not an IAM RPC. This file
// exists to establish the paigasus.iam.v1 package and exercise the buf codegen
// path end-to-end (prost / betterproto2 / protobuf-es).
//
// Reserved for later milestones (do not repurpose without an ADR):
//   service AuthorizationService { rpc IsAuthorized(...); rpc Introspect(...); }  // M4/M5
//   messages: Principal, User, Organization, Team, Project, ApiKey, Policy, ...   // M1+

// Placeholder so the package generates a concrete type in all three languages.
// Replaced by real messages in M1; carries a service PRN string for now.
message ServiceInfo {
  string prn = 1;
}
```

- [ ] **Step 2: Format + lint the proto**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run contracts:fmt contracts:lint
```
Expected: PASS (STANDARD lint; `PACKAGE_DIRECTORY_MATCH` is excepted in `buf.yaml`).

- [ ] **Step 3: Regenerate all bindings**

```bash
moon run contracts:generate
```
Expected: new files under all three `generated/paigasus/iam/v1/` dirs. Confirm **no** `paigasus.iam.v1.tonic.rs` was produced (serviceless package):

```bash
ls rs/crates/libs/paigasus-proto/src/generated/paigasus/iam/v1/
```
Expected: only `paigasus.iam.v1.rs` (prost). If a `.tonic.rs` exists, the proto accidentally declared a service — remove it.

- [ ] **Step 4: Wire the module into `paigasus-proto/src/lib.rs`** — mirror the `common/v1` block (prost `.rs` ONLY), NOT the `gateway/v1` block.

Add inside `pub mod paigasus { … }`, after the `common` module:

```rust
    pub mod iam {
        pub mod v1 {
            // Generated code is excluded from the strict lint gate.
            #![allow(clippy::all, warnings)]
            // Only the prost file: iam.proto (M0) declares no service, so
            // neoeinstein-tonic emits no `.tonic.rs` for this package.
            include!("generated/paigasus/iam/v1/paigasus.iam.v1.rs");
        }
    }
```

- [ ] **Step 5: Verify the proto crate compiles + drift gate is clean**

```bash
moon run paigasus-proto-rs:build paigasus-proto-rs:lint
git add --intent-to-add rs/crates/libs/paigasus-proto/src/generated py/packages/paigasus-proto/src/paigasus_proto/generated ts/packages/paigasus-proto/src/generated
git diff --exit-code -- rs/crates/libs/paigasus-proto/src/generated py/packages/paigasus-proto/src/paigasus_proto/generated ts/packages/paigasus-proto/src/generated && echo "no drift" || echo "commit the generated files"
```
Expected: proto crate builds; the generated files are the committed artifact.

- [ ] **Step 6: Commit**

```bash
git add contracts/proto/paigasus/iam rs/crates/libs/paigasus-proto/src \
        py/packages/paigasus-proto/src/paigasus_proto/generated \
        ts/packages/paigasus-proto/src/generated rs/Cargo.lock
git commit -m "feat(contracts): scaffold paigasus.iam.v1 proto + wire codegen (SMA-441)"
```

---

### Task 3: `paigasus-iam-core` value objects (`Email`, `PrincipalId`, `DomainError`)

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/Cargo.toml`
- Create: `rs/crates/libs/paigasus-iam-core/moon.yml`
- Create: `rs/crates/libs/paigasus-iam-core/src/lib.rs`
- Create: `rs/crates/libs/paigasus-iam-core/src/value.rs`

**Interfaces:**
- Produces:
  - `Email::parse(&str) -> Result<Email, DomainError>`; `Email::as_str(&self) -> &str`
  - `PrincipalId::from_prn(Prn) -> PrincipalId`; `PrincipalId::uuid(&self) -> uuid::Uuid`; `PrincipalId::prn(&self) -> &Prn`; `PrincipalId::canonical(&self) -> String`
  - `#[derive(Debug, thiserror::Error)] pub enum DomainError { InvalidEmail(String) }`

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "paigasus-iam-core"
version = "0.0.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
publish = false

[dependencies]
paigasus-kernel = { workspace = true }
uuid = { workspace = true }
chrono = { workspace = true }
thiserror = { workspace = true }
async-trait = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 2: Write `moon.yml`**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-iam-core-rs'
layer: 'library'
language: 'rust'

dependsOn:
  - 'paigasus-kernel-rs'

tasks:
  build:
    deps: ['^:build']
  test:
    deps: ['^:build']
```

- [ ] **Step 3: Write `src/lib.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! Pure IAM domain (M0 walking skeleton): entities, value objects, and port traits.
//! No I/O, no SeaORM, no axum/tonic — the service crate provides adapters (ADR-0005,
//! hexagonal). IDs/PRNs come from `paigasus-kernel`; time/entropy are injected via ports.

pub mod value;

pub use value::{DomainError, Email, PrincipalId};
```

- [ ] **Step 4: Write the failing tests + implementation in `src/value.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! Domain value objects: `Email` and `PrincipalId`.

use paigasus_kernel::Prn;
use uuid::Uuid;

/// A domain-validation error (invalid value object input).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("invalid email: {0}")]
    InvalidEmail(String),
}

/// A validated email address. M0 rule: non-empty, exactly one `@`, non-empty local
/// and domain parts. Deliberately minimal — full RFC 5322 is out of scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email(String);

impl Email {
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let s = raw.trim();
        let bad = |r: &str| DomainError::InvalidEmail(r.to_string());
        if s.is_empty() {
            return Err(bad(raw));
        }
        let (local, domain) = s.split_once('@').ok_or_else(|| bad(raw))?;
        if local.is_empty() || domain.is_empty() || domain.contains('@') {
            return Err(bad(raw));
        }
        Ok(Email(s.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A principal's stable identity: its PRN (`prn:pgs:iam:::principal/<uuidv7>`). The UUID
/// (the PK/FK) is derived from the PRN's resource-id — stored once, never duplicated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrincipalId(Prn);

impl PrincipalId {
    #[must_use]
    pub fn from_prn(prn: Prn) -> Self {
        PrincipalId(prn)
    }

    #[must_use]
    pub fn uuid(&self) -> Uuid {
        self.0.resource_id()
    }

    #[must_use]
    pub fn prn(&self) -> &Prn {
        &self.0
    }

    #[must_use]
    pub fn canonical(&self) -> String {
        self.0.canonical()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_accepts_a_simple_address() {
        assert_eq!(Email::parse("  a@b.com ").unwrap().as_str(), "a@b.com");
    }

    #[test]
    fn email_rejects_empty_missing_at_and_empty_parts() {
        for bad in ["", "  ", "nope", "@b.com", "a@", "a@@b", "a b"] {
            assert!(Email::parse(bad).is_err(), "expected {bad:?} to be rejected");
        }
    }

    #[test]
    fn principal_id_derives_uuid_and_canonical_from_prn() {
        let uuid = Uuid::parse_str("0192f1c0-0000-7000-8000-000000000000").unwrap();
        let prn = Prn::build("iam", "", None, "principal", uuid).unwrap();
        let id = PrincipalId::from_prn(prn);
        assert_eq!(id.uuid(), uuid);
        assert_eq!(id.canonical(), format!("prn:pgs:iam:::principal/{uuid}"));
    }
}
```

Note: `"a b"` is rejected because it has no `@` (splits to `Err`), and `"a@@b"` because `domain` (`"@b"`) contains `@`.

- [ ] **Step 5: Verify**

```bash
moon run paigasus-iam-core-rs:test paigasus-iam-core-rs:lint paigasus-iam-core-rs:fmt
```
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core rs/Cargo.lock
git commit -m "feat(rs): add paigasus-iam-core value objects Email + PrincipalId (SMA-441)"
```

---

### Task 4: `paigasus-iam-core` entities + ports

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/src/principal.rs`
- Create: `rs/crates/libs/paigasus-iam-core/src/user.rs`
- Create: `rs/crates/libs/paigasus-iam-core/src/ports.rs`
- Modify: `rs/crates/libs/paigasus-iam-core/src/lib.rs` (re-exports)

**Interfaces:**
- Produces:
  - `PrincipalKind::User`, `PrincipalStatus::Active` (both `#[non_exhaustive]`-free but one-variant for M0); `.as_str()`/`FromStr`-style string mapping helpers.
  - `Principal { id: PrincipalId, kind, status, created_at: DateTime<Utc>, updated_at: DateTime<Utc> }` + `Principal::new(...)`
  - `User { principal_id: PrincipalId, email: Email, display_name: String, locale: Option<String>, timezone: Option<String>, created_at, updated_at }` + `User::new(...)`
  - `RepositoryError { Conflict(String), Backend(Box<dyn Error + Send + Sync>) }`
  - `#[async_trait] PrincipalRepository { create_user, find_user }`; `IdGenerator { new_principal_id }`; `Clock { now }`

- [ ] **Step 1: Write `src/principal.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! The `Principal` root entity and its kind/status value enums.

use crate::value::PrincipalId;
use chrono::{DateTime, Utc};

/// Principal subtype. M0 mints only `User`; `ServiceAccount` arrives in a later milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalKind {
    User,
}

impl PrincipalKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            PrincipalKind::User => "user",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(PrincipalKind::User),
            _ => None,
        }
    }
}

/// Principal lifecycle status. M0 only ever `Active`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalStatus {
    Active,
}

impl PrincipalStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            PrincipalStatus::Active => "active",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(PrincipalStatus::Active),
            _ => None,
        }
    }
}

/// The root identity. In M0 every principal is a `User` (see `crate::user::User`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub id: PrincipalId,
    pub kind: PrincipalKind,
    pub status: PrincipalStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Principal {
    #[must_use]
    pub fn new(
        id: PrincipalId,
        kind: PrincipalKind,
        status: PrincipalStatus,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> Self {
        Principal { id, kind, status, created_at, updated_at }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_and_status_round_trip_through_strings() {
        assert_eq!(PrincipalKind::parse(PrincipalKind::User.as_str()), Some(PrincipalKind::User));
        assert_eq!(PrincipalStatus::parse(PrincipalStatus::Active.as_str()), Some(PrincipalStatus::Active));
        assert_eq!(PrincipalKind::parse("nope"), None);
    }
}
```

- [ ] **Step 2: Write `src/user.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! The `User` entity — the human profile sharing a `Principal`'s identity (1:1).

use crate::value::{Email, PrincipalId};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub principal_id: PrincipalId,
    pub email: Email,
    pub display_name: String,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl User {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        principal_id: PrincipalId,
        email: Email,
        display_name: String,
        locale: Option<String>,
        timezone: Option<String>,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> Self {
        User { principal_id, email, display_name, locale, timezone, created_at, updated_at }
    }
}
```

- [ ] **Step 3: Write `src/ports.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! Hexagonal ports (traits) the service's adapters implement. Kept in the pure core so
//! use cases depend on abstractions, not on SeaORM/axum (ADR-0005).

use crate::principal::Principal;
use crate::user::User;
use crate::value::PrincipalId;
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Persistence errors, source-preserving. The adapter maps its backend error (e.g. SeaORM
/// `DbErr`) into these; the core never imports the backend.
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("conflict: {0}")]
    Conflict(String),
    #[error(transparent)]
    Backend(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// Persistence port for user-principals.
#[async_trait]
pub trait PrincipalRepository: Send + Sync {
    async fn create_user(&self, principal: &Principal, user: &User) -> Result<(), RepositoryError>;
    async fn find_user(
        &self,
        id: &PrincipalId,
    ) -> Result<Option<(Principal, User)>, RepositoryError>;
}

/// Mints new principal identities (UUIDv7 + PRN). Impure (clock + entropy) — hence a port.
pub trait IdGenerator: Send + Sync {
    fn new_principal_id(&self) -> PrincipalId;
}

/// A source of the current time, truncated to microseconds so values round-trip through
/// Postgres `TIMESTAMPTZ` (µs resolution) bit-for-bit.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
```

- [ ] **Step 4: Update `src/lib.rs` re-exports**

```rust
// SPDX-License-Identifier: Apache-2.0

//! Pure IAM domain (M0 walking skeleton): entities, value objects, and port traits.
//! No I/O, no SeaORM, no axum/tonic — the service crate provides adapters (ADR-0005,
//! hexagonal). IDs/PRNs come from `paigasus-kernel`; time/entropy are injected via ports.

pub mod ports;
pub mod principal;
pub mod user;
pub mod value;

pub use ports::{Clock, IdGenerator, PrincipalRepository, RepositoryError};
pub use principal::{Principal, PrincipalKind, PrincipalStatus};
pub use user::User;
pub use value::{DomainError, Email, PrincipalId};
```

- [ ] **Step 5: Add an object-safety + construction test** — append to `src/ports.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Compile-time proof the repository port is object-safe (injected as a trait object).
    #[allow(dead_code)]
    fn assert_object_safe(_: &dyn PrincipalRepository) {}

    #[test]
    fn repository_error_wraps_a_source_error() {
        let e: RepositoryError =
            Box::<dyn std::error::Error + Send + Sync>::from("boom").into();
        assert!(matches!(e, RepositoryError::Backend(_)));
    }
}
```

- [ ] **Step 6: Verify**

```bash
moon run paigasus-iam-core-rs:test paigasus-iam-core-rs:lint paigasus-iam-core-rs:fmt
```
Expected: PASS (kind/status round-trip + repository-error + object-safety).

- [ ] **Step 7: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core
git commit -m "feat(rs): add paigasus-iam-core entities + hexagonal ports (SMA-441)"
```

---

### Task 5: `paigasus-iam` scaffold + figment config

**Files:**
- Create: `rs/crates/services/paigasus-iam/Cargo.toml`
- Create: `rs/crates/services/paigasus-iam/moon.yml`
- Create: `rs/crates/services/paigasus-iam/src/main.rs` (temporary stub; real wiring in Task 11)
- Create: `rs/crates/services/paigasus-iam/src/config.rs`
- Create: `rs/crates/services/paigasus-iam/iam.toml.example`
- Modify: `rs/Cargo.toml` (add `figment`, `anyhow`, extend `tokio` features)

**Interfaces:**
- Produces: `config::IamConfig { http_addr: SocketAddr, grpc_addr: SocketAddr, database_url: String, log_level: String }`; `IamConfig::figment() -> Figment`; `IamConfig::load() -> Result<IamConfig, figment::Error>`.

- [ ] **Step 1: Add workspace deps** — in `rs/Cargo.toml` `[workspace.dependencies]`:

```toml
figment = { version = "0.10", features = ["toml", "env"] }
```

Extend the `tokio` line's guidance is per-crate; the service enables features in its own manifest (Step 2). `anyhow` is already pinned.

- [ ] **Step 2: Write `Cargo.toml`**

```toml
[package]
name = "paigasus-iam"
version = "0.0.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
publish = false

[dependencies]
paigasus-iam-core = { path = "../../libs/paigasus-iam-core", version = "0.0.0" }
paigasus-logging = { path = "../../libs/paigasus-logging", version = "0.0.0" }
paigasus-kernel = { workspace = true }
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "net", "signal", "time", "sync"] }
figment = { workspace = true }
serde = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }

[lints]
workspace = true
```

(Later tasks add axum/tonic/sea-orm/etc. to this manifest as their consumers land.)

- [ ] **Step 3: Write `moon.yml`**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-iam-rs'
layer: 'application'
language: 'rust'

dependsOn:
  - 'paigasus-iam-core-rs'
  - 'paigasus-logging-rs'
  - 'paigasus-kernel-rs'

tasks:
  build:
    deps: ['^:build']
  test:
    deps: ['^:build']
```

- [ ] **Step 4: Write `iam.toml.example`**

```toml
# paigasus-iam config. Copy to iam.toml or supply via IAM_* env vars.
# Precedence: built-in defaults < iam.toml < IAM_* environment.

# http_addr = "0.0.0.0:8080"     # axum HTTP (health) — default shown
# grpc_addr = "0.0.0.0:9090"     # tonic gRPC (grpc.health.v1) — default shown
# log_level = "info"             # RUST_LOG-style filter — default shown

# REQUIRED — no default. Example:
# database_url = "postgres://postgres:postgres@127.0.0.1:5432/paigasus_iam"
#   or:  IAM_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/paigasus_iam
```

- [ ] **Step 5: Write the failing test + implementation in `src/config.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! Service configuration via figment: built-in defaults < `iam.toml` < `IAM_*` env.

use figment::providers::{Env, Format, Serialized, Toml};
use figment::{Figment, error::Error as FigmentError};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct IamConfig {
    pub http_addr: SocketAddr,
    pub grpc_addr: SocketAddr,
    pub database_url: String,
    pub log_level: String,
}

// Only the fields that HAVE a default. `database_url` is intentionally absent so a
// missing value is a hard error at load time.
#[derive(Serialize)]
struct Defaults {
    http_addr: SocketAddr,
    grpc_addr: SocketAddr,
    log_level: String,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults {
            http_addr: "0.0.0.0:8080".parse().expect("valid addr"),
            grpc_addr: "0.0.0.0:9090".parse().expect("valid addr"),
            log_level: "info".to_string(),
        }
    }
}

impl IamConfig {
    #[must_use]
    pub fn figment() -> Figment {
        Figment::from(Serialized::defaults(Defaults::default()))
            .merge(Toml::file("iam.toml"))
            .merge(Env::prefixed("IAM_"))
    }

    pub fn load() -> Result<Self, FigmentError> {
        Self::figment().extract()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_url_from_env_with_defaults() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert_eq!(cfg.database_url, "postgres://u:p@localhost/db");
            assert_eq!(cfg.http_addr.to_string(), "0.0.0.0:8080");
            assert_eq!(cfg.log_level, "info");
            Ok(())
        });
    }

    #[test]
    fn missing_database_url_is_an_error() {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            let result = IamConfig::figment().extract::<IamConfig>();
            assert!(result.is_err(), "expected missing database_url to error");
            Ok(())
        });
    }
}
```

- [ ] **Step 6: Write the temporary `src/main.rs` stub** (replaced in Task 11 — keeps the crate a valid binary now)

```rust
// SPDX-License-Identifier: Apache-2.0

//! paigasus-iam service entrypoint. Full composition root lands in SMA-441 Task 11.

mod config;

fn main() {
    // Placeholder: real wiring (logging, DB, servers, shutdown) added in Task 11.
    let _ = config::IamConfig::figment();
}
```

- [ ] **Step 7: Verify**

```bash
moon run paigasus-iam-rs:test paigasus-iam-rs:lint paigasus-iam-rs:fmt
```
Expected: PASS (2 config tests).

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam rs/Cargo.toml rs/Cargo.lock
git commit -m "feat(rs): scaffold paigasus-iam service + figment config (SMA-441)"
```

---

### Task 6: `CreateUser` use case + in-memory fakes

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/application/mod.rs`
- Create: `rs/crates/services/paigasus-iam/src/application/create_user.rs`
- Modify: `rs/crates/services/paigasus-iam/src/main.rs` (add `mod application;`)

**Interfaces:**
- Consumes: `paigasus_iam_core::{Clock, IdGenerator, PrincipalRepository, Principal, PrincipalKind, PrincipalStatus, User, Email, PrincipalId, RepositoryError}`.
- Produces: `application::create_user::{CreateUser, NewUser, CreateUserError}`; `CreateUser::new(repo, id_gen, clock)`; `async fn execute(&self, cmd: NewUser) -> Result<PrincipalId, CreateUserError>`.

- [ ] **Step 1: Write `src/application/mod.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! Application layer — use cases orchestrating the domain + ports.

pub mod create_user;
```

- [ ] **Step 2: Write the failing test + implementation in `src/application/create_user.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! `CreateUser` use case: mint an identity, build a `User` principal, persist it.

use paigasus_iam_core::{
    Clock, Email, IdGenerator, Principal, PrincipalId, PrincipalKind, PrincipalRepository,
    PrincipalStatus, RepositoryError, User,
};

/// Input to create a user principal.
#[derive(Debug, Clone)]
pub struct NewUser {
    pub email: String,
    pub display_name: String,
    pub locale: Option<String>,
    pub timezone: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateUserError {
    #[error("invalid email: {0}")]
    InvalidEmail(String),
    #[error(transparent)]
    Repository(#[from] RepositoryError),
}

pub struct CreateUser<R, I, C> {
    repo: R,
    id_gen: I,
    clock: C,
}

impl<R, I, C> CreateUser<R, I, C>
where
    R: PrincipalRepository,
    I: IdGenerator,
    C: Clock,
{
    pub fn new(repo: R, id_gen: I, clock: C) -> Self {
        CreateUser { repo, id_gen, clock }
    }

    pub async fn execute(&self, cmd: NewUser) -> Result<PrincipalId, CreateUserError> {
        let email = Email::parse(&cmd.email)
            .map_err(|_| CreateUserError::InvalidEmail(cmd.email.clone()))?;
        let id = self.id_gen.new_principal_id();
        let now = self.clock.now();

        let principal = Principal::new(
            id.clone(),
            PrincipalKind::User,
            PrincipalStatus::Active,
            now,
            now,
        );
        let user = User::new(id.clone(), email, cmd.display_name, cmd.locale, cmd.timezone, now, now);

        self.repo.create_user(&principal, &user).await?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::{DateTime, TimeZone, Utc};
    use paigasus_kernel::Prn;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use uuid::Uuid;

    #[derive(Default)]
    struct InMemoryPrincipalRepository {
        rows: Mutex<HashMap<Uuid, (Principal, User)>>,
    }

    #[async_trait]
    impl PrincipalRepository for InMemoryPrincipalRepository {
        async fn create_user(&self, p: &Principal, u: &User) -> Result<(), RepositoryError> {
            let mut rows = self.rows.lock().unwrap();
            if rows.contains_key(&p.id.uuid()) {
                return Err(RepositoryError::Conflict("duplicate principal".into()));
            }
            rows.insert(p.id.uuid(), (p.clone(), u.clone()));
            Ok(())
        }
        async fn find_user(
            &self,
            id: &PrincipalId,
        ) -> Result<Option<(Principal, User)>, RepositoryError> {
            Ok(self.rows.lock().unwrap().get(&id.uuid()).cloned())
        }
    }

    struct FixedIdGenerator(Uuid);
    impl IdGenerator for FixedIdGenerator {
        fn new_principal_id(&self) -> PrincipalId {
            PrincipalId::from_prn(Prn::build("iam", "", None, "principal", self.0).unwrap())
        }
    }

    struct FixedClock(DateTime<Utc>);
    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    #[tokio::test]
    async fn create_user_persists_and_round_trips_through_the_port() {
        let uuid = Uuid::parse_str("0192f1c0-0000-7000-8000-000000000001").unwrap();
        let clock = FixedClock(Utc.timestamp_opt(1_700_000_000, 0).unwrap());
        let repo = InMemoryPrincipalRepository::default();
        let uc = CreateUser::new(&repo, FixedIdGenerator(uuid), clock);

        let id = uc
            .execute(NewUser {
                email: "alice@example.com".into(),
                display_name: "Alice".into(),
                locale: None,
                timezone: None,
            })
            .await
            .unwrap();

        assert_eq!(id.uuid(), uuid);
        let (p, u) = repo.find_user(&id).await.unwrap().unwrap();
        assert_eq!(p.kind, PrincipalKind::User);
        assert_eq!(p.status, PrincipalStatus::Active);
        assert_eq!(u.email.as_str(), "alice@example.com");
    }

    #[tokio::test]
    async fn create_user_rejects_a_bad_email() {
        let repo = InMemoryPrincipalRepository::default();
        let uc = CreateUser::new(
            &repo,
            FixedIdGenerator(Uuid::nil()),
            FixedClock(Utc.timestamp_opt(0, 0).unwrap()),
        );
        let err = uc
            .execute(NewUser { email: "nope".into(), display_name: "X".into(), locale: None, timezone: None })
            .await
            .unwrap_err();
        assert!(matches!(err, CreateUserError::InvalidEmail(_)));
    }
}
```

Note: the use case takes `R/I/C` by value; `&repo` (a `&T where T: PrincipalRepository`) works because the impl is generic. Add `thiserror` to the service manifest if not already present:

```toml
thiserror = { workspace = true }
```

- [ ] **Step 3: Register the module** — in `src/main.rs`, add `mod application;` above `mod config;`.

- [ ] **Step 4: Verify**

```bash
moon run paigasus-iam-rs:test paigasus-iam-rs:lint paigasus-iam-rs:fmt
```
Expected: PASS (2 new async tests + prior config tests).

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src rs/crates/services/paigasus-iam/Cargo.toml rs/Cargo.lock
git commit -m "feat(rs): add CreateUser use case with in-memory fakes (SMA-441)"
```

---

### Task 7: `KernelIdGenerator` + `SystemClock` adapters

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/mod.rs`
- Create: `rs/crates/services/paigasus-iam/src/adapters/id.rs`
- Create: `rs/crates/services/paigasus-iam/src/adapters/clock.rs`
- Modify: `src/main.rs` (`mod adapters;`), `Cargo.toml` (add `rand`, `chrono`)

**Interfaces:**
- Produces: `adapters::id::KernelIdGenerator` (impl `IdGenerator`); `adapters::clock::SystemClock` (impl `Clock`, µs-truncated).

- [ ] **Step 1: Add workspace/crate deps** — `rand` is workspace-new; add to `rs/Cargo.toml`:

```toml
rand = "0.9"
```

Add to the service `Cargo.toml` `[dependencies]`:

```toml
paigasus-iam-core = { path = "../../libs/paigasus-iam-core", version = "0.0.0" }  # already present
rand = { workspace = true }
chrono = { workspace = true }
uuid = { workspace = true }
```

- [ ] **Step 2: Write `src/adapters/mod.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! Adapters — concrete implementations of the core's ports.

pub mod clock;
pub mod id;
```

- [ ] **Step 3: Write the failing test + implementation in `src/adapters/id.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! `KernelIdGenerator` — mints a UUIDv7 + PRN via `paigasus-kernel`, supplying the host's
//! clock and entropy (the kernel is pure and does neither).

use paigasus_iam_core::{IdGenerator, PrincipalId};
use paigasus_kernel::{Prn, mint_uuid7};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Default, Clone, Copy)]
pub struct KernelIdGenerator;

impl IdGenerator for KernelIdGenerator {
    fn new_principal_id(&self) -> PrincipalId {
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before 1970")
            .as_millis() as u64;
        let rand: [u8; 10] = rand::random();
        let uuid = mint_uuid7(unix_ms, rand);
        // Statically infallible for these fixed, valid inputs (service/type are valid labels,
        // region empty, org none, id a valid UUID).
        let prn = Prn::build("iam", "", None, "principal", uuid).expect("valid IAM principal PRN");
        PrincipalId::from_prn(prn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mints_a_v7_principal_prn() {
        let id = KernelIdGenerator.new_principal_id();
        assert_eq!(id.uuid().get_version_num(), 7);
        let canonical = id.canonical();
        assert!(
            canonical.starts_with("prn:pgs:iam:::principal/"),
            "unexpected PRN: {canonical}"
        );
        // Distinct calls mint distinct ids.
        assert_ne!(KernelIdGenerator.new_principal_id().uuid(), id.uuid());
    }
}
```

- [ ] **Step 4: Write the failing test + implementation in `src/adapters/clock.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! `SystemClock` — wall-clock time truncated to microseconds so timestamps round-trip
//! through Postgres `TIMESTAMPTZ` (µs resolution) without truncation-on-store mismatch.

use chrono::{DateTime, SubsecRound, Utc};
use paigasus_iam_core::Clock;

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now().trunc_subsecs(6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_has_no_sub_microsecond_digits() {
        let t = SystemClock.now();
        assert_eq!(t.timestamp_subsec_nanos() % 1_000, 0);
    }
}
```

- [ ] **Step 5: Register the module** — in `src/main.rs`, add `mod adapters;`.

- [ ] **Step 6: Verify**

```bash
moon run paigasus-iam-rs:test paigasus-iam-rs:lint paigasus-iam-rs:fmt
```
Expected: PASS. Also confirm the wasm gate is unaffected:

```bash
moon run repo:wasm-getrandom-free
```
Expected: PASS (rand is native-only; not in the wasm tree).

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam rs/Cargo.toml rs/Cargo.lock
git commit -m "feat(rs): add KernelIdGenerator + SystemClock adapters (SMA-441)"
```

---

### Task 8: axum HTTP health adapter (`/healthz`, `/readyz`)

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/http.rs`
- Create: `rs/crates/services/paigasus-iam/tests/health.rs`
- Modify: `src/adapters/mod.rs` (`pub mod http;`), `Cargo.toml` (axum, tower-http, serde_json), `src/main.rs`

**Interfaces:**
- Produces: `adapters::http::{AppState, health_router, router}` — `health_router() -> axum::Router` (stateless, `/healthz` only); `router(state: AppState) -> axum::Router` (adds `/readyz` + layers); `AppState { db: sea_orm::DatabaseConnection }` (DB type introduced here as a field; SeaORM dep added now).

- [ ] **Step 1: Add deps** — `rs/Cargo.toml` already pins `axum`/`tower-http`/`serde_json`; ensure `tower-http` has the needed features:

```toml
tower-http = { version = "0.6", features = ["trace", "timeout"] }
```

Service `Cargo.toml` `[dependencies]`:

```toml
axum = { workspace = true }
tower-http = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
sea-orm = { workspace = true }   # for the DatabaseConnection in AppState + readyz ping
```

Add the workspace `sea-orm` pin now (its first consumer):

```toml
sea-orm = { version = "1", default-features = false, features = [
  "sqlx-postgres", "runtime-tokio-rustls", "macros", "with-uuid", "with-chrono",
] }
```

- [ ] **Step 2: Write `src/adapters/http.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! axum HTTP surface: `/healthz` (liveness) and `/readyz` (DB-backed readiness).

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde_json::json;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
}

/// Liveness only — stateless, so it is testable without a database.
pub fn health_router() -> Router {
    Router::new().route("/healthz", get(healthz))
}

/// Full HTTP surface: liveness + DB-backed readiness.
pub fn router(state: AppState) -> Router {
    health_router()
        .merge(Router::new().route("/readyz", get(readyz)).with_state(state))
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let ping = state
        .db
        .execute(Statement::from_string(state.db.get_database_backend(), "SELECT 1"))
        .await;
    match ping {
        Ok(_) => (StatusCode::OK, Json(json!({ "status": "ready" }))),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "unready" }))),
    }
}
```

- [ ] **Step 3: Register the module** — `src/adapters/mod.rs`: add `pub mod http;`.

- [ ] **Step 4: Write `tests/health.rs`** (no DB — uses `health_router`)

```rust
// SPDX-License-Identifier: Apache-2.0

//! HTTP liveness smoke test — `/healthz` returns 200 without a database.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use paigasus_iam::adapters::http::health_router;
use tower::ServiceExt; // for `oneshot`

#[tokio::test]
async fn healthz_returns_200() {
    let app = health_router();
    let resp = app
        .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
```

For the test to import `paigasus_iam::adapters::http`, the crate needs a library target. Add a `src/lib.rs` exposing the modules and have `main.rs` use the lib. **Step 4a:** create `src/lib.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! paigasus-iam library surface (for integration tests + the binary).

pub mod adapters;
pub mod application;
pub mod config;
```

Update `src/main.rs` to drop the inline `mod` decls and use the lib crate (`use paigasus_iam::…`). Add to `Cargo.toml`:

```toml
[lib]
name = "paigasus_iam"
path = "src/lib.rs"

[[bin]]
name = "paigasus-iam"
path = "src/main.rs"
```

And add the dev-dep for `oneshot`:

```toml
[dev-dependencies]
tower = { workspace = true }
```

- [ ] **Step 5: Verify**

```bash
moon run paigasus-iam-rs:test paigasus-iam-rs:lint paigasus-iam-rs:fmt
```
Expected: PASS (health test + prior tests).

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam rs/Cargo.toml rs/Cargo.lock
git commit -m "feat(rs): add axum health/readyz surface + lib target (SMA-441)"
```

---

### Task 9: tonic gRPC health adapter (`grpc.health.v1`)

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/grpc.rs`
- Create: `rs/crates/services/paigasus-iam/tests/grpc_health.rs`
- Modify: `src/adapters/mod.rs`, `Cargo.toml` (tonic, tonic-health)

**Interfaces:**
- Produces: `adapters::grpc::{health_service, serve}` — `health_service() -> (tonic_health::server::HealthReporter, HealthServer<impl Health>)` returning a reporter already set `SERVING` for the overall (`""`) service; `async fn serve(addr, shutdown)` wires the health service onto a tonic `Server` (used by `main`).

- [ ] **Step 1: Add deps** — `rs/Cargo.toml` pins `tonic`; add `tonic-health`:

```toml
tonic-health = "0.14"
```

Service `Cargo.toml` `[dependencies]`:

```toml
tonic = { workspace = true }
tonic-health = { workspace = true }
```

- [ ] **Step 2: Write `src/adapters/grpc.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! tonic gRPC surface. M0 serves only the well-known `grpc.health.v1.Health` (via
//! tonic-health); IAM RPCs arrive in later milestones.

use std::net::SocketAddr;
use tonic::transport::server::Router as TonicRouter;
use tonic::transport::Server;
use tonic_health::ServingStatus;

/// Build a health service with the overall server marked SERVING, plus its reporter
/// (so readiness can flip it later). The empty service name ("") is the overall status
/// that `grpc_health_probe`/k8s query by default.
pub async fn health_service() -> (
    tonic_health::server::HealthReporter,
    tonic_health::pb::health_server::HealthServer<impl tonic_health::pb::health_server::Health>,
) {
    let (reporter, service) = tonic_health::server::health_reporter();
    reporter.set_service_status("", ServingStatus::Serving).await;
    (reporter, service)
}

/// A tonic `Server` router with the health service mounted. `main` calls `.serve_with_shutdown`.
pub async fn router(timeout: std::time::Duration) -> TonicRouter {
    let (_reporter, health) = health_service().await;
    Server::builder()
        .timeout(timeout)
        .add_service(health)
}

/// Serve gRPC on `addr` until `shutdown` resolves.
pub async fn serve(
    addr: SocketAddr,
    timeout: std::time::Duration,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<(), tonic::transport::Error> {
    router(timeout).await.serve_with_shutdown(addr, shutdown).await
}
```

Note: verify the `tonic-health` 0.14 re-export paths (`tonic_health::pb::health_server::{Health, HealthServer}`, `tonic_health::server::health_reporter`, `tonic_health::ServingStatus`) against the pinned crate docs; adjust the exact paths if the minor version differs, keeping the behavior (set `""` → `Serving`).

- [ ] **Step 3: Register the module** — `src/adapters/mod.rs`: add `pub mod grpc;`.

- [ ] **Step 4: Write `tests/grpc_health.rs`** (boots on an ephemeral port; no DB)

```rust
// SPDX-License-Identifier: Apache-2.0

//! gRPC health smoke test — the server reports SERVING for the overall service.

use std::time::Duration;
use tokio::net::TcpListener;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::{health_client::HealthClient, HealthCheckRequest};

#[tokio::test]
async fn grpc_health_reports_serving() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let (reporter, health) = paigasus_iam::adapters::grpc::health_service().await;
    let _ = &reporter; // keep the reporter alive for the server's lifetime
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .timeout(Duration::from_secs(5))
            .add_service(health)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    let mut client = HealthClient::connect(format!("http://{addr}")).await.unwrap();
    let resp = client
        .check(HealthCheckRequest { service: String::new() })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.status, ServingStatus::Serving as i32);

    server.abort();
}
```

Add dev-deps to the service `Cargo.toml`:

```toml
[dev-dependencies]
tower = { workspace = true }
tokio-stream = { workspace = true, features = ["net"] }
```

And pin `tokio-stream` in `rs/Cargo.toml`:

```toml
tokio-stream = { version = "0.1", default-features = false }
```

- [ ] **Step 5: Verify**

```bash
moon run paigasus-iam-rs:test paigasus-iam-rs:lint paigasus-iam-rs:fmt
```
Expected: PASS (gRPC health test boots + checks SERVING).

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam rs/Cargo.toml rs/Cargo.lock
git commit -m "feat(rs): add tonic grpc.health.v1 surface + boot test (SMA-441)"
```

---

### Task 10: SeaORM persistence — entities, migration, `PgPrincipalRepository` + round-trip

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs`
- Create: `.../persistence/entities/mod.rs`, `.../entities/principal.rs`, `.../entities/user.rs`
- Create: `.../persistence/migration/mod.rs`, `.../migration/m0001_create_principal_and_user.rs`
- Create: `.../persistence/pg_repository.rs`
- Create: `rs/crates/services/paigasus-iam/tests/roundtrip.rs`
- Modify: `src/adapters/mod.rs`, `Cargo.toml` (sea-orm-migration; dev: testcontainers)

**Interfaces:**
- Produces: `adapters::persistence::{Migrator, PgPrincipalRepository}` — `PgPrincipalRepository::new(db: DatabaseConnection)`; impl `PrincipalRepository`. `Migrator: sea_orm_migration::MigratorTrait`.

- [ ] **Step 1: Add deps** — `rs/Cargo.toml`:

```toml
sea-orm-migration = { version = "1", default-features = false, features = [
  "sqlx-postgres", "runtime-tokio-rustls",
] }
```

`rs/deny.toml` — add the pre-committed crypto-backend license exception (§3-C11 of the spec):

```toml
exceptions = [
  { name = "ring", allow = ["OpenSSL"] },
]
```

Service `Cargo.toml`:

```toml
[dependencies]
sea-orm-migration = { workspace = true }
async-trait = { workspace = true }

[dev-dependencies]
testcontainers = "0.24"
testcontainers-modules = { version = "0.12", features = ["postgres"] }
```

Pin `testcontainers`/`testcontainers-modules` in `rs/Cargo.toml` if the workspace prefers central pins; otherwise crate-local dev-deps are acceptable (they never enter a published artifact). Verify the exact resolved versions and adjust the API in Steps 2/5 if needed.

- [ ] **Step 2: Write the SeaORM entities**

`entities/principal.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `principal` table. Persistence representation only — mapped to/from
//! the pure-core `Principal` in `pg_repository` (SeaORM never derives on the core types).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "principal")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub prn: String,
    pub kind: String,
    pub status: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_one = "super::user::Entity")]
    User,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
```

`entities/user.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `user` table (1:1 with `principal`, shared PK).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "user")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub principal_id: Uuid,
    #[sea_orm(unique)]
    pub email: String,
    pub display_name: String,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::principal::Entity",
        from = "Column::PrincipalId",
        to = "super::principal::Column::Id"
    )]
    Principal,
}

impl Related<super::principal::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Principal.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
```

`entities/mod.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

pub mod principal;
pub mod user;
```

- [ ] **Step 3: Write the migration**

`migration/m0001_create_principal_and_user.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! m0001 — create `principal` and `user` (1:1, shared PK). Text-backed enum columns.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Principal {
    Table,
    Id,
    Prn,
    Kind,
    Status,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum User {
    Table,
    PrincipalId,
    Email,
    DisplayName,
    Locale,
    Timezone,
    CreatedAt,
    UpdatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Principal::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Principal::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Principal::Prn).text().not_null().unique_key())
                    .col(ColumnDef::new(Principal::Kind).text().not_null())
                    .col(ColumnDef::new(Principal::Status).text().not_null())
                    .col(ColumnDef::new(Principal::CreatedAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(Principal::UpdatedAt).timestamp_with_time_zone().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(User::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(User::PrincipalId).uuid().not_null().primary_key())
                    .col(ColumnDef::new(User::Email).text().not_null().unique_key())
                    .col(ColumnDef::new(User::DisplayName).text().not_null())
                    .col(ColumnDef::new(User::Locale).text().null())
                    .col(ColumnDef::new(User::Timezone).text().null())
                    .col(ColumnDef::new(User::CreatedAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(User::UpdatedAt).timestamp_with_time_zone().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_principal")
                            .from(User::Table, User::PrincipalId)
                            .to(Principal::Table, Principal::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(User::Table).to_owned()).await?;
        manager.drop_table(Table::drop().table(Principal::Table).to_owned()).await?;
        Ok(())
    }
}
```

`migration/mod.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

mod m0001_create_principal_and_user;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m0001_create_principal_and_user::Migration)]
    }
}
```

- [ ] **Step 4: Write `pg_repository.rs`** (maps domain ↔ entity; `DbErr` → `RepositoryError`)

```rust
// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `PrincipalRepository` (SeaORM). Maps domain <-> entity models and
//! backend errors into the core's `RepositoryError`.

use super::entities::{principal, user};
use async_trait::async_trait;
use paigasus_iam_core::{
    Email, Principal, PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus,
    RepositoryError, User,
};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, DatabaseConnection, DbErr, EntityTrait, Set, SqlErr};

pub struct PgPrincipalRepository {
    db: DatabaseConnection,
}

impl PgPrincipalRepository {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgPrincipalRepository { db }
    }
}

fn map_err(e: DbErr) -> RepositoryError {
    match e.sql_err() {
        Some(SqlErr::UniqueConstraintViolation(msg)) => RepositoryError::Conflict(msg),
        _ => RepositoryError::Backend(Box::new(e)),
    }
}

#[async_trait]
impl PrincipalRepository for PgPrincipalRepository {
    async fn create_user(&self, p: &Principal, u: &User) -> Result<(), RepositoryError> {
        let principal = principal::ActiveModel {
            id: Set(p.id.uuid()),
            prn: Set(p.id.canonical()),
            kind: Set(p.kind.as_str().to_string()),
            status: Set(p.status.as_str().to_string()),
            created_at: Set(p.created_at),
            updated_at: Set(p.updated_at),
        };
        principal.insert(&self.db).await.map_err(map_err)?;

        let user = user::ActiveModel {
            principal_id: Set(u.principal_id.uuid()),
            email: Set(u.email.as_str().to_string()),
            display_name: Set(u.display_name.clone()),
            locale: Set(u.locale.clone()),
            timezone: Set(u.timezone.clone()),
            created_at: Set(u.created_at),
            updated_at: Set(u.updated_at),
        };
        user.insert(&self.db).await.map_err(map_err)?;
        Ok(())
    }

    async fn find_user(
        &self,
        id: &PrincipalId,
    ) -> Result<Option<(Principal, User)>, RepositoryError> {
        let uuid = id.uuid();
        let Some(pm) = principal::Entity::find_by_id(uuid).one(&self.db).await.map_err(map_err)?
        else {
            return Ok(None);
        };
        let Some(um) = user::Entity::find_by_id(uuid).one(&self.db).await.map_err(map_err)?
        else {
            return Ok(None);
        };

        let prn = Prn::parse(&pm.prn)
            .map_err(|e| RepositoryError::Backend(Box::new(std::io::Error::other(e.to_string()))))?;
        let pid = PrincipalId::from_prn(prn);
        let kind = PrincipalKind::parse(&pm.kind)
            .ok_or_else(|| RepositoryError::Backend(Box::new(std::io::Error::other("bad kind"))))?;
        let status = PrincipalStatus::parse(&pm.status).ok_or_else(|| {
            RepositoryError::Backend(Box::new(std::io::Error::other("bad status")))
        })?;
        let email = Email::parse(&um.email)
            .map_err(|e| RepositoryError::Backend(Box::new(std::io::Error::other(format!("{e}")))))?;

        let principal = Principal::new(pid.clone(), kind, status, pm.created_at, pm.updated_at);
        let user = User::new(
            pid,
            email,
            um.display_name,
            um.locale,
            um.timezone,
            um.created_at,
            um.updated_at,
        );
        Ok(Some((principal, user)))
    }
}
```

`persistence/mod.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Postgres persistence adapter: entities, migrations, and the repository impl.

pub mod entities;
pub mod migration;
pub mod pg_repository;

pub use migration::Migrator;
pub use pg_repository::PgPrincipalRepository;
```

Register in `src/adapters/mod.rs`: add `pub mod persistence;`.

- [ ] **Step 5: Write `tests/roundtrip.rs`** (testcontainers; hard-fail in CI if Docker absent)

```rust
// SPDX-License-Identifier: Apache-2.0

//! AC #2 — a Principal/User row round-trips through real Postgres.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note.

use chrono::{SubsecRound, Utc};
use paigasus_iam::adapters::persistence::{Migrator, PgPrincipalRepository};
use paigasus_iam_core::{
    Email, Principal, PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus, User,
};
use paigasus_kernel::{mint_uuid7, Prn};
use sea_orm::Database;
use sea_orm_migration::MigratorTrait;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

#[tokio::test]
async fn principal_user_round_trips_through_postgres() {
    let node = match Postgres::default().start().await {
        Ok(n) => n,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("Docker is required for the round-trip test in CI: {e}");
            }
            eprintln!("skipping round-trip: Docker unavailable ({e})");
            return;
        }
    };

    let port = node.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let db = Database::connect(&url).await.unwrap();
    Migrator::up(&db, None).await.unwrap();

    // Build a principal with µs-truncated timestamps (matches the SystemClock contract).
    let uuid = mint_uuid7(1_700_000_000_000, [7u8; 10]);
    let id = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", uuid).unwrap());
    let now = Utc::now().trunc_subsecs(6);
    let principal = Principal::new(id.clone(), PrincipalKind::User, PrincipalStatus::Active, now, now);
    let user = User::new(
        id.clone(),
        Email::parse("roundtrip@example.com").unwrap(),
        "Round Trip".into(),
        Some("en-US".into()),
        None,
        now,
        now,
    );

    let repo = PgPrincipalRepository::new(db);
    repo.create_user(&principal, &user).await.unwrap();

    let (got_p, got_u) = repo.find_user(&id).await.unwrap().expect("row present");
    assert_eq!(got_p, principal);
    assert_eq!(got_u, user);
}
```

- [ ] **Step 6: Verify**

```bash
# Requires a running Docker daemon locally; without it, the round-trip test SKIPS (CI unset).
moon run paigasus-iam-rs:test paigasus-iam-rs:lint paigasus-iam-rs:fmt
moon run repo:deny        # confirm the ring/OpenSSL exception keeps deny green
moon run repo:machete     # confirm no unused deps
```
Expected: PASS. With Docker up, the round-trip asserts full equality; the `deny` gate is green with the new exception.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam rs/Cargo.toml rs/Cargo.lock rs/deny.toml
git commit -m "feat(rs): add SeaORM persistence + Postgres round-trip test (SMA-441)"
```

---

### Task 11: `main.rs` composition root + graceful shutdown

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/main.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http.rs` (add a `serve_http` helper) if not already present

**Interfaces:**
- Consumes: `config::IamConfig`, `paigasus_logging::init`, `adapters::http::{router, AppState}`, `adapters::grpc::serve`, `adapters::persistence::Migrator`, `sea_orm::Database`.
- Produces: a booting binary that serves HTTP + gRPC health, runs migrations, and shuts down gracefully on SIGINT/SIGTERM.

- [ ] **Step 1: Add an axum serve helper** — append to `src/adapters/http.rs`:

```rust
use std::net::SocketAddr;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Serve the HTTP surface on `addr` until `shutdown` resolves.
pub async fn serve_http(
    addr: SocketAddr,
    state: AppState,
    request_timeout: Duration,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let app = router(state)
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::new(request_timeout));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).with_graceful_shutdown(shutdown).await
}
```

- [ ] **Step 2: Write `src/main.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! paigasus-iam composition root: load config, init logging, connect + migrate the DB,
//! serve HTTP + gRPC health on two ports, and shut down gracefully on SIGINT/SIGTERM.

use std::time::Duration;

use paigasus_iam::adapters::http::{serve_http, AppState};
use paigasus_iam::adapters::{grpc, persistence::Migrator};
use paigasus_iam::config::IamConfig;
use sea_orm::Database;
use sea_orm_migration::MigratorTrait;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    paigasus_logging::init("paigasus-iam");

    let config = IamConfig::load()?;
    let db = Database::connect(&config.database_url).await?;
    Migrator::up(&db, None).await?;

    let request_timeout = Duration::from_secs(30);
    let (tx, rx) = tokio::sync::watch::channel(());

    let http = {
        let mut rx = rx.clone();
        let state = AppState { db: db.clone() };
        tokio::spawn(async move {
            serve_http(config.http_addr, state, request_timeout, async move {
                let _ = rx.changed().await;
            })
            .await
        })
    };

    let grpc = {
        let mut rx = rx.clone();
        tokio::spawn(async move {
            grpc::serve(config.grpc_addr, request_timeout, async move {
                let _ = rx.changed().await;
            })
            .await
        })
    };

    tracing::info!(%config.http_addr, %config.grpc_addr, "paigasus-iam started");
    shutdown_signal().await;
    tracing::info!("shutdown signal received");
    let _ = tx.send(());

    let (http_res, grpc_res) = tokio::try_join!(http, grpc)?;
    http_res?;
    grpc_res?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
```

- [ ] **Step 3: Verify build + all fast tests**

```bash
moon run paigasus-iam-rs:build paigasus-iam-rs:lint paigasus-iam-rs:fmt paigasus-iam-rs:test
```
Expected: PASS. (The boot path is exercised by `tests/health.rs` + `tests/grpc_health.rs` for the surfaces and `tests/roundtrip.rs` for DB+migration; `main` is thin glue over those.)

- [ ] **Step 4: Optional manual boot check** (with a local Postgres or the example URL):

```bash
IAM_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/postgres \
  cargo run -p paigasus-iam &
sleep 2
curl -s localhost:8080/healthz   # {"status":"ok"}
curl -s localhost:8080/readyz    # {"status":"ready"} if DB reachable
kill %1
```

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam
git commit -m "feat(rs): wire paigasus-iam composition root + graceful shutdown (SMA-441)"
```

---

### Task 12: Full local CI-parity verification

**Files:** none (verification + fixes only).

- [ ] **Step 1: Run the full affected graph the way CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking \
        :affected-smoke :parity-corpus-drift :wasm-getrandom-free \
        --base origin/main --include-relations
```
Expected: all green. Common fixes if red:
- `deny` license → confirm the `ring` (or `aws-lc-sys`) OpenSSL exception is present in `rs/deny.toml`.
- `machete` → an added workspace dep isn't consumed by any crate; remove it or add the consumer.
- `fmt` → run `cargo fmt` in `rs/` and `moon run contracts:fmt`.

- [ ] **Step 2: Confirm the codegen drift gate is clean**

```bash
moon run contracts:generate
git status --porcelain -- rs/crates/libs/paigasus-proto/src/generated \
  py/packages/paigasus-proto/src/paigasus_proto/generated \
  ts/packages/paigasus-proto/src/generated
```
Expected: no changes (generated code already committed in Task 2).

- [ ] **Step 3: Confirm CODEOWNERS is in sync**

```bash
moon sync code-owners
git diff --exit-code .github/CODEOWNERS
```
Expected: no diff (or commit the regenerated CODEOWNERS if the new projects added owners).

- [ ] **Step 4: Final commit if anything changed**

```bash
git add -A
git commit -m "chore(rs): keep paigasus-iam CI gates green (SMA-441)" || echo "nothing to fix"
```

---

## Self-Review

**Spec coverage:**
- §2.1 paigasus-logging → Task 1 ✓
- §2.2 iam-core (entities, VOs, ports) → Tasks 3–4 ✓
- §2.3 hexagonal service → Tasks 5–11 ✓
- §2.4 iam.proto + codegen → Task 2 ✓
- §2.5 HTTP/gRPC health, two ports → Tasks 8, 9, 11 ✓
- §2.6 SeaORM + migration + round-trip → Task 10 ✓
- §2.7 figment config + iam.toml.example → Task 5 ✓
- §2.8 Moon projects + green CI → every task's moon.yml + Task 12 ✓
- §3-C8 gRPC health test → Task 9 ✓; C9 CI-hard-fail Docker gate → Task 10 Step 5 ✓; C10 Clock + µs → Tasks 4, 7, 10 ✓; C11 deny exception → Task 10 Step 1 ✓; C12 no proto dep on service → Task 5 manifest (absent) ✓; C13 prost-only include → Task 2 Step 4 ✓; C14 protocol-correct layers → Tasks 8/9/11 ✓; C15 async-trait repo → Task 4 ✓; C16 minors (source-preserving error, entity-side enums, Email rule, SPDX, PrincipalId(Prn), watch shutdown, config test) → Tasks 3/4/5/8/10/11 ✓

**Placeholder scan:** the Task 5 `main.rs` is an explicit temporary stub, replaced in full in Task 11 — not a plan placeholder. No "TBD"/"add error handling"/uncoded steps remain.

**Type consistency:** `PrincipalId::uuid()`/`canonical()`/`from_prn()`, `PrincipalKind::{as_str,parse}`, `PrincipalStatus::{as_str,parse}`, `Email::{parse,as_str}`, `PrincipalRepository::{create_user,find_user}`, `Clock::now`, `IdGenerator::new_principal_id`, `CreateUser::{new,execute}`, `adapters::http::{health_router,router,serve_http,AppState}`, `adapters::grpc::{health_service,serve}`, `adapters::persistence::{Migrator,PgPrincipalRepository}` — used consistently across tasks.

**Known version-sensitivity to confirm during execution (not placeholders — the behavior is fixed, only exact paths may shift with the pinned minor):** `tonic-health` 0.14 re-export paths (Task 9); `sea-orm` 1.x `DbErr::sql_err()`/`SqlErr::UniqueConstraintViolation` shape (Task 10); `testcontainers`/`testcontainers-modules` runner API + `get_host_port_ipv4` (Task 10). Each has a note at its use site.
