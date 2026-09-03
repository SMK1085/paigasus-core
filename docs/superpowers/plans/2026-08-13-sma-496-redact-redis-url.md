# SMA-496 — Redact `redis_url` and `publisher.url` in Config Dumps: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put the four credential-bearing URLs in `IamConfig` behind the existing `RedactedUrl` newtype, so neither `Debug` nor `Serialize` can ever emit a Redis or NATS password.

**Architecture:** `RedactedUrl` (`src/config.rs:50`) already exists and already covers the two Postgres DSNs. This change applies it to `authn.jwks_cache.redis_url`, `authz.cache.redis_url`, `api_keys.introspect_cache.redis_url` and `outbox.publisher.url`, and deletes the ~40 lines of hand-rolled `Debug`/`Serialize` on `PublisherConfig` that were doing the same job by hand. Redaction lives purely in the outbound directions — `Deserialize` still delegates to `String`, and `as_str()` still yields the real value — so no connection, validation rule, or config file changes behavior.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), `serde`, `figment`, `serde_json`, `cargo nextest`, Moon 2.3.2.

**Spec:** `docs/superpowers/specs/2026-08-13-sma-496-redact-redis-url-design.md`

## Global Constraints

- **Work in the worktree** `/Users/smaschek/dev/paigasus/paigasus-core-sma496` on branch `feature/sma-496-iam-redact-redis-url`. NOT the main checkout at `/Users/smaschek/dev/paigasus/paigasus-core` — a concurrent session owns that one. If you are a subagent, your cwd is pinned to the main checkout; your **first action** must be to enter the worktree and confirm `git branch --show-current` prints `feature/sma-496-iam-redact-redis-url`.
- **PATH:** every command that invokes a **proto-managed CLI** (`moon`, `cargo`/`cargo-nextest`, `uv`, `buf`) must be prefixed with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` — those are not on the default tool PATH, and shims must come first so the repo-pinned versions win. Plain `git`, `grep`, `docker` and `cd` steps need no prefix, which is why the `git`-only blocks below omit it.
- **All `cargo` commands run from `rs/`.**
- **`--all-targets` is mandatory** on every `cargo clippy` invocation. Without it, clippy skips test targets and seven `publisher.url` construction sites in the NATS suites are never compiled.
- **The workspace is `warnings = deny`.** An unused import is a hard error. Never leave a `use` behind that a later step removes the last user of.
- **SPDX header:** every source file opens with `// SPDX-License-Identifier: Apache-2.0`. All files here already exist; do not add or move headers.
- **Commit style:** conventional commits with workspace scope, e.g. `feat(rs): …`. Subject must **start lowercase** and be **≤100 chars**. The body must contain no `#NNN` issue references and no stray `token: value` lines — commitlint reads those as footers and fails `footer-leading-blank`. Write "SMA-496", never "#496".
- **Never use `--no-verify`.** The worktree is provisioned; the hooks work.
- **`RedactedUrl::as_str` is written as a path, never a closure.** Use `.as_ref().map(RedactedUrl::as_str)`, not `.as_ref().map(|u| u.as_str())` — the closure form leaves the `RedactedUrl` import unused (a hard error) and defeats the greppability the type exists for.

---

## File Structure

No files are created except this plan's own test additions, which go into existing test modules. Every change is a modification.

| File | Responsibility in this change |
|---|---|
| `rs/crates/services/paigasus-iam/src/config.rs` | The four field type changes, the `PublisherConfig` derive, deletion of two hand-rolled impls, one `validate()` read site, three test assertion sites, two test construction sites, two new tests, two strengthened tests, and all eight doc corrections |
| `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` | Three `redis_url` read sites in `AppState::new` + the import |
| `rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs` | One `publisher.url` read site + the import + two construction sites in its `#[cfg(test)] mod tests` |
| `rs/crates/services/paigasus-iam/tests/authz_acceptance.rs` | Three `redis_url` construction sites |
| `rs/crates/services/paigasus-iam/tests/api_key_cache_connection.rs` | Two `redis_url` construction sites + the import |
| `rs/crates/services/paigasus-iam/tests/nats_publisher.rs` | One `publisher.url` construction site |
| `rs/crates/services/paigasus-iam/tests/nats_permissions.rs` | Four `publisher.url` construction sites |

**Why the tasks are drawn where they are.** The workspace is `warnings = deny` and a field's type change breaks every reader at once, so a task may never end with the crate half-converted — "change the type now, fix the readers later" does not compile. Task 1 and Task 2 are therefore each atomic over one field group, and each ends green. They are separable because a reviewer could accept the three `redis_url` fields (the issue's stated scope) while rejecting the `publisher.url` widening.

---

### Task 1: The three `redis_url` fields

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs` — field decls at `:119-122`, `:164-168`, `:272-277`; assertions at `:1531`, `:1763`, `:2068`; new test in `mod tests`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` — import at `:71`; read sites at `:325`, `:583`, `:676`
- Modify: `rs/crates/services/paigasus-iam/tests/authz_acceptance.rs` — `:466`, `:553`, `:637`
- Modify: `rs/crates/services/paigasus-iam/tests/api_key_cache_connection.rs` — import at `:28`; `:58`, `:60`
- Test: `rs/crates/services/paigasus-iam/src/config.rs` (`mod tests`)

