# SMA-570 CA-bundle doc scope and boot diagnostics — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Narrow five over-broad CA-bundle trust claims from process scope to client scope, and make a structurally invalid certificate in `extra_ca_bundle_path` name its config key at boot instead of blaming the platform trust store.

**Architecture:** When a `reqwest::Client` build fails and a bundle was configured, a *control* build with the same options but no added anchors decides attribution. Control succeeds → the anchors caused it. Control fails → the platform store is broken, and because reqwest adds user roots first and short-circuits, the bundle may be broken too. The decision is a pure function over an injected probe closure, so both failure arms are reachable in tests without touching host state.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), `reqwest` 0.12.28 with `rustls-tls` + `rustls-tls-native-roots`, `thiserror`, `tracing`, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-09-03-sma-570-ca-bundle-doc-and-diagnostics-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Conventional commits with a workspace scope from `[rs, py, ts, contracts, ci, docs, deps, release, repo, claude, workspace]` — use `rs` here. Never `--no-verify`.
- Rust edition 2024. `std::env::set_var` is `unsafe` — **no test may mutate `SSL_CERT_FILE`/`SSL_CERT_DIR`** or depend on host trust-store state.
- The two services' bundle handling stays **duplicated, not extracted** (SMA-558 D7). `Attribution`, `attribute_build_failure` and `base_builder` are written once per service.
- **No message string may quote an error-registry code spelling** (`upstream-unavailable`, `internal`, …). `ci/error-registry/check.py` scans `rs/crates/**/src/**/*.rs` and neither file has a MANIFEST row, so a quoted code reds `repo:error-code-single-site`.
- `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs` is **not** touched — that keeps `repo:http-extractor-envelope` out of the affected set.
- Narrowing phrase, used verbatim at every site: **"for every request this client makes, to any host it reaches"**. Never narrow to "the IdP connection" — that under-claims (the client follows redirects and dials whatever `jwks_uri` names).
- Every "ROOTS ONLY" / unconstrained-anchor warning stays. Only the *scope* changes.
- Run all cargo commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` first.
- **Run cargo from `rs/`, never from the worktree root with `--manifest-path`.** rustup reads
  `rust-toolchain.toml` from the **current working directory**, so a `--manifest-path rs/Cargo.toml`
  invocation launched at the root silently resolves the host default toolchain (measured: rustc
  1.98.0) instead of the pinned `channel = "1.95.0"`. That yields phantom clippy findings from
  lints that do not exist in 1.95, and is the same build-vs-lint skew SMA-389 recorded. `moon ci`
  is the exception — it runs from the repo root.

---

### Task 1: Narrow the trust-scope claim at all five sites

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs:136-138`
- Modify: `rs/crates/services/paigasus-gateway/src/config.rs:116-117`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs:135-138`
- Modify: `docs/ops/RUNBOOK-containers.md:257-258`
- Modify: `rs/crates/services/paigasus-iam/iam.toml.example:30-31`
- Modify: `rs/crates/services/paigasus-gateway/gateway.toml.example:53-57` (additive)

**Interfaces:**
- Consumes: nothing.
- Produces: nothing. Documentation only — no code, no tests.

- [ ] **Step 1: Confirm the five sites are exactly where the plan says**

```bash
grep -rn "process makes\|outbound HTTPS call" \
  rs/crates/services/paigasus-iam/src/config.rs \
  rs/crates/services/paigasus-gateway/src/config.rs \
  rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs \
  rs/crates/services/paigasus-iam/iam.toml.example \
  docs/ops/RUNBOOK-containers.md
```

Expected: exactly 5 matches, one per file. If the count differs, STOP and report — the site inventory has drifted.

- [ ] **Step 2: Narrow site 1 — IAM config.rs**

Replace:

```rust
    /// **ROOTS ONLY.** Every certificate here becomes an UNCONSTRAINED trust anchor for every
    /// HTTPS call this process makes — rustls performs no `cA` basic-constraints check on an
    /// anchor. An intermediate placed here is silently promoted to a root.
```

With:

```rust
    /// **ROOTS ONLY.** Every certificate here becomes an UNCONSTRAINED trust anchor for every
    /// request this client makes, to any host it reaches — rustls performs no `cA`
    /// basic-constraints check on an anchor. An intermediate placed here is silently promoted to
    /// a root.
    ///
    /// The anchors go onto the JWKS fetcher's own `reqwest::ClientBuilder`, NOT the whole
    /// process: the gRPC (`tonic`), NATS and Redis links each build their own TLS config and
    /// never consult them (SMA-570).
```

- [ ] **Step 3: Narrow site 2 — gateway config.rs**

Replace:

```rust
    /// **ROOTS ONLY** — every certificate here becomes an unconstrained trust anchor for every
    /// HTTPS call this process makes. Read once at boot; an unreadable, malformed or
    /// certificate-free bundle is a hard boot failure. Mirrors
    /// `paigasus-iam`'s `authn.extra_ca_bundle_path`.
```

With:

```rust
    /// **ROOTS ONLY** — every certificate here becomes an unconstrained trust anchor for every
    /// request this client makes, to any host it reaches. The anchors go onto the OpenAI egress
    /// client's own `reqwest::ClientBuilder`, NOT the whole process: the IAM `tonic` link builds
    /// its own TLS config and never consults them (SMA-570). Read once at boot; an unreadable,
    /// malformed or certificate-free bundle is a hard boot failure. Mirrors
    /// `paigasus-iam`'s `authn.extra_ca_bundle_path`.
