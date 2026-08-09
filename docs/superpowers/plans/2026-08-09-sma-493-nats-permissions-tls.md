# SMA-493 — NATS permissions, TLS and credential rotation: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `[outbox.publisher].backend = "nats"` safe to turn on in production — a dedicated NATS account with least-privilege subject permissions, mandatory TLS with a nameable CA, and credentials that are re-read on every reconnect — with the permission set proven by an executable CI gate.

**Architecture:** Committed ops artifacts under `ops/nats/` (one `subjects.env` source of truth, an nsc provisioning script, and a static-nkey test fixture) plus three service changes: a `creds.rs` credential loader wired into `async-nats`' per-attempt `auth_callback`, three new `PublisherConfig` fields with `validate()` rules, and `event_callback` logging so permission denials are diagnosable. Integration tests boot a real broker with the committed permission config and assert both sufficiency and over-breadth.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), `async-nats` 0.50, `nkeys` 0.4.5, `testcontainers` 0.27.3 + `testcontainers-modules` 0.15, `rcgen` 0.13 (already a dev-dep), Moon 2.3.2, NATS server 2.10.14.

**Spec:** `docs/superpowers/specs/2026-08-09-sma-493-nats-permissions-tls-design.md`

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for shell).
- Rust crates are **edition 2024 + rust-version 1.95**.
- `[workspace.lints.rust] warnings = "deny"` — **dead code is a hard compile error**. Anything added in one task must be `pub` and re-exported, or consumed in the same task. Never stage an unused item "to wire up later".
- Shell commands need `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` (shims first) before `moon`/`cargo nextest`/`buf` resolve to the pinned versions.
- All commands run from the worktree root `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-493-nats-permissions-tls`. Never `cd` to the main checkout.
- Conventional commits with a workspace scope (`feat(rs):`, `docs(rs):`, `test(rs):`, `chore(ops):`). Commit **subject must start lowercase** and be ≤100 chars. Never put a bare `#NNN` or a `token: value` line in the commit body — it breaks commitlint's footer parsing.
- Never bypass git hooks with `--no-verify`.
- Docker is required for the integration tests: a missing daemon is a **hard failure when `CI` is set** and a skip otherwise (mirror `tests/nats_publisher.rs::start_nats`).
- New workspace deps may need `rs/deny.toml` entries; a dep added before it is consumed reds `:machete`.

## File Structure

**Created**
- `ops/nats/subjects.env` — the single source for every subject and inbox prefix
- `ops/nats/check-subjects.sh` — asserts the test fixture grants exactly `subjects.env`
- `ops/nats/provision.sh` — nsc + nats CLI provisioning
- `ops/nats/permissions.md`, `ops/nats/README.md` — operator documentation
- `ops/nats/test/accounts.conf.tmpl` — static-nkey account block (nkey placeholders rendered at test time)
- `ops/nats/test/nats-server.conf`, `ops/nats/test/nats-server-tls.conf` — fixture server configs
- `rs/crates/services/paigasus-iam/src/adapters/events/creds.rs` — credential loader
- `rs/crates/services/paigasus-iam/tests/nats_permissions.rs` — permission + TLS + rotation integration tests
- `docs/ops/RUNBOOK-nats.md` — operator runbook

**Modified**
- `rs/crates/libs/paigasus-iam-core/src/domain_event.rs` — `EventType::ALL` becomes `pub`
- `rs/crates/services/paigasus-iam/src/config.rs` — three fields, three validate rules, tests
- `rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs` — connect options
- `rs/crates/services/paigasus-iam/src/adapters/events/mod.rs` — re-export `creds`
- `rs/Cargo.toml`, `rs/crates/services/paigasus-iam/Cargo.toml` — `nkeys`
- `moon.yml`, `.github/workflows/ci.yml` — the `nats-permissions` gate
- `docs/dev-setup.md`, `docs/ops/RUNBOOK-observability.md` — docs

---

### Task 1: Make `EventType::ALL` public

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/domain_event.rs:73` (the `#[cfg(test)]`-private `ALL`)

**Interfaces:**
- Produces: `paigasus_iam_core::EventType::ALL: [EventType; 8]` — consumed by Task 6's integration test, which cannot see a `cfg(test)` item in the lib.

- [ ] **Step 1: Write the failing test**

Add to the existing `mod tests` in `domain_event.rs`, after `wire_strings_are_namespaced_and_distinct`:

```rust
    /// `ALL` must stay exhaustive: this match has no wildcard arm, so a new variant fails to
    /// compile here rather than silently shrinking every consumer's coverage (SMA-493 §3.4 —
    /// `tests/nats_permissions.rs` iterates `ALL` to prove the publisher's grant covers every
    /// subject the service can emit).
    #[test]
    fn all_lists_every_event_type() {
        for et in EventType::ALL {
            match et {
                EventType::PrincipalCreated
                | EventType::PrincipalArchived
                | EventType::RoleGranted
                | EventType::RoleRevoked
                | EventType::ApiKeyIssued
                | EventType::ApiKeyRevoked
                | EventType::PolicyPut
                | EventType::PolicyDeleted => {}
            }
        }
        assert_eq!(EventType::ALL.len(), 8);
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam-core domain_event
```

Expected: FAIL — `no associated item named ALL found for enum EventType`.

- [ ] **Step 3: Promote `ALL` onto the type**

Delete the `const ALL: [EventType; 8] = [...]` from `mod tests` and add it to the `impl EventType` block in the lib body, directly above `as_wire`:

```rust
    /// Every variant, in declaration order. Public because consumers outside this crate need to
    /// enumerate the event surface: `tests/nats_permissions.rs` (SMA-493) asserts the NATS
    /// publisher's `pub` grant covers every subject this service can emit, and an integration
    /// test cannot see a `#[cfg(test)]` constant. Kept exhaustive by
    /// `all_lists_every_event_type`, whose wildcard-free match stops compiling when a variant is
    /// added.
    pub const ALL: [EventType; 8] = [
        Self::PrincipalCreated,
        Self::PrincipalArchived,
        Self::RoleGranted,
        Self::RoleRevoked,
        Self::ApiKeyIssued,
        Self::ApiKeyRevoked,
        Self::PolicyPut,
        Self::PolicyDeleted,
    ];
```

Then update the two existing tests that referenced the private constant to use `EventType::ALL`:

```rust
    #[test]
    fn event_type_roundtrips_through_wire_strings() {
        for et in EventType::ALL {
            assert_eq!(EventType::parse(et.as_wire()), Some(et));
        }
        assert_eq!(EventType::parse("nope"), None);
    }

    #[test]
    fn wire_strings_are_namespaced_and_distinct() {
        let wires: Vec<&str> = EventType::ALL.iter().map(EventType::as_wire).collect();
```

(The remainder of `wire_strings_are_namespaced_and_distinct` is unchanged.)

- [ ] **Step 4: Run tests and lint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam-core && cargo clippy -p paigasus-iam-core -- -D warnings
```

Expected: PASS, no warnings.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src/domain_event.rs
git commit -m "refactor(rs): expose EventType::ALL for cross-crate exhaustive iteration (SMA-493)"
```

---

### Task 2: The credential loader (`creds.rs`)

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/events/creds.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/mod.rs` (declare + re-export)
- Modify: `rs/Cargo.toml` (workspace `nkeys`), `rs/crates/services/paigasus-iam/Cargo.toml`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `pub enum CredsError` (variants `MissingSeed`, `MissingJwt`, `BadSeed(String)`, `Sign(String)`) — `thiserror::Error`
  - `pub fn parse_credentials(raw: &str) -> Result<ParsedCredentials, CredsError>`
  - `pub struct ParsedCredentials { pub jwt: Option<String>, pub key_pair: nkeys::KeyPair }`
  - `pub async fn auth_from_credentials(path: &str, nonce: &[u8]) -> Result<async_nats::Auth, async_nats::AuthError>`
  - Task 4 calls `parse_credentials` for its pre-flight and `auth_from_credentials` inside the auth callback.

- [ ] **Step 1: Add the `nkeys` dependency**

In `rs/Cargo.toml`, in the `[workspace.dependencies]` block, directly after the `async-nats` entry:

```toml
# nkeys — NATS ed25519 key handling, a DIRECT dependency of `paigasus-iam` since SMA-493.
# Already in the tree transitively (async-nats' own `auth_utils` under the `nkeys` feature this
# workspace enables), so this pins no new build weight. It is needed directly because
# async-nats' `.creds` parser is `pub(crate)`: rotation-safe auth re-reads and re-signs on every
# connection attempt via `ConnectOptions::with_auth_callback`, which hands us a raw nonce and
# expects a signature back (SMA-493 D8). Version tracks what async-nats 0.50 resolves.
nkeys = "0.4.5"
```

In `rs/crates/services/paigasus-iam/Cargo.toml`, in `[dependencies]`, directly after the `async-nats` line:

```toml
# `adapters::events::creds` signs the server nonce on every connection attempt so a rotated
# `.creds` is picked up without a restart (SMA-493 D8).
nkeys = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Create `rs/crates/services/paigasus-iam/src/adapters/events/creds.rs` containing **only** the test module for now (the file will not compile — that is the point of step 3):

```rust
// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders a `.creds` file body the way `nsc generate creds` does: a decorated JWT block
    /// followed by a decorated seed block.
    fn creds_file(jwt: &str, seed: &str) -> String {
        format!(
            "-----BEGIN NATS USER JWT-----\n{jwt}\n------END NATS USER JWT------\n\n\
             *************************** IMPORTANT ***************************\n\
             NKEY Seed printed below can be used sign and prove identity.\n\n\
             -----BEGIN USER NKEY SEED-----\n{seed}\n------END USER NKEY SEED------\n"
        )
    }

    fn a_seed() -> String {
        nkeys::KeyPair::new_user().seed().expect("a fresh user keypair exposes its seed")
    }

    #[test]
    fn a_two_block_creds_file_yields_jwt_auth() {
        let seed = a_seed();
        let parsed = parse_credentials(&creds_file("header.payload.signature", &seed)).expect("valid creds");
        assert_eq!(parsed.jwt.as_deref(), Some("header.payload.signature"));
        assert_eq!(parsed.key_pair.seed().unwrap(), seed);
    }

    /// A bare seed file (`.nk`) authenticates by nkey instead — the fixture shape SMA-493 D2's
    /// static-account test broker uses, and the reason this loader keys off the file's contents
    /// rather than a config flag.
    #[test]
    fn a_seed_only_file_yields_nkey_auth() {
        let seed = a_seed();
        let parsed = parse_credentials(&format!("-----BEGIN USER NKEY SEED-----\n{seed}\n------END USER NKEY SEED------\n")).expect("valid seed file");
        assert!(parsed.jwt.is_none(), "a seed-only file has no JWT to present");
        assert_eq!(parsed.key_pair.seed().unwrap(), seed);
    }

    /// Deliberately stricter than async-nats' own parser, which takes the first and second
    /// `-----`-delimited blocks REGARDLESS of their labels (`auth_utils.rs:74-91`). Keying on the
    /// labels means a file whose blocks are reordered or mislabelled is rejected rather than
    /// silently misread as a JWT (SMA-493 D8).
    #[test]
    fn a_mislabelled_block_is_not_read_as_a_seed() {
        let err = parse_credentials("-----BEGIN SOMETHING ELSE-----\nSUAAAA\n------END SOMETHING ELSE------\n").expect_err("must not accept an unlabelled block");
        assert!(matches!(err, CredsError::MissingSeed), "got {err:?}");
    }

    #[test]
    fn an_empty_file_is_rejected() {
        assert!(matches!(parse_credentials("").expect_err("empty is not credentials"), CredsError::MissingSeed));
    }

    #[test]
    fn a_corrupt_seed_is_rejected() {
        let err = parse_credentials("-----BEGIN USER NKEY SEED-----\nNOTASEED\n------END USER NKEY SEED------\n").expect_err("a malformed seed must not parse");
        assert!(matches!(err, CredsError::BadSeed(_)), "got {err:?}");
    }

    /// The property D8 exists for: the file is read on EVERY call, so a rotated credential is
    /// picked up by the next connection attempt without a restart.
    #[tokio::test]
    async fn each_call_re_reads_the_file_from_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("iam.creds");
        let path_str = path.to_string_lossy().to_string();

        std::fs::write(&path, creds_file("jwt.one", &a_seed())).unwrap();
        let first = auth_from_credentials(&path_str, b"nonce").await.expect("first load");

        std::fs::write(&path, creds_file("jwt.two", &a_seed())).unwrap();
        let second = auth_from_credentials(&path_str, b"nonce").await.expect("second load");

        assert_eq!(first.jwt.as_deref(), Some("jwt.one"));
        assert_eq!(second.jwt.as_deref(), Some("jwt.two"), "the rotated file must be re-read, not cached");
    }

    /// The signature must verify against the seed's own public key — async-nats base64url-encodes
    /// what we hand back (`connector.rs:694-696`), so we return RAW bytes.
    #[tokio::test]
    async fn the_signature_verifies_against_the_seeds_public_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("iam.creds");
        let seed = a_seed();
        std::fs::write(&path, creds_file("jwt.one", &seed)).unwrap();

        let auth = auth_from_credentials(&path.to_string_lossy(), b"the-server-nonce").await.expect("load");
        let kp = nkeys::KeyPair::from_seed(&seed).unwrap();
        kp.verify(b"the-server-nonce", &auth.signature.expect("a signature is always returned"))
            .expect("the signature must verify against the same seed");
    }

    #[tokio::test]
    async fn a_missing_file_names_the_path() {
        let err = auth_from_credentials("/nonexistent/iam.creds", b"nonce").await.expect_err("missing file");
        assert!(format!("{err}").contains("/nonexistent/iam.creds"), "{err}");
    }
}
```

- [ ] **Step 3: Run the tests and watch them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam creds
```