**Interfaces:**
- Consumes: `RedactedUrl` (`src/config.rs:50`) — existing, unchanged. `RedactedUrl::as_str(&self) -> &str`; `impl From<String> for RedactedUrl`; `impl From<&str> for RedactedUrl`.
- Produces: `JwksCacheConfig.redis_url`, `AuthzCacheConfig.redis_url`, `ApiKeyCacheConfig.redis_url` all typed `Option<RedactedUrl>`. Task 2 extends the test this task creates, `cache_urls_never_appear_in_debug_or_serialized_config`, renaming it to `cache_and_broker_urls_never_appear_in_debug_or_serialized_config`.

- [ ] **Step 1: Enter the worktree and confirm the branch**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496
git branch --show-current   # MUST print: feature/sma-496-iam-redact-redis-url
git status --short          # MUST be clean apart from untracked .entire/ and .claude/
```

If the branch is anything else, STOP — you are in the wrong checkout.

- [ ] **Step 2: Write the failing test**

Add to `rs/crates/services/paigasus-iam/src/config.rs`, in `mod tests`, immediately after `redacted_url_renders_a_placeholder_in_both_outbound_directions` (which ends at `:2199`).

This version deliberately asserts only on the rendered strings, so it compiles against the current `Option<String>` fields and fails for the right reason. The `as_str()` non-vacuity assertions arrive in Step 8, once the type change makes them expressible.

```rust
    /// SMA-496. Companion to `connection_urls_never_appear_in_debug_or_serialized_config`
    /// above: a Redis connection string carries credentials exactly as a Postgres DSN does
    /// (`redis://user:pass@host:6379/0`), and `IamConfig` derives `Debug`/`Serialize`, so
    /// `RedactedUrl` has to cover BOTH outbound directions for all three cache URLs too.
    ///
    /// Each URL gets its own password and host so a leak names its own source.
    #[test]
    fn cache_urls_never_appear_in_debug_or_serialized_config() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://db_user:db_pw_secret@db.example.com/iam");
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [api_keys]
                        pepper = "{}"

                        [authn.jwks_cache]
                        backend = "redis"
                        redis_url = "redis://jwks_user:jwks_pw_secret@jwks.example.com:6379/0"

                        [authz.cache]
                        backend = "redis"
                        redis_url = "redis://authz_user:authz_pw_secret@authz.example.com:6379/1"

                        [api_keys.introspect_cache]
                        backend = "redis"
                        redis_url = "redis://apikey_user:apikey_pw_secret@apikey.example.com:6379/2"

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]
                    "#,
                    valid_pepper_b64()
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;

            let debugged = format!("{cfg:?}");
            let serialized = serde_json::to_string(&cfg).expect("IamConfig serializes");

            // Hosts are asserted as EXACT names, never as a bare "example.com": the mandatory
            // issuer is `https://idp.example.com/...` and is deliberately NOT redacted, so a
            // blanket substring check would fail on it.
            for secret in [
                // `db_*` too: the fixture must set `database_url` for figment to extract at all,
                // so it plants this credential whether or not the field is under test here.
                "db_pw_secret",
                "jwks_pw_secret",
                "authz_pw_secret",
                "apikey_pw_secret",
                "db.example.com",
                "jwks.example.com",
                "authz.example.com",
                "apikey.example.com",
            ] {
                assert!(!debugged.contains(secret), "{secret} leaked into IamConfig's Debug output: {debugged}");
                assert!(!serialized.contains(secret), "{secret} leaked into IamConfig's serialized form: {serialized}");
            }

            // The placeholder must land IN PLACE, and in the right NUMBER. A field silently
            // dropped from the dump satisfies the "must not contain" assertions above just as
            // well as a redacted one does, which is why this is a count and not a `contains`.
            assert_eq!(serialized.matches(r#""redis_url":"<redacted>""#).count(), 3, "{serialized}");

            Ok(())
        });
    }
```

- [ ] **Step 3: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496/rs
cargo nextest run -p paigasus-iam cache_urls_never_appear
```

Expected: **FAIL**. The panic is the `assert_eq!` on the count — `left: 0, right: 3` — and the printed `serialized` contains the three real URLs including `jwks_pw_secret`. (The first `assert!` in the loop fires first, on `jwks_pw_secret` leaking into `Debug`; either failure proves the point.) If it PASSES, the fields are already redacted and something is wrong — stop and investigate.

- [ ] **Step 4: Change the three field types**

In `rs/crates/services/paigasus-iam/src/config.rs`, three edits. Each replaces the bare field with a documented `RedactedUrl` one. The doc text deliberately does NOT repeat the "dumped in logs and `readyz`" claim — Task 3 removes that claim everywhere, and adding three fresh copies of it here would work against that.

