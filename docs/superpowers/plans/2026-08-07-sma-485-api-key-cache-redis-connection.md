# SMA-485 — Honour `api_keys.introspect_cache.redis_url` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `AppState::new` reuse the authz Redis connection for the API-key introspection cache only when the two configured URLs match textually, and dial a dedicated `RedisRole::ApiKeys` connection otherwise.

**Architecture:** One predicate (`shares_one_connection`) plus one restructured binding in the IAM composition root (`AppState::new`). The authz `RedisHandle` starts carrying the URL it was opened with, so the sharing decision compares against that handle's own origin rather than a second, independent read of the config. Everything else is correcting documentation that states the old rule — including a `# HELP` string served in every `/metrics` scrape.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), axum, `redis` 1.3 via `adapters::redis_conn`, `metrics` + `metrics-exporter-prometheus`, `testcontainers-modules` (Postgres + Redis), `cargo nextest`, Moon 2.3.2.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-08-07-sma-485-api-key-cache-redis-connection-design.md`. Decisions are cited as D1–D6; read it before Task 1.
- **SPDX header** on every source file: `// SPDX-License-Identifier: Apache-2.0` (first line).
- **rustfmt `max_width = 200`** (`rs/rustfmt.toml`). Do not hand-wrap to 100.
- **`cargo clippy --workspace -- -D warnings`** must pass; warnings are errors.
- **PATH:** every shell command in this plan must be preceded by
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` — the Bash tool's PATH lacks the proto-managed CLIs (moon, nextest, uv, buf). Shims **first**.
- **Working directory:** `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-485-api-key-redis-connection`. This is a git worktree; never `cd` to the main checkout. Rust commands run from its `rs/` subdirectory.
- **`repo:redis-connect-single-site` gate** (`moon.yml:145`): the strings `ConnectionManager`, `.get_connection_manager`, `.get_multiplexed_async_connection` must not appear on a **code** line anywhere in `rs/crates/services/paigasus-iam/src` or `tests`, and `.get_connection` must not appear outside `src/adapters/persistence/migration/`. Only `//` line comments are exempt — a `/* */` block comment trips it. This applies to the new test file.
- **Commits:** conventional, workspace-scoped (`feat(rs):`, `docs(rs):`). Body lines **≤ 100 chars** (commitlint `body-max-line-length`); subject lowercase after the colon and ≤ 100 chars. Never `--no-verify`. Write commit bodies via a heredoc (`git commit -F - <<'EOF'`), not `-m` with a long single line. A bare `#NNN` in the body breaks `footer-leading-blank` — write "PR NNN" instead.
- **Do not run anything in the background.** Every build/test command in this plan runs in the foreground to completion.
- **Docker is available on this machine** and is required by Task 2 onward.

---

### Task 1: The sharing predicate

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` — add a function after `connect_redis` (currently ends at line 726), and a test in the existing `#[cfg(test)] mod tests` (line 841).

**Interfaces:**
- Consumes: nothing.
- Produces: `pub(crate) fn shares_one_connection(authz_url: &str, api_key_url: &str) -> bool` in `crate::adapters::http`. Task 3 calls it.

- [ ] **Step 1: Write the failing test**

In `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`, inside `mod tests` (after the existing `protected_router_merge_has_no_path_conflicts` test, before the closing `}` at line 869), add:

```rust
    /// SMA-485 D1: the API-key introspect cache reuses the authz connection on TEXTUAL equality
    /// after trimming — deliberately not endpoint identity. Every row here is a spelling D1
    /// names explicitly, so the accepted costs are executable rather than prose.
    ///
    /// Trimming is not cosmetic: URL parsing strips leading/trailing C0 controls and spaces, so
    /// `redis://a:6379\n` (an env-var override or a heredoc'd secret) DIALS FINE and would
    /// otherwise split a deployment the operator believes is unified — silently, since the only
    /// symptom is an `iam_redis_breaker_state{role="api_keys"}` series that reads as deliberate.
    #[test]
    fn shares_one_connection_is_trimmed_textual_equality() {
        for (authz, api_key, expected, why) in [
            ("redis://a:6379", "redis://a:6379", true, "identical: SMA-444 Task 21's optimisation"),
            ("redis://a:6379", "redis://a:6379\n", true, "trailing newline is trimmed"),
            (" redis://a:6379 ", "redis://a:6379", true, "surrounding spaces are trimmed"),
            ("redis://a:6379", "redis://a:6379/0", false, "accepted cost: explicit default db"),
            ("redis://localhost:6379", "redis://127.0.0.1:6379", false, "accepted cost: host alias"),
            ("redis://:pw1@a:6379", "redis://:pw2@a:6379", false, "credentials differ"),
            ("redis://a:6379", "redis://b:6379", false, "the genuine split this issue is about"),
        ] {
            assert_eq!(shares_one_connection(authz, api_key), expected, "{why}: ({authz:?}, {api_key:?})");
        }
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib shares_one_connection 2>&1 | tail -20
```

Expected: **compile error**, `cannot find function 'shares_one_connection' in this scope`.

- [ ] **Step 3: Write the implementation**

In the same file, immediately after the `connect_redis` function (which currently ends at line 726 with `}`), add:

```rust
/// Whether the API-key introspect cache may reuse the authz connection: textual equality of the
/// two configured URLs after trimming, **not** endpoint identity (SMA-485 D1).
///
/// `redis://cache:6379` and `redis://cache:6379/0` name one backend spelled two ways and
/// deliberately get two connections — erring toward a second connection is wasteful, never wrong
/// (the key namespaces are disjoint: `iam:apikey:` vs `iam:authz:*` vs `iam:jwks:`), whereas
/// erring the other way is what SMA-485 exists to fix. Normalising through redis-rs was declined
/// because it would not resolve the motivating case either — `redis://localhost:6379` vs
/// `redis://127.0.0.1:6379` differs by HOST, which no parser resolves at config-read time — while
/// putting credential comparison (`ConnectionInfo` carries `password`) into the composition root.
///
/// The trim is load-bearing, not cosmetic. `IamConfig::validate` trims `authn.issuers` but no
/// `redis_url`, and URL parsing strips surrounding C0 controls and spaces — so a trailing newline
/// from an env-var override dials perfectly well and would differ only textually, silently
/// splitting a deployment the operator believes is unified.
pub(crate) fn shares_one_connection(authz_url: &str, api_key_url: &str) -> bool {
    authz_url.trim() == api_key_url.trim()
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib shares_one_connection 2>&1 | tail -20
```

Expected: `test result: ok. 1 passed`.

Note: at this point `shares_one_connection` has no non-test caller. If clippy complains about dead code, do **not** add `#[allow(dead_code)]` — Task 3 adds the caller. Just proceed; run clippy at the end of Task 3.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-485-api-key-redis-connection
git add rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
git commit -F - <<'EOF'
feat(rs): add the api-key cache connection-sharing predicate (SMA-485)

shares_one_connection decides whether the API-key introspect cache may
reuse the authz Redis connection: textual equality after trimming, not
endpoint identity (SMA-485 D1). The table test pins every spelling D1
names, including the trailing-newline case that dials fine but would
otherwise split a deployment silently.

No caller yet; the composition root switches over next.
EOF
```

---

### Task 2: The failing integration test

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/api_key_cache_connection.rs`

**Interfaces:**
- Consumes: `support::start_migrated_postgres()`, `support::start_mock_idp()`, `support::test_config(&idp)` from `tests/support/mod.rs`; `paigasus_iam::adapters::http::AppState::new`; `paigasus_observability::init`.
- Produces: nothing consumed by later tasks. Task 3 makes it pass.

**Why this task exists before the fix:** three of its four phases fail against current `main`, which is what makes it a regression test rather than a description of whatever the code happens to do.

- [ ] **Step 1: Write the failing test**

Create `rs/crates/services/paigasus-iam/tests/api_key_cache_connection.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! SMA-485: which Redis connection the API-key introspection cache is wired to.
//!
//! `AppState::new` reuses the authz `RedisHandle` for `api_keys.introspect_cache` only when the
//! two configured URLs match textually after trimming (SMA-485 D1); otherwise it dials its own
//! with `RedisRole::ApiKeys`, which is what gives the split deployment its own circuit breaker
//! (SMA-476 D1) and its own `role="api_keys"` metrics.
//!
//! **Why this observes a metric rather than the data path.** Proving that cache *traffic* reaches
//! the API-key Redis would mean opening a Redis client here to inspect the `iam:apikey:*` keys —
//! and `repo:redis-connect-single-site` bans the unnamed-connection constructors in `tests/` just
//! as in `src/` (moon.yml). The breaker gauge is the sanctioned observation channel: it is set at
//! construction (`redis_conn::connect` -> `Breaker::new(role)`), so the presence or absence of
//! `iam_redis_breaker_state{role="api_keys"}` is exactly "was a second connection opened".
//! Accepted residual: this proves the connection was opened from the configured URL, not that
//! traffic flows through it. The unit test `shares_one_connection_is_trimmed_textual_equality`
//! pins the predicate; `AppState::new` passes the dialled handle straight into
//! `RedisApiKeyCache::from_connection` on the next line.
//!
//! Runs against ephemeral Postgres + Redis in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test returns with a note — the same
//! gating pattern as `tests/authz_cache_redis.rs` / `tests/redis_jwks_cache.rs`.

mod support;

use paigasus_iam::adapters::http::AppState;
use paigasus_iam::config::{ApiKeyCacheBackend, AuthzCacheBackend, IamConfig};
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

/// Starts an ephemeral Redis container, returning its connection URL. Same CI-hard-fail /
/// local-skip gating as `support::start_migrated_postgres`; self-contained here, mirroring
/// `tests/authz_cache_redis.rs`.
async fn start_redis() -> Option<(ContainerAsync<Redis>, String)> {
    let node = match Redis::default().start().await {
        Ok(n) => n,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("Docker is required for the api-key cache connection test in CI: {e}");
            }
            eprintln!("skipping api_key_cache_connection: Docker unavailable ({e})");
            return None;
        }
    };

    let port = node.get_host_port_ipv4(6379).await.unwrap();
    let url = format!("redis://127.0.0.1:{port}");
    Some((node, url))
}

/// `authz.cache` on redis at `authz_url`, `api_keys.introspect_cache` on redis at
/// `api_key_url` (`None` = the field left unset, which `IamConfig::validate` rejects — phase d).
fn split_config(base: &IamConfig, authz_url: &str, api_key_url: Option<&str>) -> IamConfig {
    let mut cfg = base.clone();
    cfg.authz.cache.backend = AuthzCacheBackend::Redis;
    cfg.authz.cache.redis_url = Some(authz_url.to_string());
    cfg.api_keys.introspect_cache.backend = ApiKeyCacheBackend::Redis;
    cfg.api_keys.introspect_cache.redis_url = api_key_url.map(str::to_string);
    cfg
}

const API_KEYS_SERIES: &str = r#"iam_redis_breaker_state{role="api_keys"}"#;
const AUTHZ_SERIES: &str = r#"iam_redis_breaker_state{role="authz"}"#;

/// All four phases in ONE test function, deliberately. Two reasons, neither of them the
/// `OnceLock`: container reuse (four `AppState::new` boots against one Postgres + one Redis, not
/// four pairs), and correctness under a plain `cargo test`, where the whole file shares one
/// process and therefore one metrics registry — the `api_keys` gauge, once set, never disappears,
/// so the absence assertion is only meaningful before the presence one. Under `cargo nextest run`
/// (what `.moon/tasks/rust.yml` actually runs) each test is its own process and the ordering
/// would be moot; it is kept so the file is correct under both runners.
///
/// `AppState::new` runs four times against one Postgres. Boot reconciliation is converge-to-code
/// and idempotent since SMA-477, so repeated boots against one database are what production does
/// on every restart.
#[tokio::test]
async fn api_key_cache_shares_the_authz_connection_only_on_matching_urls() {
    // MUST be first: `metrics::gauge!` against a not-yet-installed global recorder is silently
    // dropped, so installing it after the first `AppState::new` (the order `tests/metrics.rs`
    // uses) would make every assertion below pass vacuously.
    let handle = paigasus_observability::init("test-iam-api-key-cache-conn");

    let Some((_pg, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let Some((_redis, redis_url)) = start_redis().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let base = support::test_config(&idp);

    // --- Phase (a): identical URLs -> ONE shared connection (SMA-444 Task 21, AC2) -----------
    let cfg = split_config(&base, &redis_url, Some(&redis_url));
    AppState::new(db.clone(), &cfg).await.expect("phase a: both caches on one reachable redis");
    let out = handle.render();
    // The positive control is what makes the negative assertion mean anything: without it, a
    // dead recorder / renamed metric / misspelled label would all read as "absent" and the
    // phase would pass while proving nothing.
    assert!(out.contains(AUTHZ_SERIES), "phase a: the authz breaker must be registered (control):\n{out}");
    assert!(!out.contains(API_KEYS_SERIES), "phase a: identical URLs must share ONE connection, so no api_keys breaker:\n{out}");

    // --- Phase (b): distinct URLs -> its OWN connection, role api_keys (AC1) -----------------
    // `/1` selects logical database 1 on the same server: a different URL string, so D1 splits
    // it, and a reachable endpoint, so the dial succeeds. Stock Redis ships `databases 16` and
    // the testcontainers module does not override it; a `SELECT 1` failure would red the
    // `expect` below rather than pass silently.
    let cfg = split_config(&base, &redis_url, Some(&format!("{redis_url}/1")));
    AppState::new(db.clone(), &cfg).await.expect("phase b: both redis endpoints are reachable");
    let out = handle.render();
    assert!(out.contains(API_KEYS_SERIES), "phase b: distinct URLs must open a second connection with role=api_keys:\n{out}");

    // --- Phase (c): the api-key URL is actually dialled (AC1/AC3) ----------------------------
    // The regression proof: `redis_conn::connect` is eager, so before SMA-485 this config boots
    // happily (the URL is never read) and after it refuses to start. `127.0.0.1:1` follows the
    // crate's own precedent (`adapters/redis_conn.rs`): unbindable by an unprivileged process,
    // so deterministically refused, and not racy against testcontainers' port mapping the way
    // bind-ephemeral-then-drop would be.
    //
    // Safe to run after (b): `connect` propagates the dial failure with `?` BEFORE
    // `Breaker::new(role)`, so a failed dial registers no gauge and cannot invalidate (a).
    //
    // `AppState` is not `Debug` (it derives `Clone` only), so `unwrap_err`/`expect_err` will not
    // compile — assert on `is_err()` instead. Same trap SMA-476 documented in `redis_conn.rs`.
    let cfg = split_config(&base, &redis_url, Some("redis://127.0.0.1:1"));
    let err = AppState::new(db.clone(), &cfg).await.err().map(|e| e.to_string());
    assert!(err.is_some(), "phase c: an unreachable api_keys redis_url must fail boot — it is dialled now");
    let err = err.unwrap();
    // Discriminates a DIAL failure from phase (d)'s wiring-defect error. Deliberately not
    // asserting on "Connection refused", which is OS-specific. Phases (a)/(b) having already
    // booted against these same containers is what rules out an environmental explanation.
    assert!(!err.contains("IamConfig::validate"), "phase c: expected a dial failure, got the missing-url wiring defect: {err}");

    // --- Phase (d): a missing URL is a wiring defect, not "inherit authz's" (D2) -------------
    // Before SMA-485 this booted, because the `Some(conn)` arm masked the absent URL.
    let cfg = split_config(&base, &redis_url, None);
    let err = AppState::new(db.clone(), &cfg).await.err().map(|e| e.to_string());
    assert!(err.is_some(), "phase d: backend=redis without redis_url must fail boot");
    assert!(
        err.as_deref().unwrap_or_default().contains("IamConfig::validate"),
        "phase d: expected the wiring-defect error naming validate, got: {err:?}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test api_key_cache_connection --no-tests=pass 2>&1 | tail -30
```

Expected: **FAIL**. The first assertion to blow up is phase (b)'s — `distinct URLs must open a second connection with role=api_keys` — because current code reuses the authz handle. (Phase (a) passes on current code; that is expected and is why it needs its positive control.)

If it instead fails to compile, fix the compile error and re-run; the phase-(b) failure is the state this step is waiting for.

- [ ] **Step 3: Confirm the failure is the right one**

Read the output and confirm it is the phase (b) assertion message, not a container/Docker/config error. Record the observed message; Task 3 Step 3 checks it is gone.

- [ ] **Step 4: Commit the failing test**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-485-api-key-redis-connection
git add rs/crates/services/paigasus-iam/tests/api_key_cache_connection.rs
git commit -F - <<'EOF'
test(rs): pin which redis connection the api-key cache uses (SMA-485)

Four phases over one Postgres + one Redis: identical URLs share one
connection, distinct URLs open a second with role=api_keys, an
unreachable api-key URL fails boot, and a missing one is a wiring
defect. Three of the four fail against the current composition root,
which is the point.

Observes the breaker gauge rather than the data path because
repo:redis-connect-single-site bans opening a redis client in tests/.
EOF
```

---

### Task 3: The composition-root fix

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:312-325` (the authz binding), `:396`, `:401` (destructuring), `:556-579` (the API-key cache arm).

**Interfaces:**
- Consumes: `shares_one_connection` from Task 1.
- Produces: `AppState::new` behaviour matching the spec's §3 table. Nothing else depends on it.

- [ ] **Step 1: Change the authz binding to carry its URL (D5)**

Replace lines 312-325 (`let (gens, redis_conn): (Generations, Option<RedisHandle>) = …` through the closing `};`) with:

```rust
        // SMA-485 D5: the handle is paired with the URL it was OPENED with, not left for a
        // later, independent re-read of `authz_cfg.cache.redis_url` to guess at. The API-key
        // cache below decides whether to reuse this connection by comparing against that URL, so
        // pairing them makes the comparison structurally be against this handle's own origin —
        // and deletes the `(Some(handle), None)` state that a second read would have to handle.
        let (gens, redis_conn): (Generations, Option<(RedisHandle, &str)>) = match authz_cfg.cache.backend {
            AuthzCacheBackend::Memory => (Generations::memory(), None),
            AuthzCacheBackend::Redis => {
                // `IamConfig::validate` rejects a redis backend without a URL at boot; a
                // `None` here is a wiring defect, not an operator error.
                let redis_url = authz_cfg
                    .cache
                    .redis_url
                    .as_deref()
                    .ok_or_else(|| AuthnError::Backend("authz.cache.backend = \"redis\" without redis_url (IamConfig::validate must run first)".into()))?;
                let conn = connect_redis(redis_url, RedisRole::Authz).await?;
                (Generations::from_connection(conn.clone()), Some((conn, redis_url)))
            }
        };
```

- [ ] **Step 2: Update the two authz destructuring sites**

At what is currently line 396 (inside the `slices` block), change:

```rust
            match &redis_conn {
                Some(conn) => Arc::new(SliceCache::from_connection(pg_loader, conn.clone(), authz_cfg.slice_cache_ttl_secs)) as Arc<dyn EntitySliceLoader>,
                None => pg_loader,
            }
```

to:

```rust
            match &redis_conn {
                Some((conn, _)) => Arc::new(SliceCache::from_connection(pg_loader, conn.clone(), authz_cfg.slice_cache_ttl_secs)) as Arc<dyn EntitySliceLoader>,
                None => pg_loader,
            }
```

At what is currently line 401, change:

```rust
        let decisions: Arc<dyn DecisionCache> = match &redis_conn {
            Some(conn) => Arc::new(RedisDecisionCache::from_connection(conn.clone(), authz_cfg.decision_cache_ttl_secs)),
            None => Arc::new(MemoryDecisionCache::new()),
        };
```

to:

```rust
        let decisions: Arc<dyn DecisionCache> = match &redis_conn {
            Some((conn, _)) => Arc::new(RedisDecisionCache::from_connection(conn.clone(), authz_cfg.decision_cache_ttl_secs)),
            None => Arc::new(MemoryDecisionCache::new()),
        };
```

- [ ] **Step 3: Rewrite the API-key cache arm**

Replace lines 558-578 (the whole `ApiKeyCacheBackend::Redis => { … }` arm, from its comment block through its closing `}`) with:

```rust
            ApiKeyCacheBackend::Redis => {
                // SMA-485: reuse the SHARED `redis_conn` opened above ONLY when the two
                // configured URLs match textually after trimming (`shares_one_connection`, D1) —
                // that is SMA-444 Task 21's one-connection-per-deployment optimisation, and it is
                // sound precisely when both URLs name the same endpoint. Before SMA-485 the reuse
                // was unconditional, so an operator who pointed `api_keys.introspect_cache` at a
                // second Redis got the authz one anyway: the URL `IamConfig::validate` REQUIRES
                // was then discarded, and SMA-476 D1's per-connection breaker isolation silently
                // did not hold (one connection, one breaker, so an authz outage short-circuited
                // API-key introspection against a healthy backend).
                //
                // The URL is read BEFORE the match, so a missing one is a loud wiring defect
                // rather than a silent fallback to the authz connection (D2) — matching the
                // authz arm above and the JWKS arm below. `IamConfig::validate` rejects that
                // config at boot; `AppState::new` takes a bare `&IamConfig`, so this stays a
                // real fallible step here.
                let api_key_url = cfg
                    .api_keys
                    .introspect_cache
                    .redis_url
                    .as_deref()
                    .ok_or_else(|| AuthnError::Backend("api_keys.introspect_cache.backend = \"redis\" without redis_url (IamConfig::validate must run first)".into()))?;
                let conn = match &redis_conn {
                    Some((conn, authz_url)) if shares_one_connection(authz_url, api_key_url) => conn.clone(),
                    _ => connect_redis(api_key_url, RedisRole::ApiKeys).await?,
                };
                Arc::new(RedisApiKeyCache::from_connection(conn, cfg.api_keys.introspect_cache.ttl_secs))
            }
```

- [ ] **Step 4: Run the integration test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test api_key_cache_connection --no-tests=pass 2>&1 | tail -20
```

Expected: `1 passed`. If phase (c) or (d) fails on its message assertion, read the actual error string before changing the assertion — a mismatch may mean the error is being produced by the wrong branch.

- [ ] **Step 5: Run the whole IAM suite plus clippy and fmt**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo nextest run -p paigasus-iam --no-tests=pass 2>&1 | tail -25
```

Expected: fmt clean, clippy clean, all IAM tests pass. `cargo clippy --all-targets` is what catches an unused import or a dead-code warning in the new test file.

- [ ] **Step 6: Verify the CI gate the change is closest to**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-485-api-key-redis-connection
moon run repo:redis-connect-single-site
```

Expected: PASS. If it reports offenders, the new test file or the new comments name a banned identifier on a code line — the fix is to reword, not to widen the gate.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-485-api-key-redis-connection
git add rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
git commit -F - <<'EOF'
fix(rs): honour api_keys.introspect_cache.redis_url (SMA-485)

AppState::new reused the authz RedisHandle for the API-key introspection
cache whenever authz.cache.backend was redis, so the redis_url that
IamConfig::validate REQUIRES was discarded in exactly the deployment
where an operator had set it deliberately. SMA-476 D1's per-connection
breaker isolation silently did not hold there either: one connection
means one breaker, so an authz outage short-circuited API-key
introspection against a healthy Redis.

Reuse is now conditioned on the two URLs matching textually after
trimming; otherwise the cache dials its own connection with
RedisRole::ApiKeys. The authz handle carries the URL it was opened with,
so the comparison cannot drift from a second read of the config, and a
missing api-key URL is a wiring defect rather than a silent fallback.
EOF
```

---

### Task 4: Correct the shipped documentation of the old rule

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/main.rs:386` (the `describe_gauge!` help text)
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs:69-73` (the metric's doc comment)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/api_keys/cache.rs:214-218`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:713-723` (the `connect_redis` doc)

**Interfaces:**
- Consumes: the behaviour established in Task 3.
- Produces: nothing. Documentation only.

**Why this is not cosmetic:** `main.rs:386`'s string is served as `# HELP` in **every `/metrics` scrape**. Left alone, an on-call reading it concludes a `role="api_keys"` series is impossible in their authz-on-Redis deployment — which is precisely the deployment where it now appears.

- [ ] **Step 1: Fix the `# HELP` text**

In `rs/crates/services/paigasus-iam/src/main.rs`, replace the `describe_gauge!` body string:

old:
```
"Redis circuit-breaker state per connection: 0=closed, 1=half_open, 2=open. Label role=authz|api_keys|jwks. Set independently by every replica — aggregate max by (job, role), never sum. role=\"api_keys\" only exists when authz.cache.backend=\"memory\" and api_keys.introspect_cache.backend=\"redis\"; otherwise those commands are attributed to role=\"authz\"."
```

new:
```
"Redis circuit-breaker state per connection: 0=closed, 1=half_open, 2=open. Label role=authz|api_keys|jwks. Set independently by every replica — aggregate max by (job, role), never sum. role=\"api_keys\" exists only when the API-key cache has its own connection: authz.cache.backend=\"memory\", or the two redis_urls differ textually (SMA-485). Otherwise those commands are attributed to role=\"authz\"."
```

- [ ] **Step 2: Fix the canonical metric doc**

In `rs/crates/libs/paigasus-observability/src/names.rs`, replace the first attribution caveat (lines 69-73):

old:
```rust
/// - `role="api_keys"` exists ONLY when `authz.cache.backend = "memory"` while
///   `api_keys.introspect_cache.backend = "redis"`. Otherwise the API-key cache reuses the authz
///   connection and its commands are attributed to `role="authz"` — a missing `api_keys` series
///   does NOT mean the API-key cache is idle.
```

new:
```rust
/// - `role="api_keys"` exists ONLY when the API-key cache holds its own connection: either
///   `authz.cache.backend = "memory"` while `api_keys.introspect_cache.backend = "redis"`, or
///   both are redis-backed with `redis_url`s that differ textually after trimming (SMA-485 D1).
///   Otherwise the API-key cache reuses the authz connection and its commands are attributed to
///   `role="authz"` — a missing `api_keys` series does NOT mean the API-key cache is idle.
///   Conversely, because the comparison is textual, two spellings of ONE endpoint produce an
///   `api_keys` series fronting the same physical Redis — see the next caveat.
```

- [ ] **Step 3: Fix the two in-crate wiring docs**

In `rs/crates/services/paigasus-iam/src/adapters/api_keys/cache.rs`, replace lines 214-218's first paragraph:

old:
```rust
    /// Builds a cache over an ALREADY-CONNECTED handle: mirrors
    /// `RedisDecisionCache::from_connection`/`SliceCache::from_connection` (SMA-444 Task 21) —
    /// `AppState::new` shares ONE redis connection across the redis-backed `Generations` +
    /// `RedisDecisionCache` + `SliceCache` + this cache rather than each opening its own;
    /// `connect` above stays the standalone-caller/test entry point.
```

new:
```rust
    /// Builds a cache over an ALREADY-CONNECTED handle: mirrors
    /// `RedisDecisionCache::from_connection`/`SliceCache::from_connection` (SMA-444 Task 21) —
    /// `AppState::new` shares ONE redis connection across the redis-backed `Generations` +
    /// `RedisDecisionCache` + `SliceCache` rather than each opening its own, and extends that
    /// sharing to THIS cache only when `api_keys.introspect_cache.redis_url` matches the authz
    /// one textually (SMA-485 D1); otherwise this cache is handed its own connection, dialled
    /// with `RedisRole::ApiKeys`. `connect` above stays the standalone-caller/test entry point.
```

In `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`, replace the `connect_redis` doc comment (lines 713-723):

old:
```rust
/// Opens `redis_url` and wraps it in a breaker-guarded [`RedisHandle`] — shared by every
/// redis-backed cache `AppState::new` wires (the authz `Generations`/`RedisDecisionCache`/
/// `SliceCache` trio, SMA-444 Task 21; the API-key `RedisApiKeyCache`, SMA-445 Task 19, when it
/// can't reuse the already-open `redis_conn` LOCAL BINDING in `AppState::new` — not to be
/// confused with the [`crate::adapters::redis_conn`] MODULE this delegates to), mirroring
/// `RedisJwksCache::connect`'s connect pattern.
///
/// Delegates to [`crate::adapters::redis_conn::connect`] for the tuned reconnect retry budget
/// (SMA-473) and the per-connection circuit breaker (SMA-476) — this function owns only the
/// `AuthnError` mapping. `role` labels this connection's breaker metrics; see SMA-476 D10 for why
/// a shared connection reports as `authz` even when it also serves the API-key cache.
```

new:
```rust
/// Opens `redis_url` and wraps it in a breaker-guarded [`RedisHandle`] — shared by every
/// redis-backed cache `AppState::new` wires (the authz `Generations`/`RedisDecisionCache`/
/// `SliceCache` trio, SMA-444 Task 21; the API-key `RedisApiKeyCache`, SMA-445 Task 19, when
/// [`shares_one_connection`] says its configured URL matches the authz one — otherwise that cache
/// gets its OWN handle from this same function, SMA-485). The `redis_conn` LOCAL BINDING in
/// `AppState::new` is not to be confused with the [`crate::adapters::redis_conn`] MODULE this
/// delegates to. Mirrors `RedisJwksCache::connect`'s connect pattern.
///
/// Delegates to [`crate::adapters::redis_conn::connect`] for the tuned reconnect retry budget
/// (SMA-473) and the per-connection circuit breaker (SMA-476) — this function owns only the
/// `AuthnError` mapping. `role` labels this connection's breaker metrics; a SHARED connection
/// reports every command as `authz`, including the API-key cache's (SMA-476 D10, as amended by
/// SMA-485 D1: sharing now requires the two URLs to match).
```

- [ ] **Step 4: Build and run the drift gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-485-api-key-redis-connection
moon run repo:observability-drift
```

Expected: all clean. `names.rs` is an input to `repo:observability-drift` (`moon.yml:119`), so this gate must actually be run rather than reasoned about — no metric family is added or renamed, so it is expected to stay green, but that is a result to confirm.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-485-api-key-redis-connection
git add rs/crates/services/paigasus-iam/src/main.rs rs/crates/libs/paigasus-observability/src/names.rs rs/crates/services/paigasus-iam/src/adapters/api_keys/cache.rs rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
git commit -F - <<'EOF'
docs(rs): correct the api_keys breaker-role attribution rule (SMA-485)

The iam_redis_breaker_state help text is served as # HELP in every
scrape and claimed role="api_keys" exists only when authz is
memory-backed. That is now false, and false in the worst direction: an
on-call would conclude the series is impossible in exactly the
deployment where it appears. Fixes the exposition text, the canonical
doc in names.rs, and the two in-crate wiring docs that state the old
rule.
EOF
```

---

### Task 5: RUNBOOK

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md` — lines 96, 1083-1084, 1375, 1447-1450, 1556-1561, plus a new entry after 1468.

**Interfaces:**
- Consumes: nothing.
- Produces: nothing. Satisfies AC5.

Line numbers drift as you edit. Work **bottom-up** (1556 first, then 1462, then 1447, 1375, 1083, 96) so earlier edits do not shift later targets.

- [ ] **Step 1: The hottest-path paragraph (currently 1556-1561)**

old:
```markdown
**`RedisApiKeyCache` usually shares the same connection, and sits on the hottest path.** It
reuses the `redis_conn` handle **only when `authz.cache.backend = "redis"`**; with the authz
cache on `memory` it dials its own `ConnectionManager` from
`api_keys.introspect_cache.redis_url` (`adapters/http/mod.rs`), which may be a different Redis
and in any case keeps its own independent connection and reconnect state. So a single Redis
outage need not hit both paths, and in the split configuration each fails on its own schedule.
```

new:
```markdown
**`RedisApiKeyCache` usually shares the same connection, and sits on the hottest path.** Since
SMA-485 it reuses the `redis_conn` handle **only when `api_keys.introspect_cache.redis_url` and
`authz.cache.redis_url` are the same string** (compared textually, after trimming — see the
`iam_redis_breaker_state` caveats above). Otherwise — the two URLs differ, or the authz cache is
on `memory` — it dials its own connection from `api_keys.introspect_cache.redis_url`
(`adapters/http/mod.rs`), with its own breaker, its own reconnect state and its own
`role="api_keys"` metrics. So a single Redis outage need not hit both paths, and in the split
configuration each fails on its own schedule. Before SMA-485 that promise did not hold with authz
on Redis: the API-key URL was ignored, both caches shared one connection and therefore one
breaker, so an authz-Redis outage short-circuited API-key introspection against a Redis that was
perfectly healthy.
```

- [ ] **Step 2: New remediation entry (insert after the paragraph ending at line 1468, before "**Verify that your orchestrator actually applies restart backoff")**

```markdown
**A boot failure naming `api_keys.introspect_cache`, on a deployment that started fine
yesterday.** Since SMA-485 `api_keys.introspect_cache.redis_url` is actually dialled. It was
previously ignored whenever `authz.cache.backend = "redis"`, so a wrong or unreachable value was
harmless — and `IamConfig::validate` **requires** the field, so a stale or placeholder value is
exactly what a config written under the old behaviour is likely to contain. Fix the endpoint, or,
to restore the previous behaviour exactly, set `api_keys.introspect_cache.redis_url`
byte-identical to `authz.cache.redis_url`. Note this dial happens *late* in `AppState::new` —
after boot reconciliation and the initial policy-snapshot compile — so each crash-loop attempt
pays a full Postgres reconcile and Cedar compile before failing.
```

- [ ] **Step 3: The D10 attribution caveat (currently 1447-1450)**

old:
```markdown
- `role="api_keys"` exists **only** in the split configuration
  (`api_keys.introspect_cache` pointed at its own Redis). Ordinarily the API-key cache reuses the
  `authz` handle, so a missing `api_keys` series does not mean the API-key cache is idle — check
  `role="authz"` instead.
```

new:
```markdown
- `role="api_keys"` exists **only** when the API-key cache holds its own connection — since
  SMA-485 that means `api_keys.introspect_cache.redis_url` differs from `authz.cache.redis_url`
  as a **string** (trimmed), or the authz cache is `memory`-backed. Ordinarily the two URLs are
  identical and the API-key cache reuses the `authz` handle, so a missing `api_keys` series does
  not mean the API-key cache is idle — check `role="authz"` instead. The comparison being textual
  cuts the other way too: two spellings of one endpoint (`…:6379` vs `…:6379/0`, a host alias, a
  differing password) produce an `api_keys` series fronting the same physical Redis, which is the
  next caveat's case arrived at by accident rather than by design.
```

- [ ] **Step 4: The breaker overview (currently 1374-1375)**

old:
```markdown
circuit breaker (`adapters::redis_conn::RedisHandle`; one breaker per connection — one instance per
`RedisRole`, i.e. `authz`, `api_keys` in the split configuration, and `jwks`) now sits in front of
```

new:
```markdown
circuit breaker (`adapters::redis_conn::RedisHandle`; one breaker per connection — one instance per
`RedisRole`, i.e. `authz`, `api_keys` when its cache holds its own connection, and `jwks`) now sits in front of
```

- [ ] **Step 5: `IamRedisBreakerOpen`'s meaning (currently 1083-1084)**

old:
```markdown
the per-connection Redis circuit breaker (SMA-476) for `role` (`authz`, or `api_keys` in the split
configuration) has read Open or HalfOpen for at least 2 minutes, not just a momentary probe.
```

new:
```markdown
the per-connection Redis circuit breaker (SMA-476) for `role` (`authz`, or `api_keys` when that
cache holds its own connection) has read Open or HalfOpen for at least 2 minutes, not just a
momentary probe.
```

- [ ] **Step 6: The metric table (currently line 96)**

In the `iam_redis_breaker_state` row, replace:

```
`role` ∈ `authz`/`api_keys`/`jwks` (closed set — `api_keys` exists only in the split configuration).
```

with:

```
`role` ∈ `authz`/`api_keys`/`jwks` (closed set — `api_keys` exists only when that cache holds its own connection: distinct `redis_url`s, or a memory-backed authz cache — SMA-485).
```

- [ ] **Step 7: Verify no stale statement of the old rule survives**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-485-api-key-redis-connection
grep -n "split configuration" docs/ops/RUNBOOK-observability.md
grep -rn 'only when `authz.cache.backend' docs/ops/RUNBOOK-observability.md
```

Expected: the surviving "split configuration" mentions read as *descriptions of a deployment shape* (e.g. ":1437", ":1499"), never as *the definition of when `role="api_keys"` exists*. The second grep returns nothing. Read each hit and confirm rather than counting them.

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-485-api-key-redis-connection
git add docs/ops/RUNBOOK-observability.md
git commit -F - <<'EOF'
docs(ops): restate when role="api_keys" exists (SMA-485)

The runbook defined the split configuration as "authz is memory-backed"
in five places. Since SMA-485 the API-key cache holds its own connection
whenever the two redis_urls differ textually, so the definition changes
and the hottest-path paragraph's claim about when the handle is reused
was outright false. Adds a remediation entry for the new boot failure: a
previously ignored URL is now dialled, and the fast fix is to make it
byte-identical to the authz one.
EOF
```

---

### Task 6: Full gate run

**Files:** none — verification only.

**Interfaces:**
- Consumes: Tasks 1-5.
- Produces: evidence the branch is CI-clean.

- [ ] **Step 1: Run the full affected graph the way CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-485-api-key-redis-connection
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: every action passes.

- [ ] **Step 2: If anything failed, identify which**

Moon reports an unattributed "N failed" in non-TTY output. Get the actual failing action:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-485-api-key-redis-connection
jq '.actions[] | select(.status == "failed") | {label, status}' .moon/cache/ciReport.json
```

Fix and re-run Step 1. Do not proceed with a red gate.

- [ ] **Step 3: Confirm the diff matches the plan**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-485-api-key-redis-connection
git diff origin/main --stat
```

Expected exactly these eight paths:

```
docs/superpowers/specs/2026-08-07-sma-485-api-key-cache-redis-connection-design.md
docs/superpowers/plans/2026-08-07-sma-485-api-key-cache-redis-connection.md
docs/ops/RUNBOOK-observability.md
rs/crates/libs/paigasus-observability/src/names.rs
rs/crates/services/paigasus-iam/src/adapters/api_keys/cache.rs
rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
rs/crates/services/paigasus-iam/src/main.rs
rs/crates/services/paigasus-iam/tests/api_key_cache_connection.rs
```

Anything else is stray — investigate before opening the PR.

- [ ] **Step 4: Confirm no debug residue**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-485-api-key-redis-connection
git diff origin/main -- '*.rs' | grep -nE '^\+.*(dbg!|println!|eprintln!|TODO|FIXME|#\[ignore\])' || echo "clean"
```

Expected: `clean`. (`eprintln!` in the Docker-skip path of the new test is legitimate — it mirrors every other Redis suite — so if it appears, confirm that is the only hit.)

- [ ] **Step 5: Carry the operational surprise into the PR description**

Spec §7 requires this in the PR body, not only in the RUNBOOK. Record the text now so it is not lost at PR time:

> **Operators:** `api_keys.introspect_cache.redis_url` is now actually dialled at boot. It was previously ignored whenever `authz.cache.backend = "redis"`, so a wrong or unreachable value was harmless — and `IamConfig::validate` requires the field, so configs written under the old behaviour may well contain a stale or placeholder value. Such a deployment will now **fail to start**. Fix the endpoint, or set it byte-identical to `authz.cache.redis_url` to restore the previous behaviour exactly. A deployment whose two URLs already match sees no change; one whose URLs differ gains a second Redis connection, its own circuit breaker, and a new `iam_redis_breaker_state{role="api_keys"}` series.

---

## Notes for the implementer

**On line numbers.** Every line number in Tasks 3-5 is relative to `origin/main`, before any task has run. Tasks 1 and 3 both edit `src/adapters/http/mod.rs`, so by Task 4 the `connect_redis` doc has moved. Match on the quoted **old text**, which is exact; treat the line number as a hint about where to look.

**On the `AppState` not-`Debug` trap.** `AppState` derives `Clone` only, so `Result<AppState, _>::unwrap_err()` and `expect_err()` do not compile (`Result::unwrap_err` requires `T: Debug`). Task 2's test uses `.err().map(|e| e.to_string())`. If you find yourself reaching for `unwrap_err`, that is why it fails.

**On the metrics recorder.** `paigasus_observability::init` is a process-global `OnceLock`. Installing it after the first `AppState::new` silently drops every gauge set before it, and the negative assertions in Task 2 would then pass while proving nothing. It must be the first statement in the test.

**On running only part of the suite.** Per-project Moon tasks do not run the repo-level gates. `cargo nextest run -p paigasus-iam` passing is necessary, not sufficient — Task 6 is what CI actually runs.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