Expected: FAIL — compile errors (`cannot find function parse_credentials`, `creds` module not declared). If `tempfile` is missing from dev-deps, add `tempfile = "3"` to `[dev-dependencies]` in `rs/crates/services/paigasus-iam/Cargo.toml` (check first: `grep tempfile rs/crates/services/paigasus-iam/Cargo.toml`).

- [ ] **Step 4: Implement the loader**

Prepend to `creds.rs`, above the test module:

```rust
// SPDX-License-Identifier: Apache-2.0

//! NATS credential loading (SMA-493 D8) — the piece that makes a rotated credential take effect
//! without a process restart.
//!
//! **Why this exists at all.** `ConnectOptions::with_credentials_file` reads the file exactly
//! once (`options.rs:429`), caches the JWT string and the parsed `KeyPair`, and every reconnect
//! rebuilds its `CONNECT` from that cache (`connector.rs:666`). A NATS user JWT can carry an
//! expiry; when the cached one lapses, every reconnect fails `AuthorizationViolation` and the
//! process cannot recover. `ConnectOptions::with_auth_callback` is invoked on EVERY connection
//! attempt (`connector.rs:681`), so a callback that re-reads the file closes that gap.
//!
//! **Two file shapes, one code path.** A `.creds` (JWT + seed) authenticates by JWT — the
//! production shape. A bare seed file authenticates by nkey, which is what lets an integration
//! test run the real adapter against a static-account broker whose users are declared as
//! `{ nkey: "U…", permissions: {…} }` (SMA-493 D2). The shape is read from the file, not
//! configured, because it is a property of the credential rather than of the deployment.
//!
//! **Stricter than upstream, deliberately.** `async-nats`' own parser takes the first and second
//! `-----`-delimited blocks regardless of their labels (`auth_utils.rs:74-91`). This one keys on
//! `BEGIN NATS USER JWT` and `BEGIN USER NKEY SEED`, so a mislabelled or reordered file is
//! rejected instead of silently misread.

use async_nats::{Auth, AuthError};
use nkeys::KeyPair;

const JWT_LABEL: &str = "NATS USER JWT";
const SEED_LABEL: &str = "USER NKEY SEED";

/// Why a credential file could not be turned into an [`Auth`].
#[derive(Debug, thiserror::Error)]
pub enum CredsError {
    #[error("no `-----BEGIN {SEED_LABEL}-----` block found")]
    MissingSeed,
    #[error("a `-----BEGIN {JWT_LABEL}-----` block was opened but never closed")]
    MissingJwt,
    #[error("the nkey seed could not be parsed: {0}")]
    BadSeed(String),
    #[error("the server nonce could not be signed: {0}")]
    Sign(String),
}

/// A parsed credential: always a key pair, and a JWT when the file carries one.
#[derive(Debug)]
pub struct ParsedCredentials {
    pub jwt: Option<String>,
    pub key_pair: KeyPair,
}

/// Extracts the single line inside the `-----BEGIN {label}-----` / `------END {label}------`
/// block, if present. Hand-rolled rather than regex-backed: two delimited blocks do not need a
/// regex engine, and keying on the label is the strictness this module wants.
fn block(raw: &str, label: &str) -> Option<String> {
    let begin = raw.find(&format!("BEGIN {label}"))?;
    let after_begin = raw[begin..].find('\n')? + begin + 1;
    let end = raw[after_begin..].find("---")? + after_begin;
    let body: String = raw[after_begin..end].split_whitespace().collect();
    if body.is_empty() { None } else { Some(body) }
}

/// Parses a `.creds` (JWT + seed) or a bare seed file.
///
/// # Errors
///
/// [`CredsError::MissingSeed`] when no seed block is present — including an empty file and a
/// file whose blocks carry other labels — and [`CredsError::BadSeed`] when the seed is present
/// but not a valid nkey.
pub fn parse_credentials(raw: &str) -> Result<ParsedCredentials, CredsError> {
    let seed = block(raw, SEED_LABEL).ok_or(CredsError::MissingSeed)?;
    let key_pair = KeyPair::from_seed(&seed).map_err(|e| CredsError::BadSeed(e.to_string()))?;
    // A JWT block that is opened but unterminated is an error rather than "no JWT": silently
    // downgrading a production `.creds` to nkey auth would fail against an operator-mode broker
    // with a message about the wrong thing.
    let jwt = if raw.contains(&format!("BEGIN {JWT_LABEL}")) {
        Some(block(raw, JWT_LABEL).ok_or(CredsError::MissingJwt)?)
    } else {
        None
    };
    Ok(ParsedCredentials { jwt, key_pair })
}

/// Reads `path`, parses it, and signs `nonce` — the body of the auth callback
/// `NatsEventPublisher::connect` installs, called once per connection attempt.
///
/// Returns raw signature bytes: async-nats base64url-encodes them itself
/// (`connector.rs:694-696`), so encoding here would double-encode.
///
/// Holds nothing across an await beyond the file's contents — `ConnectOptions::with_auth_callback`
/// requires the returned future to be `Send + Sync + 'static` (`options.rs:207`), and `KeyPair`
/// is constructed and used entirely after the read completes.
///
/// # Errors
///
/// An [`AuthError`] naming the path, for a file that cannot be read, cannot be parsed, or whose
/// key cannot sign. async-nats preserves this as the source of its `Authentication`
/// `ConnectError` (`connector.rs:685-688`) and logs it, so the path reaches the operator.
pub async fn auth_from_credentials(path: &str, nonce: &[u8]) -> Result<Auth, AuthError> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| AuthError::new(format!("nats credentials file {path} could not be read: {e}")))?;
    let parsed = parse_credentials(&raw).map_err(|e| AuthError::new(format!("nats credentials file {path} is malformed: {e}")))?;
    let signature = parsed
        .key_pair
        .sign(nonce)
        .map_err(|e| AuthError::new(format!("nats credentials file {path}: {}", CredsError::Sign(e.to_string()))))?;

    let mut auth = Auth::new();
    auth.signature = Some(signature);
    match parsed.jwt {
        Some(jwt) => auth.jwt = Some(jwt),
        // No JWT: authenticate by nkey. The server matches the public key against its configured
        // `users: [{ nkey: … }]` and verifies the signature over the nonce it sent.
        None => auth.nkey = Some(parsed.key_pair.public_key()),
    }
    Ok(auth)
}
```

In `rs/crates/services/paigasus-iam/src/adapters/events/mod.rs`, add the module declaration and re-export (alphabetical, after `cloud_event`). **Both are required**: an unreferenced private module would be dead code, which this workspace denies.

```rust
pub mod creds;
```

```rust
pub use creds::{CredsError, ParsedCredentials, auth_from_credentials, parse_credentials};
```

- [ ] **Step 5: Run the tests and the lints**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam creds && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```

Expected: 8 tests PASS, no warnings.

- [ ] **Step 6: Verify the new dep passes the supply-chain gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:deny repo:machete
```

Expected: both pass. `nkeys` is consumed in this same commit, so `:machete` is satisfied; if `:deny` objects to its licence, add an `rs/deny.toml` `[licenses] exceptions` entry naming `nkeys` and say why in a comment.

- [ ] **Step 7: Commit**

```bash
git add rs/Cargo.toml rs/Cargo.lock rs/crates/services/paigasus-iam/Cargo.toml \
  rs/crates/services/paigasus-iam/src/adapters/events/creds.rs \
  rs/crates/services/paigasus-iam/src/adapters/events/mod.rs
git commit -m "feat(rs): add a re-reading NATS credential loader (SMA-493)"
```

---

### Task 3: Config surface and validation

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs` — `PublisherConfig` (≈385-412), `Default` (≈421), `Debug` (≈436), `Serialize` (≈451), `validate` (≈980-1033), tests (≈2314+)

**Interfaces:**
- Consumes: nothing.
- Produces: `PublisherConfig::{root_ca_bundle: Option<String>, inbox_prefix: Option<String>, allow_insecure_broker: bool}` — Task 4 reads all three.

- [ ] **Step 1: Write the failing tests**

Append to the `[outbox.publisher]` section of `mod tests` in `config.rs`:

```rust
    // --- SMA-493: transport + credential posture ---------------------------------------------

    /// D6 rule 1. The default posture: a `nats` backend must speak TLS.
    #[test]
    fn a_plaintext_url_is_rejected() {
        let err = validate_err(
            r#"
            [outbox.publisher]
            backend = "nats"
            url = "nats://localhost:4222"
            credentials_file = "/etc/paigasus/iam.creds"
        "#,
        );
        assert!(err.contains("tls://"), "{err}");
        assert!(err.contains("allow_insecure_broker"), "the message must name the escape hatch: {err}");
    }

    #[test]
    fn a_plaintext_url_is_accepted_with_the_insecure_flag() {
        validate_result(
            r#"
            [outbox.publisher]
            backend = "nats"
            url = "nats://localhost:4222"
            allow_insecure_broker = true
        "#,
        )
        .expect("the explicit dev/CI escape hatch must be honoured");
    }

    /// D6 rule 2, unconditional. async-nats never reads url userinfo (`lib.rs:1682` has no
    /// caller), so accepting it would let a config that LOOKS authenticated connect anonymously.
    #[test]
    fn url_embedded_credentials_are_rejected_even_with_the_insecure_flag() {
        let err = validate_err(
            r#"
            [outbox.publisher]
            backend = "nats"
            url = "nats://user:pass@localhost:4222"
            allow_insecure_broker = true
        "#,
        );
        assert!(err.contains("credentials_file"), "{err}");
    }

    /// D6 rule 3.
    #[test]
    fn the_nats_backend_requires_a_credentials_file() {
        let err = validate_err(
            r#"
            [outbox.publisher]
            backend = "nats"
            url = "tls://localhost:4222"
        "#,
        );
        assert!(err.contains("credentials_file"), "{err}");
    }

    #[test]
    fn a_tls_url_with_credentials_passes() {
        validate_result(
            r#"
            [outbox.publisher]
            backend = "nats"
            url = "tls://nats.internal:4222"
            credentials_file = "/etc/paigasus/iam.creds"
            root_ca_bundle = "/etc/paigasus/nats-ca.pem"
            inbox_prefix = "_INBOX_IAM_PUB"
        "#,
        )
        .expect("the documented production shape must validate");
    }

    /// Every SMA-493 rule is gated on the `nats` backend: a `tracing` deployment must never fail
    /// boot over a broker it does not run.
    #[test]
    fn the_tracing_backend_is_unaffected_by_the_transport_rules() {
        validate_result(
            r#"
            [outbox.publisher]
            backend = "tracing"
            url = "nats://user:pass@localhost:4222"
        "#,
        )
        .expect("the tracing backend ignores publisher transport posture entirely");
    }

    #[test]
    fn the_new_publisher_fields_default_to_absent() {
        let cfg = load_minimal_config();
        assert_eq!(cfg.outbox.publisher.root_ca_bundle, None);
        assert_eq!(cfg.outbox.publisher.inbox_prefix, None);
        assert!(!cfg.outbox.publisher.allow_insecure_broker);
    }
```

- [ ] **Step 2: Run them and watch them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam config::tests
```

Expected: FAIL — `no field root_ca_bundle on type PublisherConfig`.

- [ ] **Step 3: Add the three fields**

In `PublisherConfig` (after `credentials_file`):

```rust
    /// Path to a PEM bundle of root CAs used to verify the broker's certificate (SMA-493 D7).
    ///
    /// **This REPLACES the system trust store, it does not extend it.**
    /// `ConnectOptions::add_root_certificates` assigns rather than appends (`options.rs:543`),
    /// and `config_tls` skips `load_native_certs()` entirely once any certificate is named
    /// (`tls.rs:61`). Concatenate every CA the client needs into one file — naming only a private
    /// CA and later moving the broker behind a public one is a total outage that presents as a
    /// bare TLS error. Omitted, the system trust store is used, which is the pre-SMA-493
    /// behaviour.
    ///
    /// Re-read on every connection attempt (`connector.rs:544`), so a rotated bundle needs no
    /// restart.
    pub root_ca_bundle: Option<String>,
    /// The client's `_INBOX` prefix (SMA-493 D4). MUST match the `subscribe` grant in the NATS
    /// account, or every publish times out waiting for an ack it is not allowed to receive.
    ///
    /// Not cosmetic: JetStream acks and pull-consumer deliveries both land on the client's inbox,
    /// so inside a shared account a client holding `sub _INBOX.>` can read another client's
    /// deliveries. Per-user prefixes are the only way to close that, because inbox replies are
    /// the one subject space every client must be able to read. `None` keeps async-nats' default
    /// `_INBOX`, so a deployment that has not adopted `ops/nats/` is unaffected.
    pub inbox_prefix: Option<String>,
    /// Escape hatch for a dev or CI broker (SMA-493 D6). Relaxes BOTH the `tls://` requirement
    /// and the `credentials_file` requirement — it legalises an unauthenticated broker as well as
    /// an unencrypted one, which is why it is not called `allow_plaintext`. Never relaxes the ban
    /// on url-embedded credentials, which async-nats ignores outright.
    pub allow_insecure_broker: bool,
```

In `Default for PublisherConfig`, after `credentials_file: None,`:

```rust
            root_ca_bundle: None,
            inbox_prefix: None,
            allow_insecure_broker: false,
```

In the hand-rolled `Debug`, after the `credentials_file` field: paths and a bool, so no redaction.

```rust
            .field("root_ca_bundle", &self.root_ca_bundle)
            .field("inbox_prefix", &self.inbox_prefix)
            .field("allow_insecure_broker", &self.allow_insecure_broker)
```

In the hand-rolled `Serialize`, **bump the field count from 8 to 11** (this impl is what supplies figment's defaults — a field missing here has no default and extraction fails when the key is absent from the file):

```rust
        let mut state = serializer.serialize_struct("PublisherConfig", 11)?;
```

and after the `credentials_file` line:

```rust
        state.serialize_field("root_ca_bundle", &self.root_ca_bundle)?;
        state.serialize_field("inbox_prefix", &self.inbox_prefix)?;
        state.serialize_field("allow_insecure_broker", &self.allow_insecure_broker)?;
```

- [ ] **Step 4: Add the validate rules**

In `validate()`, inside the existing `if self.outbox.publisher.backend == PublisherBackend::Nats {` block, immediately after the `p.url.is_none()` check:

```rust
            // --- SMA-493 D6: transport + credential posture --------------------------------
            // Parsed once and reused: `url::Url` is already this function's dependency (the
            // `source` check below), and both rules below ask questions about the same parse.
            // A url that does not parse at all is left to `connect` to report, exactly as
            // before — this block tightens posture, it does not add a syntax gate.
            if let Some(raw) = p.url.as_deref()
                && let Ok(parsed) = url::Url::parse(raw)
            {
                // Unconditional, no escape hatch: async-nats never reads url userinfo
                // (`ServerAddr::username`/`password` have no caller in the connect path), so a
                // config carrying `nats://user:pass@host` connects ANONYMOUSLY while looking
                // authenticated. Rejecting it is the only way that misconception surfaces.
                if !parsed.username().is_empty() || parsed.password().is_some() {
                    return Err(
                        "outbox.publisher.url must not embed credentials — async-nats ignores them entirely, so the connection would be anonymous; use outbox.publisher.credentials_file".to_string()
                    );
                }
                if parsed.scheme() != "tls" && !p.allow_insecure_broker {
                    return Err(format!(
                        "outbox.publisher.url must use tls:// for the nats backend (got {}://) — set outbox.publisher.allow_insecure_broker = true for a dev or CI broker, which also waives the credentials_file requirement",
                        parsed.scheme()
                    ));
                }
            }
            if p.credentials_file.is_none() && !p.allow_insecure_broker {
                return Err(
                    "outbox.publisher.backend = \"nats\" requires outbox.publisher.credentials_file (a NATS .creds) — set outbox.publisher.allow_insecure_broker = true for a dev or CI broker, which also waives the tls:// requirement".to_string()
                );
            }
```

- [ ] **Step 5: Migrate the ten existing publisher tests**

Ten existing fixtures use `url = "nats://localhost:4222"` (config.rs lines ≈2389, 2407, 2425, 2456, 2474, 2494, 2526, 2548, 2565, 2579) and now fail the new rules. Each of those tests is about a *different* rule (dedup window, `source` URI, relay pairing), so give them the production-shaped url rather than the escape hatch. In every one of those ten fixtures replace:

```toml
            url = "nats://localhost:4222"
```

with:

```toml
            url = "tls://localhost:4222"
            credentials_file = "/etc/paigasus/iam.creds"
```

Preserve each fixture's existing indentation (some are nested more deeply inside loops).

- [ ] **Step 6: Run the whole config suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam config && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```

Expected: PASS, including the seven new tests and all ten migrated ones.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/config.rs
git commit -m "feat(rs): require tls and a creds file for the nats publisher (SMA-493)"
```

---

### Task 4: Wire the connection options

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs` — the error enum (≈131-162) and `connect` (≈203-266)

**Interfaces:**
- Consumes: `creds::{parse_credentials, auth_from_credentials}` (Task 2); `PublisherConfig::{root_ca_bundle, inbox_prefix, allow_insecure_broker}` (Task 3).
- Produces: `NatsPublisherError::CredentialsParse { path, source }`. `NatsEventPublisher::connect` now installs an auth callback, an inbox prefix, a root CA bundle, and an event callback.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` at the bottom of `nats_publisher.rs` (no broker needed — these assert the pre-flight, which runs before any connection):

```rust
    /// The D8 pre-flight: a bad credential path fails boot with a typed error naming the path,
    /// rather than surfacing as an authentication failure on the first connection attempt.
    #[tokio::test]
    async fn connect_reports_a_missing_credentials_file_by_path() {
        let cfg = PublisherConfig {
            backend: crate::config::PublisherBackend::Nats,
            url: Some("nats://127.0.0.1:14222".to_string()),
            credentials_file: Some("/nonexistent/iam.creds".to_string()),
            ..PublisherConfig::default()
        };
        let err = NatsEventPublisher::connect(&cfg).await.expect_err("a missing creds file must fail boot");
        assert!(matches!(err, NatsPublisherError::Credentials { .. }), "got {err}");
        assert!(format!("{err}").contains("/nonexistent/iam.creds"), "{err}");
    }

    /// A file that reads but is not a credential gets its own variant: an operator seeing
    /// "No such file or directory" for a file that plainly exists learns nothing.
    #[tokio::test]
    async fn connect_reports_a_malformed_credentials_file_distinctly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("iam.creds");
        std::fs::write(&path, "this is not a creds file").unwrap();

        let cfg = PublisherConfig {
            backend: crate::config::PublisherBackend::Nats,
            url: Some("nats://127.0.0.1:14222".to_string()),
            credentials_file: Some(path.to_string_lossy().to_string()),
            ..PublisherConfig::default()
        };
        let err = NatsEventPublisher::connect(&cfg).await.expect_err("a malformed creds file must fail boot");
        assert!(matches!(err, NatsPublisherError::CredentialsParse { .. }), "got {err}");
        assert!(format!("{err}").contains(&path.to_string_lossy().to_string()), "{err}");
    }