At `:119-122`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct JwksCacheConfig {
    pub backend: JwksCacheBackend,
    /// Required when `backend = "redis"`. A [`RedactedUrl`] because a Redis connection string
    /// carries credentials exactly as a Postgres DSN does
    /// (`redis://user:pass@host:6379/0`); read the real value with [`RedactedUrl::as_str`].
    pub redis_url: Option<RedactedUrl>,
}
```

At `:164-168`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AuthzCacheConfig {
    pub backend: AuthzCacheBackend,
    /// Required when `backend = "redis"`. A [`RedactedUrl`], same reason as
    /// [`JwksCacheConfig::redis_url`]; read the real value with [`RedactedUrl::as_str`].
    pub redis_url: Option<RedactedUrl>,
}
```

At `:272-277`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ApiKeyCacheConfig {
    pub backend: ApiKeyCacheBackend,
    /// Required when `backend = "redis"`. A [`RedactedUrl`], same reason as
    /// [`JwksCacheConfig::redis_url`]; read the real value with [`RedactedUrl::as_str`].
    pub redis_url: Option<RedactedUrl>,
    pub ttl_secs: u64,
}
```

Do NOT touch the `*Defaults` structs. All three defaults are `None`, which needs no change and is guarded by a test in Task 2.

- [ ] **Step 5: Fix the three read sites in `AppState::new`**

In `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`.

First the import at `:71`:

```rust
use crate::config::{ApiKeyCacheBackend, AuthzCacheBackend, IamConfig, JwksCacheBackend, RedactedUrl};
```

Then three read sites. Change **only** the `.as_deref()` line in each; leave the surrounding `ok_or_else` and its message exactly as they are.

At `:322-326` (authz):

```rust
                let redis_url = authz_cfg
                    .cache
                    .redis_url
                    .as_ref()
                    .map(RedactedUrl::as_str)
                    .ok_or_else(|| AuthnError::Backend("authz.cache.backend = \"redis\" without redis_url (IamConfig::validate must run first)".into()))?;
```

At `:579-584` (api keys):

```rust
                let api_key_url = cfg
                    .api_keys
                    .introspect_cache
                    .redis_url
                    .as_ref()
                    .map(RedactedUrl::as_str)
                    .ok_or_else(|| AuthnError::Backend("api_keys.introspect_cache.backend = \"redis\" without redis_url (IamConfig::validate must run first)".into()))?;
```

At `:673-677` (jwks):

```rust
                let redis_url = authn_cfg
                    .jwks_cache
                    .redis_url
                    .as_ref()
                    .map(RedactedUrl::as_str)
                    .ok_or_else(|| AuthnError::Backend("jwks_cache.backend = \"redis\" without redis_url (IamConfig::validate must run first)".into()))?;
```

Do NOT touch the `(Generations, Option<(RedisHandle, &str)>)` annotation at `:317`, `shares_one_connection` at `:767`, or the match guard at `:594`. `as_str()` returns a `&str` borrowed from the same `&IamConfig` that `.as_deref()` borrowed from, so the lifetimes are unchanged and the trimming comparison still operates on the real string.

- [ ] **Step 6: Fix the three assertion sites in `config.rs`**

At `:1531`:

```rust
            assert_eq!(cfg.authn.jwks_cache.redis_url.as_ref().map(RedactedUrl::as_str), Some("redis://localhost:6379"));
```

At `:1763`:

```rust
            assert_eq!(cfg.authz.cache.redis_url.as_ref().map(RedactedUrl::as_str), Some("redis://localhost:6379"));
```

At `:2068`:

```rust
            assert_eq!(cfg.api_keys.introspect_cache.redis_url.as_ref().map(RedactedUrl::as_str), Some("redis://localhost:6379"));
```

`mod tests` does `use super::*`, so `RedactedUrl` is already in scope — no import needed. Leave every `assert_eq!(…redis_url, None)` alone (`:1348`, `:1576`, `:1793`); `RedactedUrl` derives `PartialEq`, so those still compile.

- [ ] **Step 7: Fix the five integration-test construction sites**

In `rs/crates/services/paigasus-iam/tests/authz_acceptance.rs`, three identical edits at `:466`, `:553` and `:637`. In each, `redis_url` is an owned `String` moved into the struct:

```rust
    cfg.authz.cache = AuthzCacheConfig {
        backend: AuthzCacheBackend::Redis,
        redis_url: Some(redis_url.into()),
    };
```

In `rs/crates/services/paigasus-iam/tests/api_key_cache_connection.rs`, extend the import at `:28`:

```rust
use paigasus_iam::config::{ApiKeyCacheBackend, AuthzCacheBackend, IamConfig, RedactedUrl};
```

then `:58` and `:60`. These are **two different edits** — `:58` takes a `&str`, `:60` maps over an `Option<&str>` and must stay an `Option` so phase (d) can still pass `None`:

```rust
    cfg.authz.cache.redis_url = Some(authz_url.into());
    cfg.api_keys.introspect_cache.backend = ApiKeyCacheBackend::Redis;
    cfg.api_keys.introspect_cache.redis_url = api_key_url.map(RedactedUrl::from);