```

- [ ] **Step 4: Narrow site 3 — jwks.rs inner comment**

Replace:

```rust
                // `from_pem_bundle`, NOT `from_pem`: a bundle may legitimately carry more than one
                // ROOT (a cross-signed CA, or two corporate roots mid-rotation) and `from_pem`
                // reads only the first. This is NOT an invitation to add intermediates — every
                // certificate here becomes an UNCONSTRAINED trust anchor (rustls performs no `cA`
                // basic-constraints check on an anchor), so an intermediate would be promoted to a
                // root for every HTTPS call this process makes.
```

With:

```rust
                // `from_pem_bundle`, NOT `from_pem`: a bundle may legitimately carry more than one
                // ROOT (a cross-signed CA, or two corporate roots mid-rotation) and `from_pem`
                // reads only the first. This is NOT an invitation to add intermediates — every
                // certificate here becomes an UNCONSTRAINED trust anchor (rustls performs no `cA`
                // basic-constraints check on an anchor), so an intermediate would be promoted to a
                // root for every request THIS CLIENT makes, to any host it reaches (SMA-570).
```

- [ ] **Step 5: Narrow site 4 — RUNBOOK**

Replace:

```markdown
  **Put roots in the bundle, never intermediates.** Every certificate in it becomes an
  unconstrained trust anchor for every outbound HTTPS call the process makes — TLS performs no
  `cA` check on an anchor, so an intermediate is silently promoted to a root.
```

With:

```markdown
  **Put roots in the bundle, never intermediates.** Every certificate in it becomes an
  unconstrained trust anchor for every request the client that loaded it makes, to any host it
  reaches — TLS performs no `cA` check on an anchor, so an intermediate is silently promoted to a
  root. The bundle is scoped to one client, not the whole process: IAM's is the JWKS fetcher's and
  the gateway's is the OpenAI egress client's, while the gRPC, NATS and Redis links each build
  their own TLS config and never consult it.
```

- [ ] **Step 6: Narrow site 5 — iam.toml.example (the operator-facing one)**

Replace:

```
# the `extra_` prefix. ROOTS ONLY: every certificate here becomes an unconstrained trust anchor
# for every outbound HTTPS call. Read once at boot, so a rotated bundle needs a restart. Cannot
```

With:

```
# the `extra_` prefix. ROOTS ONLY: every certificate here becomes an unconstrained trust anchor
# for every request the IdP discovery/JWKS client makes, to any host it reaches — not for the
# whole process. Read once at boot, so a rotated bundle needs a restart. Cannot
```

- [ ] **Step 7: Add the matching sentence to gateway.toml.example (additive)**

Replace:

```
# [iam.tls] ca_cert_path, which PINS. ROOTS ONLY. Read once at boot. A bare self-signed leaf
```

With:

```
# [iam.tls] ca_cert_path, which PINS. ROOTS ONLY: every certificate here becomes an unconstrained
# trust anchor for every request the upstream egress client makes, to any host it reaches — not
# for the whole process. Read once at boot. A bare self-signed leaf
```

- [ ] **Step 8: Verify no process-scoped claim survives**

```bash
grep -rn "process makes" rs/crates/services docs/ops/RUNBOOK-containers.md
```

Expected: **no output.** (`docs/superpowers/` is deliberately excluded — historical records are not amended.)

Then confirm every site still carries its roots-only warning:

```bash
grep -rci "roots only" \
  rs/crates/services/paigasus-iam/src/config.rs \
  rs/crates/services/paigasus-gateway/src/config.rs \
  rs/crates/services/paigasus-iam/iam.toml.example \
  rs/crates/services/paigasus-gateway/gateway.toml.example
```

Expected: `1` for each of the four files.

- [ ] **Step 9: Confirm the crates still compile (doc comments can break rustdoc links)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
cargo check -p paigasus-iam -p paigasus-gateway
```

Expected: `Finished`, no warnings (the workspace sets `warnings = "deny"`).

- [ ] **Step 10: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/config.rs \
        rs/crates/services/paigasus-gateway/src/config.rs \
        rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs \
        rs/crates/services/paigasus-iam/iam.toml.example \
        rs/crates/services/paigasus-gateway/gateway.toml.example \
        docs/ops/RUNBOOK-containers.md
git commit -m "docs(rs): scope the CA-bundle trust claim to the client, not the process (SMA-570)