```

- [ ] **Step 2: Run and watch it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam nats_publisher::tests
```

Expected: FAIL — `no variant named CredentialsParse`.

- [ ] **Step 3: Add the error variant**

In `NatsPublisherError`, directly after the existing `Credentials` variant:

```rust
    /// The `credentials_file` was read but is not a NATS credential. Split from
    /// [`Self::Credentials`] because "the file is missing" and "the file is not what you think it
    /// is" have different remediations, and an `io::Error` for a file that plainly exists reads
    /// as a filesystem problem.
    #[error("nats credentials file {path} could not be parsed")]
    CredentialsParse {
        path: String,
        #[source]
        source: crate::adapters::events::creds::CredsError,
    },
```

- [ ] **Step 4: Rebuild the connection options**

In `connect`, replace the `let opts = match &cfg.credentials_file { … };` block and the `let client = …` line with:

```rust
        // D8: read and parse the credential EAGERLY, before any connection machinery exists, so a
        // missing or malformed file is a typed boot error naming the path — then install the
        // callback that re-reads it on every subsequent attempt. `with_auth_callback` is a
        // CONSTRUCTOR (`options.rs:204`), so it has to start the chain rather than join it.
        let mut opts = match &cfg.credentials_file {
            Some(path) => {
                let raw = tokio::fs::read_to_string(path)
                    .await
                    .map_err(|source| NatsPublisherError::Credentials { path: path.clone(), source })?;
                crate::adapters::events::creds::parse_credentials(&raw)
                    .map_err(|source| NatsPublisherError::CredentialsParse { path: path.clone(), source })?;

                let path = path.clone();
                async_nats::ConnectOptions::with_auth_callback(move |nonce| {
                    let path = path.clone();
                    // Nothing non-`Sync` is held across the await inside: the callback's future
                    // must be `Send + Sync + 'static` (`options.rs:207`).
                    async move { crate::adapters::events::creds::auth_from_credentials(&path, &nonce).await }
                })
            }
            None => async_nats::ConnectOptions::new(),
        };

        // D4: the client's inbox prefix must match the account's `subscribe` grant. A mismatch is
        // not an error anywhere — it presents as every publish timing out on an ack the broker
        // refuses to deliver — which is why the event callback below matters so much.
        if let Some(prefix) = &cfg.inbox_prefix {
            opts = opts.custom_inbox_prefix(prefix.clone());
        }
        // D7: REPLACES the system trust store (see the field's doc). Re-read per attempt.
        if let Some(bundle) = &cfg.root_ca_bundle {
            opts = opts.add_root_certificates(std::path::PathBuf::from(bundle));
        }
        // D9: a denied publish is answered with an ASYNCHRONOUS `-ERR 'Permissions Violation …'`
        // and the request itself simply times out. Without this callback the single most likely
        // misconfiguration in a permissioned deployment is indistinguishable from a slow broker.
        opts = opts.event_callback(|event| async move {
            match event {
                async_nats::Event::ServerError(ref e) => tracing::error!(event = %event, "nats server error: {e}"),
                async_nats::Event::ClientError(ref e) => tracing::error!(event = %event, "nats client error: {e}"),
                async_nats::Event::Disconnected | async_nats::Event::LameDuckMode => tracing::warn!(event = %event, "nats connection event"),
                _ => tracing::info!(event = %event, "nats connection event"),
            }
        });

        let client = opts.connect(url).await.map_err(NatsPublisherError::Connect)?;
```

Update the module-level `//!` doc: add a paragraph noting that credentials and the CA bundle are both re-read per connection attempt (SMA-493 D7/D8), and that `event_callback` is what makes a permissions violation visible.

- [ ] **Step 5: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```

Expected: PASS. The 14 existing `tests/nats_publisher.rs` tests still pass unchanged — their `cfg()` helper builds `PublisherConfig` with `..PublisherConfig::default()`, so the three new fields arrive defaulted, and they call `connect` directly rather than `validate`.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs
git commit -m "feat(rs): re-read nats credentials on reconnect and log server errors (SMA-493)"
```

---

### Task 5: The `ops/nats/` artifacts

**Files:**
- Create: `ops/nats/subjects.env`, `ops/nats/check-subjects.sh`, `ops/nats/provision.sh`, `ops/nats/permissions.md`, `ops/nats/README.md`, `ops/nats/test/accounts.conf.tmpl`, `ops/nats/test/nats-server.conf`, `ops/nats/test/nats-server-tls.conf`

**Interfaces:**
- Produces: the fixture files Task 6 and Task 7 copy into containers, and the `{{PUBLISHER_NKEY}}` / `{{CONSUMER_NKEY}}` / `{{PROVISIONER_NKEY}}` placeholder contract those tests render.

- [ ] **Step 1: Write `ops/nats/subjects.env`**

```sh
# SPDX-License-Identifier: Apache-2.0
#
# The single source for every NATS subject and inbox prefix this platform grants (SMA-493 D10).
# `provision.sh` sources this to mint production users; `check-subjects.sh` asserts the test
# fixture grants exactly these. Editing a list here and nowhere else is the intended workflow —
# the gate fails until the fixture agrees.

STREAM_NAME="IAM_EVENTS"
DURABLE_NAME="gateway-cache-invalidator"

# Per-user inbox prefixes (D4). These are NOT cosmetic: JetStream acks and pull deliveries both
# land on the client's inbox, so a shared `_INBOX.>` grant inside one account lets any client
# read another's deliveries.
PUBLISHER_INBOX_PREFIX="_INBOX_IAM_PUB"
CONSUMER_INBOX_PREFIX="_INBOX_GW"
PROVISIONER_INBOX_PREFIX="_INBOX_PROV"

# iam-publisher — the paigasus-iam outbox relay's delivery identity (D3).
# `get_or_create_stream` probes STREAM.INFO and CREATEs on a 404, so both are required; the
# subscribe grant is what receives the JetStream publish ack, without which every publish times
# out. No `sub iam.>`: the publisher must not read the graph it writes. No STREAM.UPDATE /
# DELETE / PURGE: SMA-471 D7 makes non-reconciliation deliberate, and this enforces it.
PUBLISHER_PUB=(
  "iam.>"
  '$JS.API.STREAM.INFO.IAM_EVENTS'
  '$JS.API.STREAM.CREATE.IAM_EVENTS'
)
PUBLISHER_SUB=(
  "_INBOX_IAM_PUB.>"
)

# gateway-consumer — SMA-492's cache-invalidation identity (D5).
# Pull deliveries arrive on the inbox, never on `iam.*`, so subject-level subscribe permissions
# cannot narrow this user. The narrowing lives in the PRE-PROVISIONED durable's filter_subjects,
# and is binding only because no CONSUMER.CREATE verb is granted in any form.
CONSUMER_PUB=(
  '$JS.API.CONSUMER.MSG.NEXT.IAM_EVENTS.gateway-cache-invalidator'
  '$JS.API.CONSUMER.INFO.IAM_EVENTS.gateway-cache-invalidator'
  '$JS.ACK.IAM_EVENTS.gateway-cache-invalidator.>'
)
CONSUMER_SUB=(
  "_INBOX_GW.>"
)

# iam-provisioner — operator tooling only, NEVER deployed with a service.
# Deliberately not `$JS.API.>`: that wildcard includes STREAM.MSG.GET, which reads any message in
# the stream and would make this a full reader of the authorization change graph. `$JS.API.INFO`
# is account-tier info, which the `nats` CLI requests on startup.
PROVISIONER_PUB=(
  '$JS.API.INFO'
  '$JS.API.STREAM.>'
  '$JS.API.CONSUMER.>'
)
PROVISIONER_SUB=(
  "_INBOX_PROV.>"
)

# The durable's filter (D5). Revocations and grants change authz outcomes; `principal.created`
# and `api_key.issued` are excluded because nothing is cached about a principal or key that does
# not exist yet. SMA-492 may widen this — and can, without touching either service, because the
# filter lives in the provisioned durable rather than in a permission.
CONSUMER_FILTER_SUBJECTS=(
  "iam.role.granted"
  "iam.role.revoked"
  "iam.api_key.revoked"
  "iam.principal.archived"
  "iam.policy.put"
  "iam.policy.deleted"
)
```

- [ ] **Step 2: Write `ops/nats/test/accounts.conf.tmpl`**

```
# SPDX-License-Identifier: Apache-2.0
#
# Static-nkey mirror of the production account (SMA-493 D2), included by both fixture server
# configs so the plaintext and TLS runs cannot drift apart.
#
# The `{{…_NKEY}}` placeholders are rendered at test time with freshly minted public keys:
# committing fixed identities would mean committing their seeds, which are private keys. The
# permission lists — the part that actually matters — are committed verbatim and asserted against
# `subjects.env` by `check-subjects.sh`.
#
# Production uses operator-mode JWTs instead (see `provision.sh`); the subject lists are identical
# in both encodings.

accounts {
  SYS: {
    users: [ { nkey: "{{SYS_NKEY}}" } ]
  }

  PAIGASUS_IAM: {
    # max_mem 0 disables memory storage outright: the stream is File-backed (SMA-471 D8) and a
    # memory stream would silently lose everything on a broker restart.
    jetstream: { max_mem: 0, max_file: 536870912, max_streams: 4, max_consumers: 32 }

    users: [
      {
        nkey: "{{PUBLISHER_NKEY}}"
        permissions: {
          publish:   { allow: [ "iam.>", "$JS.API.STREAM.INFO.IAM_EVENTS", "$JS.API.STREAM.CREATE.IAM_EVENTS" ] }
          subscribe: { allow: [ "_INBOX_IAM_PUB.>" ] }
        }
      },
      {
        nkey: "{{CONSUMER_NKEY}}"
        permissions: {
          publish:   { allow: [ "$JS.API.CONSUMER.MSG.NEXT.IAM_EVENTS.gateway-cache-invalidator", "$JS.API.CONSUMER.INFO.IAM_EVENTS.gateway-cache-invalidator", "$JS.ACK.IAM_EVENTS.gateway-cache-invalidator.>" ] }
          subscribe: { allow: [ "_INBOX_GW.>" ] }
        }
      },
      {
        nkey: "{{PROVISIONER_NKEY}}"
        permissions: {
          publish:   { allow: [ "$JS.API.INFO", "$JS.API.STREAM.>", "$JS.API.CONSUMER.>" ] }
          subscribe: { allow: [ "_INBOX_PROV.>" ] }
        }
      }
    ]
  }
}

system_account: SYS
```

- [ ] **Step 3: Write the two fixture server configs**

`ops/nats/test/nats-server.conf`:

```
# SPDX-License-Identifier: Apache-2.0
# Plaintext fixture broker (SMA-493 §4.3). `accounts.conf` is rendered from
# `accounts.conf.tmpl` by the test and copied in alongside this file.
port: 4222
jetstream {
  store_dir: "/tmp/jetstream"
  max_memory_store: 0
  max_file_store: 536870912
}
include "accounts.conf"
```

`ops/nats/test/nats-server-tls.conf`:

```
# SPDX-License-Identifier: Apache-2.0
# TLS fixture broker (SMA-493 §4.4). Identical to `nats-server.conf` plus a tls block; the
# certificate and key are minted per-run by the test (rcgen) and copied in, so no key material is
# committed. Same `include`, so the permission lists cannot drift between the two fixtures.
port: 4222
jetstream {
  store_dir: "/tmp/jetstream"
  max_memory_store: 0
  max_file_store: 536870912
}
tls {
  cert_file: "/etc/nats/server-cert.pem"
  key_file:  "/etc/nats/server-key.pem"
}
include "accounts.conf"
```

- [ ] **Step 4: Write `ops/nats/check-subjects.sh`**

```sh
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Asserts the committed test fixture grants EXACTLY the subjects in `subjects.env` (SMA-493 D10).
#
# Why this exists: the permission lists live in two encodings — `provision.sh` (what deploys) and
# `accounts.conf.tmpl` (what the integration test proves). Without this gate the artifact that is
# PROVEN is not the artifact that is DEPLOYED, and the acceptance criterion would be satisfied by
# the wrong file.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "$here/subjects.env"

tmpl="$here/test/accounts.conf.tmpl"
fail=0

expect_present() {
  local subject="$1"
  if ! grep -qF -- "\"$subject\"" "$tmpl"; then
    echo "MISSING from accounts.conf.tmpl: $subject" >&2
    fail=1
  fi
}

for s in "${PUBLISHER_PUB[@]}" "${PUBLISHER_SUB[@]}" \
         "${CONSUMER_PUB[@]}" "${CONSUMER_SUB[@]}" \
         "${PROVISIONER_PUB[@]}" "${PROVISIONER_SUB[@]}"; do
  expect_present "$s"
done

# The other direction: every quoted subject inside an allow list must be accounted for, so a
# fixture cannot quietly grant something `subjects.env` never authorised.
declared=$(printf '%s\n' "${PUBLISHER_PUB[@]}" "${PUBLISHER_SUB[@]}" \
                          "${CONSUMER_PUB[@]}" "${CONSUMER_SUB[@]}" \
                          "${PROVISIONER_PUB[@]}" "${PROVISIONER_SUB[@]}" | sort -u)
granted=$(grep -oE 'allow: \[[^]]*\]' "$tmpl" | grep -oE '"[^"]+"' | tr -d '"' | sort -u)

while IFS= read -r s; do
  [ -z "$s" ] && continue
  if ! printf '%s\n' "$declared" | grep -qxF -- "$s"; then
    echo "UNDECLARED grant in accounts.conf.tmpl (not in subjects.env): $s" >&2
    fail=1
  fi
done <<< "$granted"

if [ "$fail" -ne 0 ]; then
  echo "ops/nats: accounts.conf.tmpl and subjects.env disagree" >&2
  exit 1
fi
echo "ops/nats: accounts.conf.tmpl grants exactly the subjects declared in subjects.env"
```

Make it executable: `chmod +x ops/nats/check-subjects.sh`

- [ ] **Step 5: Verify the gate passes and actually catches drift**

```bash
bash ops/nats/check-subjects.sh
```

Expected: `ops/nats: accounts.conf.tmpl grants exactly the subjects declared in subjects.env`

Now prove it is not vacuous — temporarily add `"iam.evil.>"` to the publisher's `allow:` list in `accounts.conf.tmpl`, re-run, confirm it fails with `UNDECLARED grant`, then revert.

- [ ] **Step 6: Write `ops/nats/provision.sh`**

```sh
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Provisions one environment's NATS account, users, stream and durable consumer (SMA-493).
# Run ONCE per environment, by an operator, from a machine that can reach the broker.
#
# Requires THREE tools — nsc alone cannot do this job:
#   nsc   — mints the operator, accounts, users and .creds files
#   nats  — creates the stream and the filtered durable consumer (a running-server operation)
#   a nats-server that loads the generated resolver config (see step 4 below)
#
# The subject lists come from `subjects.env`; edit them there, never here.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "$here/subjects.env"

OPERATOR="${OPERATOR:-paigasus}"
OUT_DIR="${OUT_DIR:-$here/out}"
NATS_URL="${NATS_URL:?set NATS_URL, e.g. tls://nats.internal:4222}"

mkdir -p "$OUT_DIR"

# --- 1. Operator + accounts -----------------------------------------------------------------
nsc add operator --name "$OPERATOR" --sys 2>/dev/null || echo "operator $OPERATOR already exists"
nsc add account --name PAIGASUS_IAM 2>/dev/null || echo "account PAIGASUS_IAM already exists"

# Account-level JetStream limits (SMA-493 D1). --js-mem-storage 0 disables memory storage: the
# stream is File-backed (SMA-471 D8) and a memory stream loses everything on a broker restart.
nsc edit account --name PAIGASUS_IAM \
  --js-mem-storage 0 \
  --js-disk-storage 10737418240 \
  --js-streams 4 \
  --js-consumer 32

# --- 2. Users -------------------------------------------------------------------------------
# `nsc add user` takes repeated --allow-pub / --allow-sub flags; the arrays are expanded one
# subject per flag so the lists stay declarative in subjects.env.
add_user() {
  local name="$1"; shift
  local -n pub_ref="$1"; shift
  local -n sub_ref="$1"; shift

  local args=()
  for s in "${pub_ref[@]}"; do args+=(--allow-pub "$s"); done
  for s in "${sub_ref[@]}"; do args+=(--allow-sub "$s"); done

  # No --expiry: a non-expiring user JWT cannot strand a long-running process on a reconnect
  # (SMA-493 §3.1). Rotation stays available — the service re-reads its .creds on every
  # connection attempt (D8) — it is simply not forced on a schedule. An operator who DOES set an
  # expiry takes on monitoring it; nothing here alerts on approaching expiry.
  nsc add user --account PAIGASUS_IAM --name "$name" "${args[@]}"
  nsc generate creds --account PAIGASUS_IAM --name "$name" > "$OUT_DIR/$name.creds"
  chmod 600 "$OUT_DIR/$name.creds"
}

add_user iam-publisher    PUBLISHER_PUB   PUBLISHER_SUB
add_user gateway-consumer CONSUMER_PUB    CONSUMER_SUB
add_user iam-provisioner  PROVISIONER_PUB PROVISIONER_SUB

# --- 3. Resolver config + push --------------------------------------------------------------
# The broker needs this stanza to validate the account JWTs minted above; `nsc push` uploads the
# account itself. Without both, every service authenticates against a server that has never heard
# of the account.
nsc generate config --nats-resolver > "$OUT_DIR/resolver.conf"
echo "include the generated $OUT_DIR/resolver.conf in the broker's nats-server.conf, restart it, then re-run with PUSH=1"
if [ "${PUSH:-0}" = "1" ]; then
  nsc push --account PAIGASUS_IAM
fi

# --- 4. Stream + durable (nats CLI, against the running broker) -----------------------------
# The stream config MUST match `PublisherConfig`'s defaults: SMA-471 D7 fails the service's boot
# when an adopted stream is weaker than configured, and retention/storage/duplicate_window are
# NOT editable in place — fixing drift means deleting the stream, i.e. a maintenance window.
# See rs/crates/services/paigasus-iam/src/config.rs (`impl Default for PublisherConfig`).
nats --server "$NATS_URL" --creds "$OUT_DIR/iam-provisioner.creds" \
  stream add "$STREAM_NAME" \
  --subjects "iam.>" \
  --storage file \
  --retention limits \
  --dupe-window 1h \
  --max-age 7d \
  --replicas 1 \
  --discard old \
  --max-msgs=-1 --max-bytes=-1 --max-msg-size=-1 --max-msgs-per-subject=-1 \
  --no-allow-rollup --no-deny-delete --no-deny-purge --defaults

# The durable carries the subject filter, because a pull consumer's permissions cannot (D5).
filter_csv=$(IFS=,; echo "${CONSUMER_FILTER_SUBJECTS[*]}")
nats --server "$NATS_URL" --creds "$OUT_DIR/iam-provisioner.creds" \
  consumer add "$STREAM_NAME" "$DURABLE_NAME" \
  --pull \
  --filter "$filter_csv" \
  --ack explicit \
  --deliver all \
  --max-deliver 5 \
  --defaults

echo "provisioned. Deploy iam-publisher.creds with paigasus-iam and gateway-consumer.creds with the gateway."
echo "KEEP iam-provisioner.creds OUT of any deployment — it can create and delete streams."
```

Make it executable: `chmod +x ops/nats/provision.sh`

- [ ] **Step 7: Write `ops/nats/permissions.md` and `ops/nats/README.md`**

`permissions.md` must contain, as prose an operator can act on:
1. The three users and their exact subject lists (referencing `subjects.env` as the source).
2. **Why the publisher needs a subscribe grant** — JetStream acks return on the inbox; without it every publish times out.
3. **Why subscribe permissions cannot narrow a consumer** — pull deliveries land on the inbox; the filter lives in the durable, and denying every `CONSUMER.CREATE` form is what makes it binding.
4. The four routes to the firehose that the allow-list closes: `sub iam.>`, a self-created wider consumer, `STREAM.MSG.GET`, `DIRECT.GET`.
5. **JetStream domains re-shape both prefixes**: acks become `$JS.ACK.<domain>.<account-hash>.<stream>.<consumer>.>` **and** the API moves to `$JS.<domain>.API.…`, so every grant shifts. Give the widened forms.
6. **`root_ca_bundle` replaces the system trust store** — concatenate every CA you need.
7. **The inbox-prefix coupling** — config and grant must match; a mismatch presents as publish timeouts. SMA-492 must set `custom_inbox_prefix` to `_INBOX_GW`.
8. **Credential delivery must be atomic** — a Kubernetes secret mount's symlink swap, or write-to-temp-then-`rename`. Never truncate-and-rewrite in place: the file is read mid-reconnect.
9. **`iam-provisioner.creds` is an operator artifact** — credential store, never a deployment secret.
10. The stream-config coupling: `provision.sh`'s stream values must match `PublisherConfig`'s defaults or the service crash-loops on SMA-471 D7 drift, and three of those fields are not editable in place.

`README.md` covers: what the directory is, the three-tool requirement, the run order (`provision.sh` → include resolver → restart broker → `PUSH=1 provision.sh`), the artifact index, and a pointer to `docs/ops/RUNBOOK-nats.md`.

- [ ] **Step 8: Commit**

```bash
git add ops/nats
git commit -m "chore(ops): add the nats account, permission and provisioning artifacts (SMA-493)"
```

---

### Task 6: The permission integration test

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/nats_permissions.rs`

**Interfaces:**
- Consumes: `EventType::ALL` (Task 1); `creds::auth_from_credentials` indirectly via `NatsEventPublisher::connect` (Tasks 2, 4); the fixture configs (Task 5).
- Produces: the test binary `nats_permissions` that Task 9's Moon task runs.

- [ ] **Step 1: Write the fixture harness and the first (sufficiency) test**

```rust
// SPDX-License-Identifier: Apache-2.0