```

Leave `tests/keycloak_e2e.rs:212` and `tests/support/mod.rs:331` alone — both are `redis_url: None`, which needs no change.

- [ ] **Step 8: Strengthen the new test with non-vacuity and `Debug` assertions**

Now that the fields are `RedactedUrl`, add the two things Step 2 could not express. In `cache_urls_never_appear_in_debug_or_serialized_config`, insert immediately after the `let cfg: IamConfig = …extract()?;` line:

```rust
            // Sanity: all three REAL values round-tripped through figment, so the "must not
            // contain" assertions below cannot pass merely because figment populated nothing —
            // and `as_str` still yields something usable at the `connect_redis` call sites.
            assert_eq!(
                cfg.authn.jwks_cache.redis_url.as_ref().map(RedactedUrl::as_str),
                Some("redis://jwks_user:jwks_pw_secret@jwks.example.com:6379/0")
            );
            assert_eq!(
                cfg.authz.cache.redis_url.as_ref().map(RedactedUrl::as_str),
                Some("redis://authz_user:authz_pw_secret@authz.example.com:6379/1")
            );
            assert_eq!(
                cfg.api_keys.introspect_cache.redis_url.as_ref().map(RedactedUrl::as_str),
                Some("redis://apikey_user:apikey_pw_secret@apikey.example.com:6379/2")
            );