The anchors go onto one reqwest::ClientBuilder each, so tonic, async-nats
and redis are unaffected. Corrects five sites including iam.toml.example,
which operators copy, and adds the matching sentence to gateway.toml.example.
The unconstrained-anchor and roots-only warnings are unchanged."
```

---

### Task 2: Promote IAM's `describe_error` to a shared module

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/error_chain.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/mod.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/relay.rs:57-72` (remove fn), `:198` (import), `:457-476` (move tests)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub(crate) fn describe_error(err: &(dyn std::error::Error + 'static)) -> String` at
  `crate::adapters::error_chain::describe_error`. Renders the `source()` chain as
  `"outer: middle: inner"`, joined with `": "`, including the top-level `Display`. Task 3 uses it.

This is a pure move — the existing tests are the proof, so no new test is written.

- [ ] **Step 1: Create the module with the function and its existing tests**

Create `rs/crates/services/paigasus-iam/src/adapters/error_chain.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Rendering an error and its full `source()` chain as one line.
//!
//! Several of this crate's error types carry a static `Display` and put the real cause in
//! `source()` — `AuthnError::Backend`'s is the literal `"backend error"`, and reqwest's
//! client-build error is the literal `"builder error"`. For those, `to_string()` alone tells an
//! operator nothing, so anything that renders one into a message, a log line or a stored column
//! walks the chain first.
//!
//! Lived in `adapters/events/relay.rs` until SMA-570 needed it in `adapters/oidc/jwks.rs` too.

/// Renders `err` and its full `source()` chain as `"outer: middle: inner"`.
pub(crate) fn describe_error(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(e) = source {
        parts.push(e.to_string());
        source = e.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_error_walks_the_full_source_chain_without_duplicating_levels() {
        #[derive(Debug, thiserror::Error)]
        #[error("transport closed")]
        struct Inner;

        #[derive(Debug, thiserror::Error)]
        #[error("publish failed")]
        struct Middle(#[source] Inner);

        #[derive(Debug, thiserror::Error)]
        #[error("backend error")]
        struct Outer(#[source] Middle);

        let err = Outer(Middle(Inner));
        assert_eq!(describe_error(&err), "backend error: publish failed: transport closed");
    }

    #[test]
    fn describe_error_of_a_sourceless_error_is_just_its_display() {
        #[derive(Debug, thiserror::Error)]
        #[error("nope")]
        struct Bare;

        assert_eq!(describe_error(&Bare), "nope");
    }
}
```

Note: the two test bodies above are transcribed from `relay.rs:455-476`. Open that file and copy
the real bodies verbatim rather than trusting this transcription — if they differ, the file wins.

- [ ] **Step 2: Register the module**

In `rs/crates/services/paigasus-iam/src/adapters/mod.rs`, add in alphabetical position among the
`pub(crate)` entries — the file already uses this exact pattern for `redis_conn` and `retryable`:

```rust
pub(crate) mod error_chain;
```

- [ ] **Step 3: Remove the old function and its tests from relay.rs, and import instead**

Delete `fn describe_error` (with its doc comment) at `relay.rs:57-72`, and delete the two
`describe_error_*` tests at `relay.rs:455-476`. Add to relay.rs's imports:

```rust
use crate::adapters::error_chain::describe_error;
```

Leave the call site at `relay.rs:198` untouched.

- [ ] **Step 4: Verify the move changed no behaviour**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
cargo nextest run -p paigasus-iam --lib -E 'test(describe_error)'
```

Expected: PASS, **2 tests run**. If it reports 0 tests, the move dropped them — go back to Step 1.

- [ ] **Step 5: Verify the whole lib still builds clean**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: `Finished`, no warnings. A leftover unused import in relay.rs fails here.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/error_chain.rs \
        rs/crates/services/paigasus-iam/src/adapters/mod.rs \
        rs/crates/services/paigasus-iam/src/adapters/events/relay.rs
git commit -m "refactor(rs): promote IAM's describe_error to a shared adapters module (SMA-570)

Pure move, tests carried over. SMA-570 needs the same source()-chain walk
in the OIDC adapter, and a second copy in the same crate is not warranted."
```

---

### Task 3: IAM — attribute a build failure to the bundle

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs` (add types + fns above `HttpJwksFetcher`; rework `new` at `:124-171`; add tests in `mod tests`)

**Interfaces:**
- Consumes: `crate::adapters::error_chain::describe_error` from Task 2.
- Produces (IAM-local; Task 4 writes its own copies, deliberately):
  - `enum Attribution<'a> { NoBundle, Bundle { path: &'a str }, BundleAndStore { path: &'a str } }`
  - `fn attribute_build_failure<'a>(bundle: Option<&'a str>, control_build_ok: impl FnOnce() -> bool) -> Attribution<'a>`
  - `fn build_failure_message(attribution: &Attribution<'_>, chain: &str) -> String`
  - `fn base_builder(timeout: Duration) -> reqwest::ClientBuilder`
  - `HttpJwksFetcher::new_with_control_build(timeout: Duration, tls: IdpTls<'_>, control_build_ok: impl FnOnce() -> bool) -> Result<Self, AuthnError>` — `pub(crate)`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `jwks.rs`, after the existing `undecodable_bundle_is_a_boot_error`:

```rust
    // ---- build-failure attribution (SMA-570) --------------------------------------------------
    // A certificate body of `AAAAAAAA` is valid base64 (six zero bytes) but not valid DER. Unlike
    // the `!!!not base64!!!` fixture above, it PASSES `from_pem_bundle` and only fails later
    // inside `builder.build()` — a genuinely different code path (reqwest's `read_pem_certs`
    // against `RootCertStore::add`), which is why this needs its own tests.
    const INVALID_DER_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----\nAAAAAAAA\n-----END CERTIFICATE-----\n";

    #[test]
    fn attribution_without_a_bundle_never_runs_the_control_build() {
        let probed = std::cell::Cell::new(false);
        let attribution = attribute_build_failure(None, || {
            probed.set(true);
            true
        });

        assert!(matches!(attribution, Attribution::NoBundle), "no bundle means nothing to attribute");
        assert!(!probed.get(), "a bundle-less failure must not pay for a second load_native_certs()");
    }

    #[test]
    fn attribution_blames_the_bundle_when_the_control_build_succeeds() {
        let attribution = attribute_build_failure(Some("/etc/paigasus/corp-ca.pem"), || true);
        assert!(
            matches!(attribution, Attribution::Bundle { path } if path == "/etc/paigasus/corp-ca.pem"),
            "a control build with no added anchors succeeding leaves the anchors as the only cause"
        );
    }

    #[test]
    fn attribution_names_both_when_the_control_build_also_fails() {
        let attribution = attribute_build_failure(Some("/etc/paigasus/corp-ca.pem"), || false);
        assert!(
            matches!(attribution, Attribution::BundleAndStore { path } if path == "/etc/paigasus/corp-ca.pem"),
            "reqwest adds user roots FIRST and short-circuits, so the bundle may also be invalid"
        );
    }

    #[test]
    fn the_no_bundle_message_is_byte_unchanged() {
        // AC4. The sentence is the one SMA-558 shipped; only the interpolated cause is now the
        // full source chain rather than reqwest's useless bare "builder error" (SMA-570 D9).
        assert_eq!(
            build_failure_message(&Attribution::NoBundle, "builder error: invalid peer certificate: BadEncoding"),
            "failed to build the IdP HTTP client: builder error: invalid peer certificate: BadEncoding — \
             this can also mean the platform trust store contains no parseable certificates"
        );
    }

    #[test]
    fn der_invalid_bundle_names_the_config_key() {
        let f = tmp_file_with(INVALID_DER_PEM);
        let err = HttpJwksFetcher::new_with_control_build(
            Duration::from_secs(5),
            IdpTls::Verify {
                extra_bundle: Some(f.path().to_str().unwrap()),
            },
            || true, // the platform store is healthy
        )
        .expect_err("a structurally invalid certificate must fail the build");

        let rendered = format!("{err:?}");
        assert!(matches!(err, AuthnError::Backend(_)), "expected Backend, got {err:?}");
        assert!(rendered.contains("authn.extra_ca_bundle_path"), "the error must name the config key: {rendered}");
        assert!(rendered.contains("not valid DER"), "the error must say what is wrong with it: {rendered}");
        assert!(
            !rendered.contains("this can also mean the platform trust store"),
            "a definitively-attributed failure must NOT send the operator to the trust store: {rendered}"
        );
    }

    #[test]
    fn der_invalid_bundle_with_a_broken_store_names_both() {
        let f = tmp_file_with(INVALID_DER_PEM);
        let err = HttpJwksFetcher::new_with_control_build(
            Duration::from_secs(5),
            IdpTls::Verify {
                extra_bundle: Some(f.path().to_str().unwrap()),
            },
            || false, // the platform store is broken too
        )
        .expect_err("a structurally invalid certificate must fail the build");

        let rendered = format!("{err:?}");
        assert!(rendered.contains("platform trust store"), "the store is the primary fault: {rendered}");
        assert!(rendered.contains("authn.extra_ca_bundle_path"), "the bundle must still be named: {rendered}");
        assert!(rendered.contains("fix that first"), "the operator needs an order of operations: {rendered}");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
cargo nextest run -p paigasus-iam --lib -E 'test(attribution) or test(der_invalid) or test(no_bundle_message)'
```

Expected: FAIL to **compile**, with `cannot find function attribute_build_failure`,
`cannot find type Attribution`, `cannot find function build_failure_message`, and
`no function or associated item named new_with_control_build`.

- [ ] **Step 3: Add the attribution types and functions**

Insert into `jwks.rs` immediately above `/// Live JwksFetcher:` (the `HttpJwksFetcher` doc comment),
and add `use crate::adapters::error_chain::describe_error;` to the imports at the top:

```rust
/// What a failed `reqwest::Client` build can be attributed to.
///
/// Only three combinations are reachable, and the type says so: a build with no configured bundle
/// can never be blamed on a bundle, so that state is unrepresentable rather than merely untested.
#[derive(Debug)]
enum Attribution<'a> {
    /// No bundle configured. Nothing to attribute; the operator gets the platform-store wording.
    NoBundle,
    /// A bundle is configured and a control build with no added anchors SUCCEEDED.
    Bundle { path: &'a str },
    /// A bundle is configured and the control build ALSO failed.
    BundleAndStore { path: &'a str },
}

/// Decides what a `build()` failure should be blamed on (SMA-570 D1).
///
/// `control_build_ok` builds a client with the SAME options but no added anchors. It is called
/// ONLY when a bundle is configured — a bundle-less failure has nothing to attribute, and probing
/// would cost a second `load_native_certs()` for nothing.
///
/// The inference on success is narrow and deliberately so: the control build succeeding does NOT
/// prove the platform store is healthy — reqwest errors on the native store only when
/// `valid_count == 0 && invalid_count > 0`, so an ABSENT or EMPTY store builds fine. What it
/// proves is that the store did not cause THIS failure, and since the two builds differ only in
/// the added anchors (guaranteed by both going through `base_builder`), the anchors did.
///
/// On failure the disjunction is genuinely incomplete, which is why that arm names both: reqwest
/// adds USER roots first and `?`-returns on the first bad one, before it ever reaches the native
/// store block. So a run where both are broken fails on the bundle, while the control fails on the
/// store — and telling the operator only about the store would send them to fix it and then fail
/// boot again on the still-invalid bundle.
fn attribute_build_failure<'a>(bundle: Option<&'a str>, control_build_ok: impl FnOnce() -> bool) -> Attribution<'a> {
    match bundle {
        None => Attribution::NoBundle,
        Some(path) => {
            if control_build_ok() {
                Attribution::Bundle { path }
            } else {
                Attribution::BundleAndStore { path }
            }
        }
    }
}

/// Renders an attribution as the operator-facing message. `chain` is `describe_error` of the
/// underlying `reqwest::Error` — its own `Display` is the bare, useless string `"builder error"`.
fn build_failure_message(attribution: &Attribution<'_>, chain: &str) -> String {
    match attribution {
        Attribution::NoBundle => format!(
            "failed to build the IdP HTTP client: {chain} — this can also mean the platform trust \
             store contains no parseable certificates"
        ),
        Attribution::Bundle { path } => format!(
            "authn.extra_ca_bundle_path {path:?} contains a structurally invalid certificate: it \
             decodes as base64 but is not valid DER ({chain}). A control client built without it \
             succeeded, so the platform trust store is not the cause."
        ),
        Attribution::BundleAndStore { path } => format!(
            "failed to build the IdP HTTP client: {chain}. A control client built WITHOUT \
             authn.extra_ca_bundle_path {path:?} also failed, so the platform trust store contains \
             no parseable certificates — fix that first, then re-check the bundle, which may also \
             be invalid."
        ),
    }
}

/// The client's non-TLS options, in ONE place so the control build differs from the real one by
/// exactly the added anchors — by construction rather than by inspection, and still true when a
/// future option is added here (SMA-570 D6). `ClientBuilder` is not `Clone`, so this must be a
/// function rather than a shared value.
fn base_builder(timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder().timeout(timeout)
}
```

- [ ] **Step 4: Rework the constructor**

Replace `HttpJwksFetcher::new`'s body (currently `jwks.rs:124-171`) with a delegating `new` plus
the real implementation. The `match tls` block and all bundle-loading code inside it are unchanged
except for capturing `configured_bundle`:

```rust
    pub fn new(timeout: Duration, tls: IdpTls<'_>) -> Result<Self, AuthnError> {
        Self::new_with_control_build(timeout, tls, || base_builder(timeout).build().is_ok())
    }

    /// `new` with the control build injected, so both attribution arms are reachable in tests
    /// without mutating `SSL_CERT_FILE` (which is `unsafe` in edition 2024) or depending on the
    /// host's trust store.
    pub(crate) fn new_with_control_build(timeout: Duration, tls: IdpTls<'_>, control_build_ok: impl FnOnce() -> bool) -> Result<Self, AuthnError> {
        let mut builder = base_builder(timeout);
        let mut configured_bundle: Option<&str> = None;

        match tls {
            IdpTls::AcceptInvalid => builder = builder.danger_accept_invalid_certs(true),
            IdpTls::Verify { extra_bundle: None } => {}
            IdpTls::Verify { extra_bundle: Some(path) } => {
                configured_bundle = Some(path);

                // ... existing body unchanged: fs::read, from_pem_bundle, is_empty guard,
                // tracing::info!, and the add_root_certificate loop ...
            }
        }

        let client = builder.build().map_err(|e| {
            let attribution = attribute_build_failure(configured_bundle, control_build_ok);
            match &attribution {
                Attribution::NoBundle => {}
                Attribution::Bundle { path } => {
                    tracing::error!(path = %path, attribution = "bundle", "IdP HTTP client build failed");
                }
                Attribution::BundleAndStore { path } => {
                    tracing::error!(path = %path, attribution = "bundle_and_store", "IdP HTTP client build failed");
                }
            }
            backend(build_failure_message(&attribution, &describe_error(&e)))
        })?;
        Ok(Self { client, clock: SystemClock })
    }
```

Keep every line inside the `Verify { extra_bundle: Some(path) }` arm exactly as it is today —
only the `configured_bundle = Some(path);` assignment is new. Do not re-type the loader from
memory; leave the existing lines in place.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
cargo nextest run -p paigasus-iam --lib -E 'test(attribution) or test(der_invalid) or test(no_bundle_message)'
```

Expected: PASS, 6 tests.

- [ ] **Step 6: Verify the pre-existing bundle tests still pass unchanged**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
cargo nextest run -p paigasus-iam --lib -E 'test(bundle) or test(accept_invalid)'
```

Expected: PASS. `missing_bundle_path_is_a_boot_error`, `certificate_free_bundle_is_a_boot_error`,
`undecodable_bundle_is_a_boot_error` and `no_bundle_and_accept_invalid_both_build` must all still
be there and green — they cover paths this change does not touch.

- [ ] **Step 7: Lint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: `Finished`, no warnings.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs
git commit -m "fix(rs): attribute an IAM CA-bundle build failure to its config key (SMA-570)

A certificate that decodes as base64 but is not valid DER passes
from_pem_bundle and only fails inside builder.build(), where the message
blamed the platform trust store. A control build with no added anchors
now decides: control green means the anchors are the cause; control red
means the store is broken and, since reqwest adds user roots first and
short-circuits, the bundle may be too — so that arm names both."
```

---

### Task 4: Gateway — the same attribution, no new error variant

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/openai/client.rs` (add types + fns; rework `new` at `:149-180`; add tests in `mod tests`)

**Interfaces:**
- Consumes: nothing from Tasks 2-3. The gateway carries its own copies of `Attribution`,
  `attribute_build_failure`, `base_builder` and `describe_error` — SMA-558 D7 keeps the two
  services duplicated rather than extracted, and there is no shared crate that would fit.
- Produces:
  - `struct InvalidBundleCertificate { source: reqwest::Error }` — a `thiserror` error whose
    `Display` carries the explanation and whose `source()` is the original `reqwest::Error`.
  - `OpenAiClient::new_with_control_build(cfg: &OpenAiConfig, connect_timeout: Duration, first_byte_timeout: Duration, stream_idle_timeout: Duration, control_build_ok: impl FnOnce() -> bool) -> Result<Self, OpenAiError>` — `pub(crate)`

**`adapters/http/error.rs` is NOT touched.** Both bundle arms return the existing
`OpenAiError::CaBundle`, which `error.rs:110` already maps to `GatewayError::UpstreamUnavailable`
alongside `Build`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `client.rs`, after `undecodable_ca_bundle_is_a_build_error`:

```rust
    // ---- build-failure attribution (SMA-570) --------------------------------------------------
    // `AAAAAAAA` is valid base64 (six zero bytes) but not valid DER, so unlike the
    // `!!!not base64!!!` fixture above it PASSES from_pem_bundle and fails later in
    // builder.build() — a different reqwest code path, hence its own tests.
    const INVALID_DER_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----\nAAAAAAAA\n-----END CERTIFICATE-----\n";

    fn client_with_bundle_and_control(extra_ca_bundle_path: Option<String>, control_build_ok: bool) -> Result<OpenAiClient, OpenAiError> {
        let cfg = OpenAiConfig {
            base_url: "https://api.openai.com/".to_string(),
            api_key: SecretString::from("sk-x".to_string()),
            extra_ca_bundle_path,
        };
        OpenAiClient::new_with_control_build(
            &cfg,
            Duration::from_secs(10),
            Duration::from_secs(30),
            Duration::from_secs(300),
            || control_build_ok,
        )
    }

    #[test]
    fn attribution_without_a_bundle_never_runs_the_control_build() {
        let probed = std::cell::Cell::new(false);
        let attribution = attribute_build_failure(None, || {
            probed.set(true);
            true
        });

        assert!(matches!(attribution, Attribution::NoBundle));
        assert!(!probed.get(), "a bundle-less failure must not pay for a second load_native_certs()");
    }

    #[test]
    fn attribution_blames_the_bundle_when_the_control_build_succeeds() {
        let attribution = attribute_build_failure(Some("/etc/paigasus/corp-ca.pem"), || true);
        assert!(matches!(attribution, Attribution::Bundle { path } if path == "/etc/paigasus/corp-ca.pem"));
    }

    #[test]
    fn attribution_names_both_when_the_control_build_also_fails() {
        let attribution = attribute_build_failure(Some("/etc/paigasus/corp-ca.pem"), || false);
        assert!(matches!(attribution, Attribution::BundleAndStore { path } if path == "/etc/paigasus/corp-ca.pem"));
    }

    #[test]
    fn the_build_variant_message_is_byte_unchanged() {
        // AC4. The no-bundle path still returns OpenAiError::Build, whose #[error] attribute is
        // untouched by SMA-570. Minted from a real failed build so the variant is exercised, not
        // just quoted.
        let bad = reqwest::Certificate::from_pem_bundle(INVALID_DER_PEM).expect("parses as one cert");
        let mut b = reqwest::Client::builder();
        for c in bad {
            b = b.add_root_certificate(c);
        }
        let reqwest_err = b.build().expect_err("an invalid-DER anchor must fail the build");

        assert_eq!(
            OpenAiError::Build(reqwest_err).to_string(),
            "failed to build the OpenAI HTTP client — this can also mean the platform trust store contains no parseable certificates"
        );
    }

    #[test]
    fn der_invalid_ca_bundle_names_the_config_key() {
        let f = tmp_file_with(INVALID_DER_PEM);
        let err = client_with_bundle_and_control(Some(f.path().to_str().unwrap().to_string()), true)
            .expect_err("a structurally invalid certificate must fail the build");

        let rendered = format!("{err:?}");
        assert!(matches!(err, OpenAiError::CaBundle { .. }), "expected CaBundle, got {err:?}");
        assert!(
            err.to_string().contains("upstream.openai.extra_ca_bundle_path"),
            "the error must name the config key: {err}"
        );
        assert!(rendered.contains("not valid DER"), "the error must say what is wrong with it: {rendered}");
    }

    #[test]
    fn der_invalid_ca_bundle_with_a_broken_store_names_both() {
        let f = tmp_file_with(INVALID_DER_PEM);
        let err = client_with_bundle_and_control(Some(f.path().to_str().unwrap().to_string()), false)
            .expect_err("a structurally invalid certificate must fail the build");

        let rendered = format!("{err:?}");
        assert!(matches!(err, OpenAiError::CaBundle { .. }), "expected CaBundle, got {err:?}");
        assert!(rendered.contains("platform trust store"), "the store is the primary fault: {rendered}");
        assert!(rendered.contains("fix that first"), "the operator needs an order of operations: {rendered}");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
cargo nextest run -p paigasus-gateway --lib -E 'test(attribution) or test(der_invalid) or test(build_variant)'
```

Expected: FAIL to compile — `cannot find function attribute_build_failure`, `cannot find type
Attribution`, `no function or associated item named new_with_control_build`.

- [ ] **Step 3: Add the attribution machinery and the typed source error**

Insert into `client.rs` immediately above `/// The outbound OpenAI client:` (the `OpenAiClient`
doc comment):

```rust
/// The `source` of a `CaBundle` error raised when the bundle's certificates parse as PEM but not
/// as DER. Holds the original `reqwest::Error` as its own `source`, so anyhow renders the whole
/// chain and callers keep the ability to downcast — a pre-rendered `String` would discard both.
#[derive(Debug, thiserror::Error)]
#[error(
    "contains a structurally invalid certificate: it decodes as base64 but is not valid DER. \
     A control client built without it succeeded, so the platform trust store is not the cause"
)]
struct InvalidBundleCertificate {
    #[source]
    source: reqwest::Error,
}

/// What a failed `reqwest::Client` build can be attributed to. Mirrors
/// `paigasus-iam/src/adapters/oidc/jwks.rs`'s copy; see SMA-558 D7 for why the two services'
/// bundle handling is duplicated rather than extracted.
///
/// Only three combinations are reachable, and the type says so.
#[derive(Debug)]
enum Attribution<'a> {
    NoBundle,
    Bundle { path: &'a str },
    BundleAndStore { path: &'a str },
}

/// Decides what a `build()` failure should be blamed on (SMA-570 D1). `control_build_ok` builds a
/// client with the SAME options but no added anchors, and is called ONLY when a bundle is
/// configured.
///
/// Success proves the store did not cause THIS failure — not that it is healthy, since reqwest
/// errors on the native store only when `valid_count == 0 && invalid_count > 0`, so an absent or
/// empty store builds fine. Failure names BOTH, because reqwest adds user roots first and
/// `?`-returns on the first bad one before reaching the native store block: with both broken, the
/// real build dies on the bundle while the control dies on the store.
fn attribute_build_failure<'a>(bundle: Option<&'a str>, control_build_ok: impl FnOnce() -> bool) -> Attribution<'a> {
    match bundle {
        None => Attribution::NoBundle,
        Some(path) => {
            if control_build_ok() {
                Attribution::Bundle { path }
            } else {
                Attribution::BundleAndStore { path }
            }
        }
    }
}

/// Renders `err` and its full `source()` chain as `"outer: middle: inner"`. reqwest's build error
/// `Display` is the bare string `"builder error"`, so the cause is only reachable this way.
/// IAM has its own copy at `adapters/error_chain.rs` (SMA-558 D7: duplicated, not extracted).
fn describe_error(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(e) = source {
        parts.push(e.to_string());
        source = e.source();
    }
    parts.join(": ")
}

/// The client's non-TLS options, in ONE place so the control build differs from the real one by
/// exactly the added anchors (SMA-570 D6). `ClientBuilder` is not `Clone`, hence a function.
fn base_builder(connect_timeout: Duration, stream_idle_timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder().connect_timeout(connect_timeout).read_timeout(stream_idle_timeout)
}
```

- [ ] **Step 4: Rework the constructor**

Replace `OpenAiClient::new`'s signature line and its first builder line, and its final
`builder.build()` mapping. The bundle-loading `if let Some(path)` block in between is unchanged.

```rust
    pub fn new(cfg: &OpenAiConfig, connect_timeout: Duration, first_byte_timeout: Duration, stream_idle_timeout: Duration) -> Result<Self, OpenAiError> {
        Self::new_with_control_build(cfg, connect_timeout, first_byte_timeout, stream_idle_timeout, || {
            base_builder(connect_timeout, stream_idle_timeout).build().is_ok()
        })
    }

    /// `new` with the control build injected, so both attribution arms are reachable in tests
    /// without mutating `SSL_CERT_FILE` (`unsafe` in edition 2024) or depending on the host's
    /// trust store.
    pub(crate) fn new_with_control_build(
        cfg: &OpenAiConfig,
        connect_timeout: Duration,
        first_byte_timeout: Duration,
        stream_idle_timeout: Duration,
        control_build_ok: impl FnOnce() -> bool,
    ) -> Result<Self, OpenAiError> {
        let mut builder = base_builder(connect_timeout, stream_idle_timeout);

        // ... existing `if let Some(path) = cfg.extra_ca_bundle_path.as_deref() { ... }` block,
        // entirely unchanged ...

        let http = builder.build().map_err(|e| match attribute_build_failure(cfg.extra_ca_bundle_path.as_deref(), control_build_ok) {
            Attribution::NoBundle => OpenAiError::Build(e),
            Attribution::Bundle { path } => {
                tracing::error!(path = %path, attribution = "bundle", "OpenAI HTTP client build failed");
                OpenAiError::CaBundle {
                    path: path.to_string(),
                    source: Box::new(InvalidBundleCertificate { source: e }),
                }
            }
            Attribution::BundleAndStore { path } => {
                tracing::error!(path = %path, attribution = "bundle_and_store", "OpenAI HTTP client build failed");
                let chain = describe_error(&e);
                OpenAiError::CaBundle {
                    path: path.to_string(),
                    source: format!(
                        "the platform trust store also contains no parseable certificates, which is the more \
                         likely cause — fix that first, then re-verify this bundle ({chain})"
                    )
                    .into(),
                }
            }
        })?;
        Ok(Self {
            http,
            // Trim a trailing slash so `{base_url}/v1/chat/completions` never doubles up.
            base_url: cfg.base_url.trim_end_matches('/').to_owned(),
            api_key: cfg.api_key.clone(),
            first_byte_timeout,
        })
    }
```

Keep the existing bundle-loading block byte-for-byte — do not retype it.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
cargo nextest run -p paigasus-gateway --lib -E 'test(attribution) or test(der_invalid) or test(build_variant)'
```

Expected: PASS, 6 tests.

- [ ] **Step 6: Verify the pre-existing gateway tests still pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
cargo nextest run -p paigasus-gateway
```

Expected: PASS. In particular `undecodable_ca_bundle_is_a_build_error`,
`missing_ca_bundle_*`, `debug_never_leaks_the_api_key`, and `adapters::http::error`'s
`OpenAiError::Build` / `CaBundle` mapping tests must all still be green — the last two prove
`error.rs` genuinely needed no change.

- [ ] **Step 7: Lint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
cargo clippy -p paigasus-gateway --all-targets -- -D warnings
```

Expected: `Finished`, no warnings.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-gateway/src/adapters/openai/client.rs
git commit -m "fix(rs): attribute a gateway CA-bundle build failure to its config key (SMA-570)

Mirrors the IAM change. Both bundle-attributed arms reuse the existing
CaBundle variant, which already names upstream.openai.extra_ca_bundle_path
and already maps to UpstreamUnavailable, so adapters/http/error.rs needs
no change. The definitive arm boxes a typed source holding the original
reqwest::Error rather than a pre-rendered string."
```

---

### Task 5: Document the new boot error, and verify the whole change

**Files:**
- Modify: `docs/ops/RUNBOOK-containers.md` (§5, after the roots-only paragraph edited in Task 1)

**Interfaces:**
- Consumes: the final message wording from Tasks 3 and 4.
- Produces: nothing.

- [ ] **Step 1: Add the operator-facing diagnostic note**

Insert into `docs/ops/RUNBOOK-containers.md` §5, directly after the "Put roots in the bundle,
never intermediates" paragraph:

```markdown
  **Reading a CA-bundle boot failure.** All four failure modes name the config key. A bundle whose
  PEM decodes as base64 but is not valid DER is the subtle one — it passes the PEM parse and fails
  only when the TLS client is built, so boot reports it against the config key rather than against
  the platform trust store. If instead you see *"a control client built WITHOUT ... also failed,
  so the platform trust store contains no parseable certificates"*, the store is the primary
  fault: fix it first, then re-check the bundle, which may also be invalid. The plain *"this can
  also mean the platform trust store contains no parseable certificates"* wording appears only
  when no bundle is configured at all.
```

- [ ] **Step 2: Verify the RUNBOOK statement matches the shipped strings**

```bash
grep -n "fix that first" rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs
grep -n "fix that first" rs/crates/services/paigasus-gateway/src/adapters/openai/client.rs
grep -n "control client built WITHOUT" docs/ops/RUNBOOK-containers.md
```

Expected: a hit in each. If the code wording drifted from the plan, update the RUNBOOK to match
the code, not the other way round.

- [ ] **Step 3: Format the workspace**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
cargo fmt --all
git diff --stat
```

Expected: either no diff, or formatting-only changes to the two files touched. Review the diff.

- [ ] **Step 4: Full test run for both crates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
env -u CI PAIGASUS_SKIP_DOCKER=1 cargo nextest run -p paigasus-iam -p paigasus-gateway --no-tests=pass
```

Expected: PASS. `PAIGASUS_SKIP_DOCKER=1` is used because this change touches no Docker-backed
behaviour and the daemon may not be up; `env -u CI` is required because the `CI` check is
presence-based, not value-based. Note this leaves a cached PASS — Step 6 forces a re-run.

- [ ] **Step 5: Clippy and fmt gates for the whole workspace**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: both clean.

- [ ] **Step 6: Run the repo-level gate graph the way CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  --base origin/main --include-relations
```

Expected: PASS. Two notes from CLAUDE.md that apply here:

- `repo:release-parity*` aborts **INCONCLUSIVE at rc=2** inside an agent session because `proto`
  emits NDJSON on stdout. An rc=2 abort is not a pass — if you see one, re-run that gate with
  `AI_AGENT`, `CLAUDECODE` and `CLAUDE_CODE_ENTRYPOINT` unset before believing it.
- If `repo:affected-smoke` fails in under 3 seconds, **capture the full output before re-running** —
  a passing re-run overwrites the evidence. Grep it for `proto-shim`; if that line is present the
  failure is infrastructure, not the affected graph.

- [ ] **Step 7: Confirm the acceptance criteria one by one**

```bash
# AC1 — no process-scoped claim survives outside historical records
grep -rn "process makes" rs/crates/services docs/ops/RUNBOOK-containers.md    # expect: no output
# AC2/AC3 — the new tests exist and pass in both services
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs   # MANDATORY: rustup reads rust-toolchain.toml from the CWD, not from --manifest-path
cargo nextest run -p paigasus-iam -p paigasus-gateway \
  --lib -E 'test(der_invalid)'                                                # expect: 4 tests, PASS
# AC4 — the unchanged-message assertions
cargo nextest run -p paigasus-iam -p paigasus-gateway \
  --lib -E 'test(byte_unchanged)'                                             # expect: 2 tests, PASS
```

- [ ] **Step 8: Commit**

```bash
git add docs/ops/RUNBOOK-containers.md
git commit -m "docs(rs): tell operators how to read a CA-bundle boot failure (SMA-570)

The invalid-DER case now names the config key, and the both-broken case
names the platform trust store first. Records which message means which
so an operator is not left guessing between the two."
```

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
|---|---|
| §1 Item 1, five sites + gateway.toml.example | Task 1 |
| §2 D1 control-build attribution, three arms | Tasks 3, 4 |
| §2 D2 `Attribution` + injected probe | Tasks 3, 4 (Steps 1, 3) |
| §2 D3 `describe_error` reuse + gateway copy | Tasks 2, 4 |
| §2 D4 no new gateway variant, `error.rs` untouched | Task 4 (Step 6 proves it) |
| §2 D5 `accept_invalid_tls` unaffected | Task 3 Step 6 (`no_bundle_and_accept_invalid_both_build`) |
| §2 D6 `base_builder` | Tasks 3, 4 (Step 3) |
| §2 D7 failure-path logging | Tasks 3, 4 (Step 4) |
| §2 D8 literal strings + no registry codes | Tasks 3, 4 (Step 3) |
| §2 D9 AC4 deviation | Task 3 (`the_no_bundle_message_is_byte_unchanged`), Task 4 (`the_build_variant_message_is_byte_unchanged`) |
| §5 CI gates — no registry work | Task 5 Step 6 |
| §6 historical docs not amended | Task 1 Step 8 scopes the grep |

**Placeholder scan:** none. Every code step carries real code. The two "existing body unchanged"
markers in Tasks 3 and 4 are deliberate instructions *not* to retype working code, with the exact
one-line delta called out.

**Type consistency:** `Attribution` / `attribute_build_failure` / `base_builder` /
`new_with_control_build` are spelled identically in Tasks 3 and 4 and in both Interfaces blocks.
`build_failure_message` exists only in IAM — the gateway maps attribution straight to
`OpenAiError` variants, because its no-bundle message lives in a fixed `#[error]` attribute. That
asymmetry is intentional and is stated in both Interfaces blocks.