//! NATS permission-set integration tests (SMA-493 §4.3).
//!
//! Boots a broker with the **committed** `ops/nats/test/` configuration and asserts the
//! `iam-publisher` and `gateway-consumer` permission sets are exactly sufficient and no broader.
//! The publisher side runs through `NatsEventPublisher` itself — not a hand-rolled client — which
//! is why the fixture's users are static **nkeys**: `auth_from_credentials` presents a bare seed
//! file as nkey auth (D2), so `credentials_file` stays in the loop.
//!
//! **Denials need the event callback.** A denied `subscribe` returns `Ok(Subscriber)` and a denied
//! `$JS.API` request simply never gets a reply, so "assert it was denied" is otherwise
//! indistinguishable from a broken fixture. Every negative case here asserts on the server's
//! asynchronous `Permissions Violation` text naming the exact subject (D9).

mod support;

use std::path::PathBuf;
use std::time::Duration;

use async_nats::jetstream;
use chrono::Utc;
use paigasus_iam::adapters::events::NatsEventPublisher;
use paigasus_iam::config::{PublisherBackend, PublisherConfig};
use paigasus_iam_core::{DomainEvent, EventPublisher, EventType};
use testcontainers::core::WaitFor;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::sync::mpsc;
use uuid::Uuid;

/// See `tests/nats_publisher.rs` — same load budget, same reasoning.
const CONTAINER_READY_BUDGET: Duration = Duration::from_secs(90);

/// Repo root, from this crate's manifest dir: `rs/crates/services/paigasus-iam` → four levels up.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..").canonicalize().expect("repo root resolves")
}

/// One rendered fixture identity: the seed file the adapter authenticates with, and the public
/// key the broker config declares.
struct Identity {
    seed_path: PathBuf,
    public_key: String,
}

/// Mints a user nkey and writes its seed as a bare seed file — the `.nk` shape
/// `parse_credentials` maps to nkey auth.
fn mint(dir: &std::path::Path, name: &str) -> Identity {
    let kp = nkeys::KeyPair::new_user();
    let seed = kp.seed().expect("a fresh keypair exposes its seed");
    let seed_path = dir.join(format!("{name}.nk"));
    std::fs::write(&seed_path, format!("-----BEGIN USER NKEY SEED-----\n{seed}\n------END USER NKEY SEED------\n")).expect("write seed");
    Identity { seed_path, public_key: kp.public_key() }
}

struct Fixture {
    _node: ContainerAsync<GenericImage>,
    _dir: tempfile::TempDir,
    url: String,
    publisher: Identity,
    consumer: Identity,
    provisioner: Identity,
}

/// Renders `accounts.conf.tmpl` with freshly minted identities and boots the broker with the
/// committed server config. `None` when Docker is unavailable outside CI.
async fn start_fixture(server_conf: &str, extra_files: Vec<(String, Vec<u8>)>) -> Option<Fixture> {
    let dir = tempfile::tempdir().expect("tempdir");
    let ops = repo_root().join("ops/nats/test");

    let publisher = mint(dir.path(), "iam-publisher");
    let consumer = mint(dir.path(), "gateway-consumer");
    let provisioner = mint(dir.path(), "iam-provisioner");
    let sys = mint(dir.path(), "sys");

    let rendered = std::fs::read_to_string(ops.join("accounts.conf.tmpl"))
        .expect("the committed accounts template must be readable")
        .replace("{{SYS_NKEY}}", &sys.public_key)
        .replace("{{PUBLISHER_NKEY}}", &publisher.public_key)
        .replace("{{CONSUMER_NKEY}}", &consumer.public_key)
        .replace("{{PROVISIONER_NKEY}}", &provisioner.public_key);

    let mut image = GenericImage::new("nats", "2.10.14")
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_copy_to("/etc/nats/accounts.conf", rendered.into_bytes())
        .with_copy_to("/etc/nats/nats-server.conf", std::fs::read(ops.join(server_conf)).expect("server conf"))
        .with_cmd(["-c", "/etc/nats/nats-server.conf"]);
    for (target, bytes) in extra_files {
        image = image.with_copy_to(target, bytes);
    }

    let node = match image.start().await {
        Ok(n) => n,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("Docker is required for the nats permission tests in CI: {e}");
            }
            eprintln!("skipping nats_permissions: Docker unavailable ({e})");
            return None;
        }
    };

    let deadline = std::time::Instant::now() + CONTAINER_READY_BUDGET;
    let port = loop {
        match node.get_host_port_ipv4(4222).await {
            Ok(p) => break p,
            Err(e) if std::time::Instant::now() >= deadline => panic!("nats port was never published: {e}"),
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    };

    Some(Fixture { _node: node, _dir: dir, url: format!("nats://127.0.0.1:{port}"), publisher, consumer, provisioner })
}

/// A publisher config pointed at the fixture, authenticating as `identity`.
fn cfg_for(fixture: &Fixture, identity: &Identity, inbox_prefix: &str) -> PublisherConfig {
    PublisherConfig {
        backend: PublisherBackend::Nats,
        url: Some(fixture.url.clone()),
        credentials_file: Some(identity.seed_path.to_string_lossy().to_string()),
        inbox_prefix: Some(inbox_prefix.to_string()),
        allow_insecure_broker: true,
        ..PublisherConfig::default()
    }
}

/// A raw client for `identity`, with its `Event`s piped into the returned channel so denials can
/// be asserted on rather than inferred from a timeout.
async fn client_for(fixture: &Fixture, identity: &Identity, inbox_prefix: &str) -> (async_nats::Client, mpsc::UnboundedReceiver<String>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let seed = std::fs::read_to_string(&identity.seed_path).expect("seed file");
    let client = async_nats::ConnectOptions::with_auth_callback(move |nonce| {
        let seed = seed.clone();
        async move {
            let parsed = paigasus_iam::adapters::events::parse_credentials(&seed).map_err(|e| async_nats::AuthError::new(e.to_string()))?;
            let mut auth = async_nats::Auth::new();
            auth.signature = Some(parsed.key_pair.sign(&nonce).map_err(|e| async_nats::AuthError::new(e.to_string()))?);
            auth.nkey = Some(parsed.key_pair.public_key());
            Ok(auth)
        }
    })
    .custom_inbox_prefix(inbox_prefix.to_string())
    .event_callback(move |event| {
        let tx = tx.clone();
        async move {
            let _ = tx.send(event.to_string());
        }
    })
    .connect(&fixture.url)
    .await
    .expect("fixture client connects");
    (client, rx)
}

/// Waits for a server error naming `subject`, or panics — the positive form of "this was denied".
async fn expect_permissions_violation(rx: &mut mpsc::UnboundedReceiver<String>, subject: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(event)) if event.to_lowercase().contains("permissions violation") && event.contains(subject) => return,
            Ok(Some(_)) => continue,
            Ok(None) => panic!("event stream closed before a permissions violation for {subject}"),
            Err(_) => panic!("no permissions violation for {subject} within 10s — the grant is WIDER than intended"),
        }
    }
}

/// `id` is caller-supplied and must be distinct per publish: it becomes `Nats-Msg-Id`, and
/// JetStream would otherwise collapse two events into one dedup hit. `Uuid::from_u128` rather
/// than a v4/v7 constructor because the workspace pins `uuid` with **no features** (a v7 rng
/// pulls `getrandom`, which the wasm binding's `repo:wasm-getrandom-free` gate forbids).
fn event(id: u128, et: EventType) -> DomainEvent {
    DomainEvent {
        id: Uuid::from_u128(id),
        event_type: et,
        schema_version: 1,
        aggregate_prn: "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string(),
        actor_prn: None,
        occurred_at: Utc::now(),
        payload: serde_json::json!({"kind": "user"}),
        correlation_id: None,
    }
}

/// Sufficiency: the committed publisher grant must cover stream ensure plus a publish on EVERY
/// subject this service can emit. Iterating `EventType::ALL` (SMA-493 §3.4) is what makes a
/// ninth event type this test's business.
#[tokio::test]
async fn the_publisher_grant_covers_ensure_and_every_event_subject() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let publisher = NatsEventPublisher::connect(&cfg_for(&fixture, &fixture.publisher, "_INBOX_IAM_PUB"))
        .await
        .expect("the committed publisher grant must cover get_or_create_stream and its config verification");

    for (i, et) in EventType::ALL.into_iter().enumerate() {
        publisher
            .publish(&event(i as u128 + 1, et))
            .await
            .unwrap_or_else(|e| panic!("publishing {} must be permitted: {e}", et.as_wire()));
    }
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test nats_permissions
```

Expected: FAIL first on compilation (`nkeys` / `tempfile` may need to be dev-visible — `nkeys` is already a normal dependency so it is visible to tests; add `tempfile = "3"` to `[dev-dependencies]` if step 2 of Task 2 did not). Then iterate until the fixture boots: the two most likely failures are the `WaitFor` string not appearing (check `docker logs` of a manual run: `docker run --rm -v …:/etc/nats nats:2.10.14 -c /etc/nats/nats-server.conf`) and JetStream failing to create `/tmp/jetstream` (adjust `store_dir` in the fixture configs if the image's user cannot write it).

- [ ] **Step 3: Add the publisher denial tests**

```rust
/// The publisher must not be able to READ the graph it writes — the whole point of SMA-493 §1.1.
#[tokio::test]
async fn the_publisher_cannot_subscribe_to_the_event_stream() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let (client, mut events) = client_for(&fixture, &fixture.publisher, "_INBOX_IAM_PUB").await;

    // A denied subscribe still returns Ok: the refusal arrives asynchronously.
    let _sub = client.subscribe("iam.>").await.expect("subscribe is accepted locally, then refused by the server");
    expect_permissions_violation(&mut events, "iam.>").await;
}

/// SMA-471 D7 made non-reconciliation deliberate; these two grants are that decision enforced at
/// the broker rather than merely intended in the code.
#[tokio::test]
async fn the_publisher_cannot_delete_or_purge_the_stream() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    NatsEventPublisher::connect(&cfg_for(&fixture, &fixture.publisher, "_INBOX_IAM_PUB")).await.expect("ensure the stream first");
    let (client, mut events) = client_for(&fixture, &fixture.publisher, "_INBOX_IAM_PUB").await;

    client.publish("$JS.API.STREAM.DELETE.IAM_EVENTS", "".into()).await.expect("published locally");
    expect_permissions_violation(&mut events, "$JS.API.STREAM.DELETE.IAM_EVENTS").await;

    client.publish("$JS.API.STREAM.PURGE.IAM_EVENTS", "".into()).await.expect("published locally");
    expect_permissions_violation(&mut events, "$JS.API.STREAM.PURGE.IAM_EVENTS").await;
}