```

and add this line directly below the existing `assert_eq!` on the serialized count:

```rust
            assert_eq!(debugged.matches(r#"redis_url: Some(RedactedUrl("<redacted>"))"#).count(), 3, "{debugged}");
```

- [ ] **Step 9: Build every target and run the package**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496/rs
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p paigasus-iam --no-tests=pass
```

Expected: clippy clean; `cache_urls_never_appear_in_debug_or_serialized_config` PASSES; every previously-passing test still passes. Docker-gated suites (`api_key_cache_connection`, `authz_acceptance`) must report their phases, not `skipping` — if they skip, start Docker and re-run, because `api_key_cache_connection` is the end-to-end proof that the SMA-485 reuse comparison survived.

- [ ] **Step 10: Format and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496/rs
cargo fmt
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496
git branch --show-current   # re-confirm: feature/sma-496-iam-redact-redis-url
git add rs/crates/services/paigasus-iam/src/config.rs \
        rs/crates/services/paigasus-iam/src/adapters/http/mod.rs \
        rs/crates/services/paigasus-iam/tests/authz_acceptance.rs \
        rs/crates/services/paigasus-iam/tests/api_key_cache_connection.rs
git commit -m "feat(rs): redact the three cache redis_url fields in config dumps (SMA-496)"
```

The `cd` target is the **worktree**, never `/Users/smaschek/dev/paigasus/paigasus-core` — a concurrent session owns that checkout and is committing to its own branch. Re-check the branch name before every commit in this plan.

---

### Task 2: `publisher.url`, deleting the hand-rolled impls, and the defaults guard

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs` — struct at `:489-494`; delete impls at `:571-609`; read site at `:1152`; test constructions at `:2909`, `:2925`; strengthen tests at `:2907`, `:2923`; extend and rename the Task 1 test; new defaults test
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs` — import at `:50`; read site at `:231`; constructions at `:714`, `:733`
- Modify: `rs/crates/services/paigasus-iam/tests/nats_publisher.rs` — `:107`
- Modify: `rs/crates/services/paigasus-iam/tests/nats_permissions.rs` — `:170`, `:551`, `:569`, `:595`
- Test: `rs/crates/services/paigasus-iam/src/config.rs` (`mod tests`)

**Interfaces:**
- Consumes: `RedactedUrl` and the three converted fields from Task 1.
- Produces: `PublisherConfig.url: Option<RedactedUrl>`; `PublisherConfig` deriving `Debug` and `Serialize` rather than hand-rolling them. `PublisherConfig::default()` is unchanged and still returns `url: None`, which `OutboxDefaults` relies on.

- [ ] **Step 1: Pin the current `publisher.url` behavior before refactoring**

Unlike Task 1, this is not a red-green step: `publisher.url` is **already** redacted, by the hand-rolled impls. The assertions added here pass now and must still pass after those impls are deleted — that is exactly their job, since the risk in this task is a refactor regression, not a missing feature.

Rename the Task 1 test to `cache_and_broker_urls_never_appear_in_debug_or_serialized_config`, and update it:

Add to the TOML fixture, directly above the `[[authn.issuers]]` block:

```
                        [outbox.publisher]
                        url = "tls://nats_user:nats_pw_secret@nats.example.com:4222"
```

`backend` is deliberately left at its `tracing` default: `url` is redacted regardless of backend, and selecting `nats` would drag SMA-493's TLS and credentials-file validation rules into a test that is about redaction.

Add `"nats_pw_secret"` and `"nats.example.com"` to the `for secret in [...]` array, and add these two assertions beside the existing counts — the broker url has to be pinned in place in **both** directions, exactly as the three cache urls are:

```rust
            assert_eq!(serialized.matches(r#""url":"<redacted>""#).count(), 1, "{serialized}");
            assert_eq!(debugged.matches(r#" url: Some(RedactedUrl("<redacted>"))"#).count(), 1, "{debugged}");
```

**The delimiter in each pattern is load-bearing**, because `redis_url` ends in the same three letters as `url`. In JSON a quote must precede `url` (in `"redis_url"` the preceding character is `_`); in `Debug` a SPACE must (`, url: Some(…)` vs `, redis_url: Some(…)`). Drop either delimiter and the count silently becomes 4 rather than 1 — worth confirming once by removing the space and checking the failure reads `left: 4, right: 1`.

Also extend the test's doc comment to say it now covers the broker URL as well as the three cache URLs.

- [ ] **Step 2: Run it to confirm it passes BEFORE the refactor**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496/rs
cargo nextest run -p paigasus-iam cache_and_broker_urls_never_appear
```

Expected: **PASS**. The hand-rolled impls already emit `"url":"<redacted>"`. If it fails here, the fixture is wrong — fix it before touching any impl, or you lose the baseline that makes the rest of this task safe.

- [ ] **Step 3: Convert the field and delete the two hand-rolled impls**

In `rs/crates/services/paigasus-iam/src/config.rs`, replace the derive at `:489` and the field at `:494`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PublisherConfig {
    pub backend: PublisherBackend,
    /// Required when `backend = "nats"`. A [`RedactedUrl`] because it may carry credentials
    /// (`nats://user:pass@host`); read the real value with [`RedactedUrl::as_str`].
    ///
    /// The sibling `credentials_file`, `root_ca_bundle` and `inbox_prefix` stay plain `String`s
    /// deliberately: the first two are filesystem paths and the third a subject prefix — none
    /// is a secret, so none needs the newtype.
    pub url: Option<RedactedUrl>,
```

Then **delete `:571-609` entirely** — the whole of `impl std::fmt::Debug for PublisherConfig` and `impl Serialize for PublisherConfig`. Delete both blocks and the blank line between them; leave the surrounding items untouched.

- [ ] **Step 4: Fix the two `publisher.url` read sites**

In `src/config.rs` at `:1152`, inside `validate()`:

```rust
            if let Some(raw) = p.url.as_ref().map(RedactedUrl::as_str) {
```

Leave `p.url.is_none()` at `:1145` exactly as it is — it needs no type knowledge.

In `src/adapters/events/nats_publisher.rs`, extend the import at `:50`:

```rust
use crate::config::{PublisherConfig, RedactedUrl};
```

and change `:231`:

```rust
        let url = cfg.url.as_ref().map(RedactedUrl::as_str).expect("validate() guarantees url is Some for the nats backend");
```

- [ ] **Step 5: Fix the nine `publisher.url` construction sites**

`src/config.rs` at `:2909` and `:2925` — inside `the_publisher_url_is_redacted_in_debug` and `…_in_serialize`:

```rust
            url: Some("nats://user:hunter2@host:4222".into()),
```

`src/adapters/events/nats_publisher.rs` at `:714` and `:733`, both inside `#[cfg(test)] mod tests`:

```rust
            url: Some("nats://127.0.0.1:14222".into()),
```

`tests/nats_publisher.rs` at `:107`, in the shared `fn cfg(url: &str)` helper:

```rust
        url: Some(url.into()),
```

`tests/nats_permissions.rs` at `:170`, in `cfg_for` — `fixture.url` is an owned `String` field, so clone then convert:

```rust
        url: Some(fixture.url.clone().into()),
```

`tests/nats_permissions.rs` at `:551`, `:569` and `:595` — three identical lines, `String::replace` returns an owned `String`:

```rust
    cfg.url = Some(fixture.url.replace("nats://", "tls://").into());
```

- [ ] **Step 6: Strengthen the two existing publisher tests to assert in place**

Both currently assert only `contains("redacted")`, which would pass even if `url` were dropped from the output entirely — the exact failure mode this whole change is guarding against.

In `the_publisher_url_is_redacted_in_debug`, replace the `contains("redacted")` line:

```rust
        assert!(rendered.contains(r#"url: Some(RedactedUrl("<redacted>"))"#), "{rendered}");
```

In `the_publisher_url_is_redacted_in_serialize`, replace its `contains("redacted")` line:

```rust
        assert!(serialized.contains(r#""url":"<redacted>""#), "{serialized}");
```

Keep both `!contains("hunter2")` assertions exactly as they are.

- [ ] **Step 7: Build every target and confirm the pinned behavior survived**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496/rs
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p paigasus-iam --no-tests=pass
```

Expected: clippy clean, and `cache_and_broker_urls_never_appear_in_debug_or_serialized_config` still PASSES — the same assertions that passed in Step 2, now satisfied by the derive instead of the deleted impls. That equivalence is the whole proof of this task. `nats_publisher` and `nats_permissions` must **compile**; they are Docker-gated and may skip at runtime, but a compile error here is the failure mode this task most risks.

- [ ] **Step 8: Add the defaults-layer guard test**

In `src/config.rs` `mod tests`, directly after the test you have been editing:

```rust
    /// SMA-496 D6. The `*Defaults` structs feed figment's default LAYER
    /// (`Serialized::defaults`, `IamConfig::figment`), and they mirror only their TOP-LEVEL
    /// struct — the nested ones are the real config types (`AuthzDefaults.cache` is an
    /// `AuthzCacheConfig`, `OutboxDefaults.publisher` a `PublisherConfig`). So a `RedactedUrl`
    /// whose default were `Some(_)` would serialize the literal `"<redacted>"` INTO that layer,
    /// and figment would deserialize that string straight back out as the value: every
    /// deployment that did not override it would boot pointed at a host named `<redacted>`.
    /// `OutboxDefaults::listen_database_url` dodges this by being a plain `String`; the four
    /// nested URLs are safe only because every default is `None`. This is what keeps it so.
    ///
    /// Asserting over `serde_json` rather than figment's own `Value` tree is valid because
    /// `RedactedUrl::serialize` is serializer-agnostic — it calls
    /// `serializer.serialize_str("<redacted>")` unconditionally — so it emits the placeholder
    /// into figment's tree exactly as it does into JSON. If that ever stops being true, this
    /// guard decouples from the hazard it guards.
    #[test]
    fn defaults_never_serialize_a_redaction_placeholder() {
        let layer = serde_json::to_string(&Defaults::default()).expect("Defaults serializes");
        assert!(
            !layer.contains("<redacted>"),
            "a RedactedUrl with a non-None default leaked the placeholder INTO figment's default layer, \
             which figment would then deserialize back out as the real value: {layer}"
        );
    }
```

- [ ] **Step 9: Prove the guard actually bites**

A guard that passes for the wrong reason is worse than none. Temporarily break it, confirm it fails, then revert.

In `impl Default for AuthzDefaults` (`:762`), temporarily change the `cache` field's `redis_url: None` to:

```rust
                redis_url: Some("redis://localhost:6379".into()),
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496/rs
cargo nextest run -p paigasus-iam defaults_never_serialize
```

Expected: **FAIL**, with the message showing `"redis_url":"<redacted>"` inside the printed layer. Now **revert that edit** back to `redis_url: None` and re-run — expected PASS. Confirm with `git diff` that no trace of the temporary edit remains before committing.

- [ ] **Step 10: Format and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496/rs
cargo fmt
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496
git add rs/crates/services/paigasus-iam/src/config.rs \
        rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs \
        rs/crates/services/paigasus-iam/tests/nats_publisher.rs \
        rs/crates/services/paigasus-iam/tests/nats_permissions.rs
git commit -m "feat(rs): redact publisher.url via RedactedUrl and drop the hand-rolled impls (SMA-496)"
```

---

### Task 3: Correct the eight stale doc claims and the dangling impl references

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs` — `:18`, `:33`, `:37-38`, `:201`, `:243`, `:418`, `:483-488`, `:492-493`, `:1186-1188`, `:2137`, `:2919`

**Interfaces:**
- Consumes: nothing. Documentation only — no signature, type, or behavior changes.
- Produces: nothing consumed by later tasks.

Line numbers shift as you edit. Work **top to bottom** and re-grep between edits rather than trusting the originals:

```bash
grep -n "dumped to logs\|dumped in logs\|readyz\` config dump\|log/\`readyz\` config dumps\|hand-rolled" rs/crates/services/paigasus-iam/src/config.rs
```

- [ ] **Step 1: Fix the claim in `RedactedUrl`'s own doc — the canonical wording**

This is the paragraph the other seven sites will point at. In `RedactedUrl`'s doc, replace the sentence beginning "`IamConfig` derives `Debug`/`Serialize` because it is dumped in logs/`readyz` (`main.rs`)" with:

```rust
/// `IamConfig` derives `Debug`/`Serialize`, so a DSN's password must never round-trip through
/// either: both outbound directions emit a fixed `<redacted>` placeholder, while `Deserialize`
/// is hand-rolled to delegate straight to `String` so figment still populates the REAL value
/// from whichever layer (default/toml/env) supplied it.
///
/// **Nothing dumps `IamConfig` today** — `readyz` returns a bare status object and the one
/// config log line prints two socket addresses; `Serialize` is exercised only by this module's
/// tests. The redaction is deliberate defense-in-depth: it makes the dump somebody eventually
/// adds — a boot-time config log, a debug endpoint, a stray `{config:?}` in an error path —
/// safe by construction, rather than a leak found in review. Choosing the type is the whole
/// mechanism; there is no runtime guard behind it.
```

While in this doc, also update the "worn by" sentence at `:30-31` to name all six fields it now covers (`database_url`, `outbox.listen_database_url`, the three `redis_url`s, and `outbox.publisher.url`), and delete the clause at `:37-38` that reads "the same job `PublisherConfig`'s hand-rolled `Debug`/`Serialize` do for `nats://user:pass@host`" — after Task 2, `PublisherConfig` is a *user* of this type, not a parallel idiom. Replace that clause with a note that `PublisherConfig::url` wears it too.

- [ ] **Step 2: Fix the remaining seven claim sites**

Each keeps its own local point and loses only the false "is dumped" premise, pointing at `RedactedUrl`'s doc instead of restating it.

- `:18` — `IamConfig::database_url`'s field doc: "…and this struct is dumped to logs and `readyz`" becomes "…and this struct derives `Debug`/`Serialize` (see [`RedactedUrl`])".
- `:201` — `ApiKeyConfig::pepper`'s field doc: "`IamConfig`'s derived `Serialize` (used by log/`readyz` config dumps) omits it entirely" becomes "`IamConfig`'s derived `Serialize` omits it entirely".
- `:243` — `RawPepper`'s doc: "`IamConfig` derives `Debug`/`Serialize` because it's dumped in logs/`readyz`" becomes "`IamConfig` derives `Debug`/`Serialize`, and see [`RedactedUrl`]'s doc for why that alone is reason enough to redact".
- `:418` — `OutboxConfig::listen_database_url`: "…and this struct is dumped in logs/`readyz`" becomes "…and this struct derives `Debug`/`Serialize`".
- `:485` — inside `PublisherConfig`'s doc; handled wholesale by Step 3.
- `:2137` — `connection_urls_never_appear_in_debug_or_serialized_config`'s doc: "`IamConfig` is dumped to logs and `readyz` — so" becomes "`IamConfig` derives both — so".
- `:2919` — `the_publisher_url_is_redacted_in_serialize`'s doc: it says `Serialize` "is hand-rolled SEPARATELY from `Debug` (see `PublisherConfig`'s doc)". After Task 2 that is false. Replace with a note that both directions are now derived and redaction rides on `RedactedUrl`.

- [ ] **Step 3: Rewrite `PublisherConfig`'s doc paragraph**

At `:483-488`, the entire "``Debug``/``Serialize`` are hand-rolled rather than derived…" paragraph describes deleted code. Replace it with:

```rust
/// `url` wears [`RedactedUrl`], so `Debug`/`Serialize` are ordinary derives: redaction travels
/// with the field's type rather than with two hand-written impls that had to spell out every
/// sibling field (and hand-maintain their own field count) to protect one of them.
```

Also at `:492-493`, the field doc's "see the manual impls below" no longer resolves — Task 2 already replaced that field's doc wholesale, so confirm no such phrase survives.

- [ ] **Step 4: Rewrite the safety-critical comment in `validate()`**

At `:1186-1188` — the most important edit in this task. This comment justifies computing `scheme_hint` instead of interpolating `raw` into the error at `:1191`, and a boot-time validation error **is** printed, making this the one place in the crate where a credentialed URL could genuinely reach a log line. Left pointing at deleted impls, the guard reads as vestigial and invites "simplification" back to `{raw}`.

Replace the clause "and `PublisherConfig`'s hand-rolled `Debug`/`Serialize` impls redact this exact field specifically so it never reaches a log line; interpolating `raw` here would bypass that redaction" with:

```rust
                    // — and `url` wears `RedactedUrl` specifically so it never reaches a log
                    // line. This error string DOES reach the logs, so interpolating `raw` here
                    // would bypass that redaction entirely: note that `as_str()` is called
                    // above to obtain `raw` for PARSING only, and deliberately does not appear
                    // in the message. Emit the scheme alone, never the url.
```

Do not change `scheme_hint` or the error text itself.

- [ ] **Step 5: Verify no stale claim survives**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496
grep -n "dumped to logs\|dumped in logs\|readyz\` config dump\|log/\`readyz\` config dumps" rs/crates/services/paigasus-iam/src/config.rs
grep -n "hand-rolled \`Debug\`\|manual impls below\|hand-rolled SEPARATELY" rs/crates/services/paigasus-iam/src/config.rs
```

Expected: **no output from either**. Any hit is a site you missed. Note `RawPepper`'s own `Deserialize` is still legitimately described as "hand-rolled" — that phrasing is fine and is not matched by the patterns above; if you get a hit mentioning `Deserialize`, read it before editing.

- [ ] **Step 6: Confirm docs still build and nothing else moved**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496/rs
# `cargo doc … | tail -20` would be worse than useless: a pipeline exits with the
# LAST command's status, so tail's success masks cargo's failure. But cargo's own
# status is not usable as the gate either — see below. So filter for the ONE thing
# that would be a regression, and let the grep decide.
cargo doc -p paigasus-iam --no-deps 2>&1 | tee /tmp/sma496-doc.log >/dev/null
grep -c '^error' /tmp/sma496-doc.log            # expect 32 — the pre-existing baseline
grep 'src/config.rs' /tmp/sma496-doc.log && echo 'REGRESSION' || echo 'ok: no config.rs doc errors'

cargo clippy --workspace --all-targets -- -D warnings
git diff --stat
```

**`cargo doc` exits NON-ZERO on this repo, and that is not your fault.** The workspace lints imply `-D rustdoc::private_intra_doc_links`, and 32 pre-existing errors live in `hasher.rs`, `cedar_authorizer.rs`, `generation.rs` and `policy_snapshot.rs` — verified identical on unmodified `origin/main`. So neither the raw exit status nor a naive `tail` tells you anything. The meaningful assertion is the second grep: **zero hits for `src/config.rs`**, which is what proves this change introduced no broken intra-doc link (a `[\`RedactedUrl\`]` typo would surface here and nowhere else — clippy does not check doc links).

Expected: `32`, then `ok: no config.rs doc errors`; clippy clean; `git diff --stat` shows **only** `src/config.rs`.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496
git add rs/crates/services/paigasus-iam/src/config.rs
git commit -m "docs(rs): correct the stale config-dump rationale and impl references (SMA-496)"
```

---

### Task 4: Full CI gate run

**Files:**
- Modify: none expected. If a gate fails, fix the offending file and note it here.

**Interfaces:**
- Consumes: the complete change from Tasks 1-3.
- Produces: a branch ready for a PR.

- [ ] **Step 1: Confirm Docker is running**

```bash
docker info >/dev/null 2>&1 || { echo "START DOCKER — the gated suites would no-op"; exit 1; }
echo "docker: RUNNING"
```

**Fails closed on purpose.** An advisory `|| echo "START IT"` exits 0, so the rest of the verification runs anyway and the Docker-gated suites quietly skip — which is precisely the failure mode this step exists to catch.

`:nats-permissions` mints TLS certs and starts a broker; `api_key_cache_connection` and `authz_acceptance` need Redis and Postgres. Without Docker they skip silently and prove nothing.

- [ ] **Step 2: Run the fast loop clean**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496/rs
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p paigasus-iam --no-tests=pass
```

Expected: all three clean. Scan the nextest output for the word `skipping` — if `api_key_cache_connection` skipped, the SMA-485 reuse proof did not run and Step 4's claim is unsupported.

- [ ] **Step 3: Run the full graph exactly as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :nats-permissions :release-parity :release-parity-py \
  :release-parity-ts --base origin/main --include-relations
```

`:nats-permissions` is triggered by the `src/config.rs` edit (`moon.yml:160` lists it as an input) even though nothing about NATS permissions changed — that is a documented, accepted over-trigger, not a mistake.

If moon reports a bare "N failed" without naming the task, find it with:

```bash
jq '.actions[]|select(.status=="failed")|.label' .moon/cache/ciReport.json
```

- [ ] **Step 4: Verify the acceptance criteria by hand**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496
# AC 1+2: no credential-bearing URL is a bare String any more.
grep -n "redis_url: Option<\|pub url: Option<" rs/crates/services/paigasus-iam/src/config.rs
# AC 2: both hand-rolled impls are gone.
grep -c "impl Serialize for PublisherConfig\|impl std::fmt::Debug for PublisherConfig" rs/crates/services/paigasus-iam/src/config.rs
# AC 4: the always-runs proof for the SMA-485 comparison.
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam shares_one_connection
```

Expected: the first grep shows four `Option<RedactedUrl>` and no `Option<String>`; the second prints `0`; `shares_one_connection_is_trimmed_textual_equality` PASSES.

- [ ] **Step 5: Review the whole diff before handing off**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma496
git diff origin/main --stat
git diff origin/main
```

Confirm: seven source files touched plus the spec and this plan; no debug prints, no commented-out code, no leftover temporary edit from Task 2 Step 9, and no change to any `validate()` error string.

---

## Self-Review

**Spec coverage.** §3.1 type changes → Task 1 Step 4, Task 2 Step 3. §3.2 five read sites → Task 1 Step 5 (three), Task 2 Step 4 (two). §4.1 combined test → Task 1 Steps 2/8, Task 2 Step 1. §4.2 defaults guard → Task 2 Steps 8-9. §4.3 strengthened publisher tests → Task 2 Step 6. §4.4 all 17 churn sites → Task 1 Steps 6-7 (8 sites), Task 2 Step 5 (9 sites). §4.5 Docker → Task 4 Step 1, Task 1 Step 9. §5.1 eight doc sites → Task 3 Steps 1-2. §5.2 impl references incl. the `validate()` comment → Task 3 Steps 3-4. §5.3 additions → Task 1 Step 4, Task 2 Step 3, Task 3 Step 1. §9 verification → Task 4. AC 1-7 → Task 4 Step 4. No gaps.

**Type consistency.** `RedactedUrl::as_str` as a path, never a closure, at all five read sites and all four assertion sites. `From<String>` used where the source is owned (`redis_url.into()`, `fixture.url.clone().into()`, `.replace(…).into()`); `From<&str>` where borrowed (`authz_url.into()`, `url.into()`, string literals); `RedactedUrl::from` as a function only at `api_key_cache_connection.rs:60`, where it maps over an `Option<&str>`. The test renamed in Task 2 Step 1 is referenced by its new name everywhere after that point.

**Known ordering hazard.** Task 1 must land complete before Task 2 begins: the crate does not compile between a field's type change and its readers being fixed, so neither task may be split further or interleaved.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