/// The third firehose route: direct message get reads any message in the stream regardless of any
/// consumer filter, so it must be denied to both service identities.
#[tokio::test]
async fn neither_service_identity_can_direct_get_stream_messages() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    NatsEventPublisher::connect(&cfg_for(&fixture, &fixture.publisher, "_INBOX_IAM_PUB")).await.expect("ensure the stream first");

    for (identity, prefix) in [(&fixture.publisher, "_INBOX_IAM_PUB"), (&fixture.consumer, "_INBOX_GW")] {
        let (client, mut events) = client_for(&fixture, identity, prefix).await;
        client.publish("$JS.API.STREAM.MSG.GET.IAM_EVENTS", "{}".into()).await.expect("published locally");
        expect_permissions_violation(&mut events, "$JS.API.STREAM.MSG.GET.IAM_EVENTS").await;

        client.publish("$JS.API.DIRECT.GET.IAM_EVENTS", "{}".into()).await.expect("published locally");
        expect_permissions_violation(&mut events, "$JS.API.DIRECT.GET.IAM_EVENTS").await;
    }
}
```

- [ ] **Step 4: Add the consumer tests**

```rust
/// Provisions the stream and the filtered durable exactly as `ops/nats/provision.sh` does, using
/// the provisioner identity — because neither service identity can, which is the point of D5.
async fn provision(fixture: &Fixture) {
    let (client, _events) = client_for(fixture, &fixture.provisioner, "_INBOX_PROV").await;
    let js = jetstream::new(client);
    let stream = js
        .get_or_create_stream(jetstream::stream::Config {
            name: "IAM_EVENTS".to_string(),
            subjects: vec!["iam.>".to_string()],
            retention: jetstream::stream::RetentionPolicy::Limits,
            storage: jetstream::stream::StorageType::File,
            duplicate_window: Duration::from_secs(3_600),
            max_age: Duration::from_secs(604_800),
            num_replicas: 1,
            ..Default::default()
        })
        .await
        .expect("the provisioner grant must cover stream creation");

    stream
        .get_or_create_consumer(
            "gateway-cache-invalidator",
            jetstream::consumer::pull::Config {
                durable_name: Some("gateway-cache-invalidator".to_string()),
                filter_subjects: vec![
                    "iam.role.granted".to_string(),
                    "iam.role.revoked".to_string(),
                    "iam.api_key.revoked".to_string(),
                    "iam.principal.archived".to_string(),
                    "iam.policy.put".to_string(),
                    "iam.policy.deleted".to_string(),
                ],
                ..Default::default()
            },
        )
        .await
        .expect("the provisioner grant must cover consumer creation");
}

/// Sufficiency for SMA-492: pull from the provisioned durable and ack.
#[tokio::test]
async fn the_consumer_grant_covers_pulling_and_acking_from_the_provisioned_durable() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    provision(&fixture).await;

    let publisher = NatsEventPublisher::connect(&cfg_for(&fixture, &fixture.publisher, "_INBOX_IAM_PUB")).await.expect("connect publisher");
    publisher.publish(&event(1, EventType::RoleRevoked)).await.expect("publish a filtered event");

    let (client, _events) = client_for(&fixture, &fixture.consumer, "_INBOX_GW").await;
    let js = jetstream::new(client);
    let consumer: jetstream::consumer::PullConsumer = js
        .get_consumer_from_stream("gateway-cache-invalidator", "IAM_EVENTS")
        .await
        .expect("the consumer grant must cover CONSUMER.INFO on its own durable");

    let mut batch = consumer.fetch().max_messages(1).messages().await.expect("the consumer grant must cover MSG.NEXT");
    let msg = tokio::time::timeout(Duration::from_secs(10), futures::StreamExt::next(&mut batch))
        .await
        .expect("a filtered event must be delivered")
        .expect("stream yields a message")
        .expect("message is Ok");
    assert_eq!(msg.subject.as_str(), "iam.role.revoked");
    msg.ack().await.expect("the consumer grant must cover $JS.ACK on its own durable");
}

/// The control that makes D5's filter binding. A consumer that can CREATE can set its own
/// `filter_subjects` and read everything — so the NAMED form must be denied, not merely the bare
/// one: async-nats builds `CONSUMER.CREATE.{stream}.{name}` (`context.rs:1512`), and a grant
/// written as `CONSUMER.CREATE.*` would match neither that nor the legacy DURABLE form.
#[tokio::test]
async fn the_consumer_cannot_create_a_wider_consumer_in_any_form() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    provision(&fixture).await;
    let (client, mut events) = client_for(&fixture, &fixture.consumer, "_INBOX_GW").await;

    for subject in [
        "$JS.API.CONSUMER.CREATE.IAM_EVENTS.wide-open",
        "$JS.API.CONSUMER.CREATE.IAM_EVENTS",
        "$JS.API.CONSUMER.DURABLE.CREATE.IAM_EVENTS.wide-open",
    ] {
        client.publish(subject, "{}".into()).await.expect("published locally");
        expect_permissions_violation(&mut events, subject).await;
    }
}

#[tokio::test]
async fn the_consumer_cannot_subscribe_to_or_forge_events() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let (client, mut events) = client_for(&fixture, &fixture.consumer, "_INBOX_GW").await;

    let _sub = client.subscribe("iam.>").await.expect("accepted locally, refused by the server");
    expect_permissions_violation(&mut events, "iam.>").await;

    client.publish("iam.role.granted", "{}".into()).await.expect("published locally");
    expect_permissions_violation(&mut events, "iam.role.granted").await;
}
```

Add `futures = "0.3"` to `[dev-dependencies]` if absent (`grep '^futures' rs/crates/services/paigasus-iam/Cargo.toml`) — `StreamExt::next` needs it.

- [ ] **Step 5: Run the full file**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test nats_permissions
```

Expected: all tests PASS. If a denial test times out, the grant is **wider than intended** — fix `accounts.conf.tmpl` and `subjects.env` together, then re-run `bash ops/nats/check-subjects.sh`.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/nats_permissions.rs rs/crates/services/paigasus-iam/Cargo.toml rs/Cargo.lock
git commit -m "test(rs): prove the committed nats permission sets are sufficient and narrow (SMA-493)"
```

---

### Task 7: The TLS end-to-end test

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/nats_permissions.rs`

**Interfaces:**
- Consumes: `start_fixture` (Task 6), `PublisherConfig::root_ca_bundle` (Task 3).

- [ ] **Step 1: Write the failing tests**

Append to `tests/nats_permissions.rs`:

```rust
/// Mints a CA and a server certificate signed by it, with an IP SAN for 127.0.0.1 (the tests dial
/// a mapped host port). Nothing is committed: `rcgen` is already a dev-dependency here for the
/// mock IdP, and a per-run key pair keeps certificate material out of git entirely.
fn mint_tls(dir: &std::path::Path) -> (Vec<u8>, Vec<u8>, PathBuf) {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, SanType};
    use std::net::{IpAddr, Ipv4Addr};

    let mut ca_params = CertificateParams::new(Vec::new()).expect("ca params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.distinguished_name.push(DnType::CommonName, "paigasus-nats-test-ca");
    let ca_key = KeyPair::generate().expect("ca key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("self-signed ca");

    let mut srv_params = CertificateParams::new(vec!["localhost".to_string()]).expect("server params");
    srv_params.subject_alt_names.push(SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    let srv_key = KeyPair::generate().expect("server key");
    let srv_cert = srv_params.signed_by(&srv_key, &ca_cert, &ca_key).expect("server cert signed by the ca");

    let ca_path = dir.join("ca.pem");
    std::fs::write(&ca_path, ca_cert.pem()).expect("write ca pem");
    (srv_cert.pem().into_bytes(), srv_key.serialize_pem().into_bytes(), ca_path)
}

/// D7's field is what makes a private-CA broker dialable at all — without it async-nats falls back
/// to the system trust store (`tls.rs:61`), which will never contain a per-run CA.
#[tokio::test]
async fn the_publisher_connects_over_tls_with_a_named_ca_bundle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (cert_pem, key_pem, ca_path) = mint_tls(dir.path());
    let extra = vec![("/etc/nats/server-cert.pem".to_string(), cert_pem), ("/etc/nats/server-key.pem".to_string(), key_pem)];

    let Some(fixture) = start_fixture("nats-server-tls.conf", extra).await else { return };

    let mut cfg = cfg_for(&fixture, &fixture.publisher, "_INBOX_IAM_PUB");
    cfg.url = Some(fixture.url.replace("nats://", "tls://"));
    cfg.root_ca_bundle = Some(ca_path.to_string_lossy().to_string());

    let publisher = NatsEventPublisher::connect(&cfg).await.expect("a tls:// connection with a named CA must succeed");
    publisher.publish(&event(1, EventType::RoleGranted)).await.expect("publish over TLS");
}

/// The negative control. Without it the test above would pass even if `root_ca_bundle` were
/// ignored entirely and verification silently disabled.
#[tokio::test]
async fn a_tls_connection_without_the_ca_bundle_fails_verification() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (cert_pem, key_pem, _ca_path) = mint_tls(dir.path());
    let extra = vec![("/etc/nats/server-cert.pem".to_string(), cert_pem), ("/etc/nats/server-key.pem".to_string(), key_pem)];

    let Some(fixture) = start_fixture("nats-server-tls.conf", extra).await else { return };

    let mut cfg = cfg_for(&fixture, &fixture.publisher, "_INBOX_IAM_PUB");
    cfg.url = Some(fixture.url.replace("nats://", "tls://"));
    // root_ca_bundle deliberately unset: the per-run CA is in no system trust store.
    NatsEventPublisher::connect(&cfg).await.expect_err("a private CA must not verify against the system trust store");
}

/// And a bundle that is well-formed but wrong must also fail — proving the bundle is actually
/// consulted rather than merely present.
#[tokio::test]
async fn a_tls_connection_with_an_unrelated_ca_bundle_fails_verification() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (cert_pem, key_pem, _ca_path) = mint_tls(dir.path());
    let extra = vec![("/etc/nats/server-cert.pem".to_string(), cert_pem), ("/etc/nats/server-key.pem".to_string(), key_pem)];

    let Some(fixture) = start_fixture("nats-server-tls.conf", extra).await else { return };

    let other = tempfile::tempdir().expect("tempdir");
    let (_c, _k, unrelated_ca) = mint_tls(other.path());

    let mut cfg = cfg_for(&fixture, &fixture.publisher, "_INBOX_IAM_PUB");
    cfg.url = Some(fixture.url.replace("nats://", "tls://"));
    cfg.root_ca_bundle = Some(unrelated_ca.to_string_lossy().to_string());
    NatsEventPublisher::connect(&cfg).await.expect_err("an unrelated CA must not verify the broker's certificate");
}
```

- [ ] **Step 2: Run them**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test nats_permissions tls
```

Expected: three PASS. If the broker refuses to start, check that `nats-server-tls.conf`'s `cert_file`/`key_file` paths match the copy targets exactly.

- [ ] **Step 3: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/nats_permissions.rs
git commit -m "test(rs): prove tls connections verify against the configured ca bundle (SMA-493)"
```

---

### Task 8: The rotation test

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/nats_permissions.rs`

**Interfaces:**
- Consumes: `start_fixture`, `cfg_for` (Task 6).

- [ ] **Step 1: Write the discriminating test**

The point of this test is that it **fails against the previous implementation**. `with_credentials_file` caches the credential at connect, so corrupting the file afterwards changes nothing; `with_auth_callback` re-reads it and the reconnect fails.

```rust
/// SMA-493 D8's regression net, and it must be able to detect the regression: corrupt the
/// credential AFTER a successful connect, force a reconnect, and require the reconnect to fail.
///
/// Under the pre-SMA-493 `with_credentials_file` this assertion is FALSE — the credential was
/// cached at connect and the reconnect succeeds regardless of what the file now says. A weaker
/// version of this test ("restart, publish again, expect success") passes identically before and
/// after the change and would have shipped the fix with a net that cannot catch its own removal.
#[tokio::test]
async fn a_corrupted_credential_is_noticed_on_reconnect() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let cfg = cfg_for(&fixture, &fixture.publisher, "_INBOX_IAM_PUB");

    let publisher = NatsEventPublisher::connect(&cfg).await.expect("first connect");
    publisher.publish(&event(1, EventType::RoleGranted)).await.expect("first publish");

    // Rotate to garbage, then force a fresh connection attempt by constructing a new publisher —
    // which is the same code path a reconnect takes, since the auth callback is per-attempt.
    std::fs::write(&fixture.publisher.seed_path, "not a credential any more").expect("overwrite the credential");

    let err = NatsEventPublisher::connect(&cfg).await.expect_err("a corrupted credential must not be papered over by a cached one");
    assert!(format!("{err}").contains(&fixture.publisher.seed_path.to_string_lossy().to_string()), "the error must name the path: {err}");
}

/// The happy half: rotating to a DIFFERENT VALID credential is picked up without a restart. The
/// broker knows both public keys (the fixture declares three users), so authenticating as the
/// provisioner after rotation proves the new file was read rather than the old one replayed.
#[tokio::test]
async fn a_rotated_credential_is_picked_up_without_a_restart() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let cfg = cfg_for(&fixture, &fixture.publisher, "_INBOX_IAM_PUB");
    NatsEventPublisher::connect(&cfg).await.expect("first connect as the publisher");

    // Same path, different identity: the provisioner's seed, which the fixture also declares.
    let provisioner_seed = std::fs::read_to_string(&fixture.provisioner.seed_path).expect("provisioner seed");
    std::fs::write(&fixture.publisher.seed_path, &provisioner_seed).expect("rotate the credential in place");

    let (client, _events) = {
        let seed = provisioner_seed.clone();
        let client = async_nats::ConnectOptions::with_auth_callback(move |nonce| {
            let seed = seed.clone();
            async move {
                let parsed = paigasus_iam::adapters::events::parse_credentials(&seed).map_err(|e| async_nats::AuthError::new(e.to_string()))?;
                let mut auth = async_nats::Auth::new();
                auth.signature = Some(parsed.key_pair.sign(&nonce).map_err(|e| async_nats::AuthError::new(e.to_string()))?);
                auth.nkey = Some(parsed.key_pair.public_key());
                Ok(auth)
            }
        })
        .custom_inbox_prefix("_INBOX_PROV".to_string())
        .connect(&fixture.url)
        .await
        .expect("the rotated credential authenticates");
        (client, ())
    };

    // The rotated identity has the provisioner's grants, not the publisher's: it can reach
    // STREAM.INFO, which is proof the NEW file was used.
    let js = jetstream::new(client);
    js.get_or_create_stream(jetstream::stream::Config {
        name: "IAM_EVENTS".to_string(),
        subjects: vec!["iam.>".to_string()],
        retention: jetstream::stream::RetentionPolicy::Limits,
        storage: jetstream::stream::StorageType::File,
        duplicate_window: Duration::from_secs(3_600),
        max_age: Duration::from_secs(604_800),
        num_replicas: 1,
        ..Default::default()
    })
    .await
    .expect("the rotated (provisioner) identity must be in force");
}
```

- [ ] **Step 2: Run it**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test nats_permissions credential
```

Expected: both PASS.

- [ ] **Step 3: Prove the test is discriminating**

Temporarily revert `connect`'s auth wiring to `ConnectOptions::with_credentials_file(path).await?` (dropping the pre-flight), re-run `a_corrupted_credential_is_noticed_on_reconnect`, and **confirm it fails**. Then restore the callback implementation. A test that passes under both implementations is not a regression net — this step is the whole reason Task 8 exists as its own task.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/nats_permissions.rs
git commit -m "test(rs): prove a rotated nats credential is re-read on reconnect (SMA-493)"
```

---

### Task 9: Wire the CI gate

**Files:**
- Modify: `moon.yml` (root `repo` project tasks), `.github/workflows/ci.yml:184`

**Interfaces:**
- Consumes: the `nats_permissions` test binary (Tasks 6-8), `ops/nats/check-subjects.sh` (Task 5).

- [ ] **Step 1: Add the Moon task**

In `moon.yml`, after the `observability-drift` task:

```yaml
  nats-permissions:
    description: 'Assert the committed ops/nats permission set is exactly sufficient for the IAM publisher and the gateway consumer, and no broader (SMA-493).'
    # Duplicates paigasus-iam-rs:test ON PURPOSE, same reasoning as observability-drift above:
    # `ops/` has no moon.yml so it belongs to the root `repo` project, while the crate's `test`
    # inputs are project-relative — an ops-only change (which is exactly how a permission set
    # gets loosened) would otherwise never make the crate affected, and the guard would not run
    # on the PRs that need it.
    script: |
      bash ops/nats/check-subjects.sh
      ( cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --test nats_permissions )
    toolchain: 'system'
    # Narrow inputs — `repo` owns the whole tree, so without these the guard runs on every change.
    inputs:
      - 'ops/nats/**/*'
      - 'rs/crates/services/paigasus-iam/tests/nats_permissions.rs'
      - 'rs/crates/services/paigasus-iam/tests/support/**/*'
      - 'rs/crates/services/paigasus-iam/src/adapters/events/**/*'
      - 'rs/crates/services/paigasus-iam/src/config.rs'
      - 'rs/crates/services/paigasus-iam/Cargo.toml'
      - 'rs/Cargo.lock'
```

- [ ] **Step 2: Enlist it in CI**

Defining a Moon task does not run it: `.github/workflows/ci.yml:184` drives a hardcoded array. Add `:nats-permissions` after `:observability-drift`:

```yaml
          T=(:build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts)
```

- [ ] **Step 3: Verify the task runs and is correctly scoped**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:nats-permissions
```

Expected: the subject gate prints its success line, then the integration tests pass.

Then confirm an ops-only edit actually selects it (this is the failure mode the task exists for):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
touch ops/nats/subjects.env
moon query tasks --affected --base origin/main < /dev/null | grep nats-permissions
```

Expected: the task is listed.

- [ ] **Step 4: Commit**

```bash
git add moon.yml .github/workflows/ci.yml
git commit -m "ci: gate the nats permission set on every ops or adapter change (SMA-493)"
```

---

### Task 10: Documentation

**Files:**
- Create: `docs/ops/RUNBOOK-nats.md`
- Modify: `docs/dev-setup.md` (the NATS section, ≈68-105), `docs/ops/RUNBOOK-observability.md` (the "NATS backend: boot hard-fails…" section, ≈542)

- [ ] **Step 1: Write `docs/ops/RUNBOOK-nats.md`**

Follow `RUNBOOK-observability.md`'s house style — symptom, cause, remediation per section. Cover:

1. **A denied publish looks like a timeout.** The server answers with an asynchronous `-ERR 'Permissions Violation for Publish to …'`; the request itself just expires after `publish_timeout_secs`. Grep the service log for `nats server error` (D9's callback) to get the refused subject. Cross-reference `IamOutboxPublishFailures`.
2. **Publishes time out with no server error at all** — the inbox-prefix mismatch. `[outbox.publisher].inbox_prefix` must equal the account's `subscribe` grant (`_INBOX_IAM_PUB` in `ops/nats/subjects.env`). The ack has nowhere to land.
3. **Boot fails with `jetstream stream IAM_EVENTS could not be ensured`** — the publisher's `$JS.API.STREAM.INFO` grant is missing, so the probe is refused rather than answered with a 404, and `get_or_create_stream` never reaches its create path.
4. **Boot fails naming the credentials file** — `Credentials` (unreadable/absent) vs `CredentialsParse` (present but not a `.creds`). Check the mount, then the file's two blocks.
5. **Reconnects fail after a credential rotation** — the new file is read on every attempt (D8), so this means the *new* credential is bad or its user was removed from the account. Roll back the file; no restart needed either way.
6. **TLS handshake fails after a broker certificate change** — `root_ca_bundle` **replaces** the system trust store. If the broker moved to a public CA, either append that CA to the bundle or unset the field.
7. **Publishes fail with `insufficient resources`** — the account's JetStream `max_file` limit (10GB by default from `provision.sh`). Raise the account limit or shorten `max_age_secs`.
8. **A JetStream domain was introduced** — every `$JS.API.*` grant and the `$JS.ACK` grant shift; see `ops/nats/permissions.md` for the widened forms.

- [ ] **Step 2: Cross-link from the observability runbook**

In `docs/ops/RUNBOOK-observability.md`, at the end of the "NATS backend: boot hard-fails on an unreachable broker or a drifted stream (SMA-471)" section:

```markdown
> **Permissions, TLS and credentials** have their own runbook: [`RUNBOOK-nats.md`](./RUNBOOK-nats.md)
> (SMA-493). A denied publish presents as a *timeout*, not an error, so it does not look like
> anything in this section.
```

- [ ] **Step 3: Update `docs/dev-setup.md`**

Replace the local config snippet in the NATS section with:

````markdown
```toml
[outbox.publisher]
backend = "nats"
url     = "nats://127.0.0.1:4222"
# Required for a local broker: the nats backend otherwise demands tls:// AND a credentials_file
# (SMA-493). This one flag waives both — it legalises an unauthenticated broker as well as an
# unencrypted one, which is why it is not called `allow_plaintext`. Never set it in a deployment.
allow_insecure_broker = true
```

For the production shape — a dedicated account, least-privilege subject permissions, TLS and
`.creds` — see [`ops/nats/README.md`](../ops/nats/README.md) and
[`docs/ops/RUNBOOK-nats.md`](./ops/RUNBOOK-nats.md).
````

- [ ] **Step 4: Run the full gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: all green. On an unattributed "1 failed", read `.moon/cache/ciReport.json`:
`jq '.actions[]|select(.status=="failed")' .moon/cache/ciReport.json`

- [ ] **Step 5: Commit**

```bash
git add docs/ops/RUNBOOK-nats.md docs/ops/RUNBOOK-observability.md docs/dev-setup.md
git commit -m "docs(rs): add the nats permissions, tls and credential runbook (SMA-493)"
```

---

## Post-implementation

**ADR-0016 (Notion, *Development → Architecture Decision Records*)** — update its Consequences:
the account model (D1), the rotation question moved from open to answered (§1.3, D8), subject
permissions moved from "a deployment requirement" to "provisioned and tested here", and the
residual absence of an account-JWT revocation and operator-key-rotation story. This is a Notion
edit, not a repo change, and it is the one deliverable no CI gate can catch.
