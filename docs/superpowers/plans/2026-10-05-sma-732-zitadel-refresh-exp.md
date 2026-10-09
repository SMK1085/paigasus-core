# SMA-732 Zitadel refresh `exp` measurement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Measure, in a committed Docker e2e test, whether a Zitadel refresh grant gives an access token with a new `exp`, and record the result.

**Architecture:** The Zitadel container start of `zitadel_e2e.rs` moves into one helper, `start_zitadel`, that returns a `ZitadelInstance`. The SMA-703 test calls it with no extra env vars. A new test, `zitadel_refresh_extends_access_token_exp`, calls it with an access token lifetime of 10 s. It logs in, refreshes three times on a fixed schedule, and checks the four access tokens with Zitadel token times only. A measurement doc holds the real output of the run and the mutation results.

**Tech Stack:** Rust edition 2024 (rust-version 1.95), tokio (`time` and `test-util` features are already on), testcontainers-modules, reqwest, serde_json, cargo-nextest, Zitadel `ghcr.io/zitadel/zitadel:v4.15.3`, Postgres `16-alpine`.

**Spec:** `docs/superpowers/specs/2026-10-05-sma-732-zitadel-refresh-exp-design.md` (APPROVED). Read it in full before you start a task. The section numbers (§), K1-K6, D1-D8 and A1-A8 below refer to it. Do not re-open D1-D8.

## Global Constraints

- Every command in Tasks 1, 2 and 4 runs from `rs/` (Task 3 runs from the worktree root) in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-732-zitadel-refresh-exp`. Start each shell with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- The test command is `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test zitadel_e2e --no-capture`. A filtered run needs `PAIGASUS_REQUIRE_DOCKER=1`: without it, a missing Docker gives a silent pass (rs/CLAUDE.md).
- `--no-capture` makes nextest run the tests one at a time. So the two tests of the binary do not overlap in that run. Task 4 runs the binary once without `--no-capture`, so the two tests run in parallel as in CI (spec § 6).
- Lint command (the same flags as `.moon/tasks/rust.yml` `lint`): `cargo clippy -p paigasus-iam --locked --all-targets -- -D warnings`. Format command: `cargo fmt --check`. `rs/rustfmt.toml` sets `max_width = 200`.
- The workspace sets `warnings = "deny"`. A test binary with an unused variable, an unused field or an unused function does not compile. Each mutation must COMPILE: it changes a value, it never adds dead code.
- Restore a mutation by hand: delete the inserted text, or reverse the one-token edit. Never use `git checkout`, `git restore` or `git stash` for this. After each restore, `git diff --exit-code -- crates/services/paigasus-iam/tests/zitadel_e2e.rs` must exit 0.
- Never print a raw access token, ID token or refresh token. Print only claims, `expires_in`, offsets and booleans. Panic messages follow the same rule.
- `L` is 10 s (D3). Do not change `L`, the waits or the scope. If `iat1 < exp0` reds, STOP (see the STOP rule).
- The SMA-703 test keeps every assertion unchanged (D2). It keeps its container order: IAM Postgres first (`start_migrated_postgres`), then Zitadel.
- No change to `paigasus-auth`, the runbook or the chart (D4).
- SPDX header `// SPDX-License-Identifier: Apache-2.0` stays line 1 of `zitadel_e2e.rs`. The Markdown specs in `docs/superpowers/specs/` have no SPDX header; do not add one.
- Commits: conventional commits with a scope (`test(rs): …`, `docs(rs): …`). End each message with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never use `--no-verify`. Do not put a line that starts with `#` followed by a number in the commit body (commitlint `footer-leading-blank`). If the commit fails with "failed to fill whole buffer", 1Password is locked: stop and ask the coordinator.

## STOP rule

Stop the work and report to the coordinator, with the full printed `SMA-732` lines and the panic message, when one of these occurs. Do not change `L`, the waits, the assertions or the scope.

1. The final panic names outcome `"keeps"` (D5).
2. The panic names `A8` and outcome `"refuses after exp"` (D5). (The M3 mutation run is the one exception: that panic is the expected result of M3.)
3. A failure line starts with `A5: iat1` (`iat1 < exp0` is red, D3).

Any other red check is a defect in the test (D5). Fix the test, not the expectation, and say what you fixed in the report.

## Review Focus

1. A token response without one of its fields: the panic must name the missing field and the response KEYS, never the response body, because the body holds tokens. `secret_field` pins this; Task 2 Step 6 greps for any token variable in a format string.
2. A slow runner: the step-4 refresh can arrive after `exp0`. The test must then fail with the D3 message (`A5: iat1 …`), not with a misleading "keeps" message. Task 2 builds the verdict with a separate D3 branch.
3. Zitadel ignores the lifetime env var (for example after a rename in a new tag): every token then has `exp - iat` near 43200. A1 must red with the real numbers. M1 proves that A1 reds on a wrong `L`.
4. A refresh that returns a non-2xx status: the panic must carry the status and the OAuth error body (no tokens in it) and the token lines so far. M3 pins the step-5 case.
5. Docker missing on a filtered run: the test must panic, not pass quietly. `PAIGASUS_REQUIRE_DOCKER=1` on every run command pins this.

---

### Task 1: Extract `start_zitadel` and `ZitadelInstance` (pure refactor)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/zitadel_e2e.rs:88-192` (the start of the SMA-703 test) and add the helper after `ZitadelApi`'s `impl` block (after line 384).

**Interfaces:**
- Consumes: `support::docker::start_or_skip<T, I>(image: T, what: &str) -> Option<ContainerAsync<I>>`, `support::docker::mapped_port(src: &impl PortSource, port: u16, what: &str) -> u16`, `wait_for_postgres(&ContainerAsync<Postgres>)`, `read_admin_pat(&ContainerAsync<GenericImage>) -> String`, `dump_logs(&ContainerAsync<GenericImage>) -> String`, `ZitadelApi<'a> { http: &'a reqwest::Client, base: &'a str, pat: &'a str }` with `wait_until_ready(&self) -> bool`.
- Produces (Task 2 uses these exact names):
  - `struct ZitadelInstance { _zitadel_pg: ContainerAsync<Postgres>, zitadel: ContainerAsync<GenericImage>, http: reqwest::Client, issuer: String, pat: String }`
  - `impl ZitadelInstance { fn api(&self) -> ZitadelApi<'_> }`
  - `async fn start_zitadel(extra_env: &[(&str, &str)]) -> Option<ZitadelInstance>`

- [ ] **Step 1: Prove the baseline is green**

Run (from `rs/`):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test zitadel_e2e --no-capture
```

Expected: `1 test run: 1 passed` for `zitadel_id_tokens_are_refused_by_the_marker_claims`. If it fails before any change, STOP and report: the refactor needs a green baseline.

- [ ] **Step 2: Add the struct, its `api` method and the helper**

Insert this block directly after the closing `}` of `impl ZitadelApi<'_>` (after the current line 384). The body of `start_zitadel` is lines 95-191 of the current file with the same comments. The only changes are: `extra_env` is applied after the fixed env vars, each `return;` becomes `return None;`, and the result is a `ZitadelInstance`.

```rust
/// A running Zitadel with its own Postgres, and the client, issuer and admin PAT to call it.
/// The containers stop when this value is dropped, so a test keeps it alive to its end.
struct ZitadelInstance {
    /// Never read. It is here only so that the Postgres container lives as long as Zitadel.
    _zitadel_pg: ContainerAsync<Postgres>,
    zitadel: ContainerAsync<GenericImage>,
    /// One client for every call of a test. It never follows a redirect, because the login
    /// flow must read each `Location` header, and the last one points at a server that does not
    /// exist.
    http: reqwest::Client,
    /// `https://127.0.0.1:{mapped port}` (see the module doc).
    issuer: String,
    /// The first-instance admin PAT.
    pat: String,
}

impl ZitadelInstance {
    /// The management API of this instance.
    fn api(&self) -> ZitadelApi<'_> {
        ZitadelApi {
            http: &self.http,
            base: &self.issuer,
            pat: &self.pat,
        }
    }
}

/// Starts Zitadel's own Postgres and Zitadel on a Docker network of their own, waits until
/// discovery and the management API answer, and reads the admin PAT. `extra_env` adds env vars
/// to the Zitadel container after the fixed ones (an empty slice keeps the defaults). Returns
/// `None` when `start_or_skip` skips (no Docker, per `tests/support/docker.rs`).
async fn start_zitadel(extra_env: &[(&str, &str)]) -> Option<ZitadelInstance> {
    // Zitadel's own Postgres, on a network of its own. The suffix keeps parallel runs apart.
    let suffix = format!("{:016x}", rand::random::<u64>());
    let network = format!("zitadel-e2e-{suffix}");
    let pg_host = format!("zitadel-e2e-pg-{suffix}");
    let zitadel_pg_image = Postgres::default().with_tag("16-alpine").with_network(&network).with_container_name(&pg_host);
    let zitadel_pg = support::docker::start_or_skip(zitadel_pg_image, "zitadel_e2e postgres").await?;
    // The Postgres module reports ready on its first log line, while the init server (unix
    // socket only) still runs. Zitadel stops at once when its first connection fails, so wait
    // for a TCP connection, which only the real server accepts.
    wait_for_postgres(&zitadel_pg).await;

    // A runtime self-signed cert for Zitadel's TLS listener, copied into the container.
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()]).expect("self-signed cert");
    let cert_pem = cert.cert.pem().into_bytes();
    let key_pem = cert.signing_key.serialize_pem().into_bytes();

    let mut image = GenericImage::new(ZITADEL_IMAGE, ZITADEL_TAG)
        .with_exposed_port(HTTPS_PORT.tcp())
        .with_network(&network)
        // The image user cannot write to a directory that `with_copy_to` makes (root owns it),
        // and Zitadel must write the admin PAT there.
        .with_user("0")
        .with_env_var("ZITADEL_MASTERKEY", "MasterkeyNeedsToHave32Characters")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_HOST", &pg_host)
        .with_env_var("ZITADEL_DATABASE_POSTGRES_PORT", "5432")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_DATABASE", "zitadel")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_USER_USERNAME", "zitadel")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_USER_PASSWORD", "zitadel")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_USER_SSL_MODE", "disable")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_ADMIN_USERNAME", "postgres")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_ADMIN_PASSWORD", "postgres")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_ADMIN_SSL_MODE", "disable")
        // See the module doc: the host of `iss` comes from here, the port from the request.
        .with_env_var("ZITADEL_EXTERNALDOMAIN", "127.0.0.1")
        .with_env_var("ZITADEL_EXTERNALPORT", HTTPS_PORT.to_string())
        .with_env_var("ZITADEL_EXTERNALSECURE", "true")
        .with_env_var("ZITADEL_TLS_ENABLED", "true")
        .with_env_var("ZITADEL_TLS_CERTPATH", format!("{STATE_DIR}/tls.crt"))
        .with_env_var("ZITADEL_TLS_KEYPATH", format!("{STATE_DIR}/tls.key"))
        .with_env_var("ZITADEL_FIRSTINSTANCE_PATPATH", format!("{STATE_DIR}/admin.pat"))
        .with_env_var("ZITADEL_FIRSTINSTANCE_ORG_MACHINE_MACHINE_USERNAME", "e2e-admin")
        .with_env_var("ZITADEL_FIRSTINSTANCE_ORG_MACHINE_MACHINE_NAME", "e2e-admin")
        .with_env_var("ZITADEL_FIRSTINSTANCE_ORG_MACHINE_PAT_EXPIRATIONDATE", "2099-01-01T00:00:00Z")
        // The measurement used Login v1. This keeps the instance on it.
        .with_env_var("ZITADEL_DEFAULTINSTANCE_FEATURES_LOGINV2_REQUIRED", "false")
        .with_copy_to(format!("{STATE_DIR}/tls.crt"), cert_pem)
        .with_copy_to(format!("{STATE_DIR}/tls.key"), key_pem)
        .with_cmd(["start-from-init", "--masterkeyFromEnv", "--tlsMode", "enabled"])
        .with_startup_timeout(Duration::from_secs(240));
    for (name, value) in extra_env {
        image = image.with_env_var(*name, *value);
    }

    let zitadel = support::docker::start_or_skip(image, "zitadel_e2e").await?;
    let https_port = support::docker::mapped_port(&zitadel, HTTPS_PORT, "zitadel https").await;
    let issuer = format!("https://127.0.0.1:{https_port}");

    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .expect("reqwest client");

    // Poll discovery until Zitadel serves, then pin the issuer form (see the module doc).
    let discovery_url = format!("{issuer}/.well-known/openid-configuration");
    let mut discovery = None;
    for _ in 0..READINESS_ATTEMPTS {
        if let Ok(response) = http.get(&discovery_url).send().await
            && response.status().is_success()
            && let Ok(body) = response.json::<Value>().await
        {
            discovery = Some(body);
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let Some(discovery) = discovery else {
        panic!("zitadel discovery never became ready at {discovery_url}\n{}", dump_logs(&zitadel).await);
    };
    assert_eq!(discovery["issuer"], issuer, "zitadel must derive the issuer from the request Host: {discovery}");

    let pat = read_admin_pat(&zitadel).await;
    let instance = ZitadelInstance {
        _zitadel_pg: zitadel_pg,
        zitadel,
        http,
        issuer,
        pat,
    };
    // MEASURED: discovery answers before the management API does. The REST gateway first
    // returns 503 `dial tcp [::1]:8080: connect: connection refused` for a few seconds.
    if !instance.api().wait_until_ready().await {
        panic!("the zitadel management API never became ready\n{}", dump_logs(&instance.zitadel).await);
    }
    Some(instance)
}
```

Notes for the implementer:
- `with_network` turns the `GenericImage` into a `ContainerRequest<GenericImage>`. So `image` has that type, and `image.with_env_var(..)` returns the same type. The loop compiles.
- An `extra_env` key that is equal to a fixed key replaces the fixed value (the later `with_env_var` call wins). The SMA-732 key is not among the fixed keys.

- [ ] **Step 3: Replace lines 95-192 of the SMA-703 test with the helper call**

In `zitadel_id_tokens_are_refused_by_the_marker_claims`, delete everything from the line `// Zitadel's own Postgres, on a network of its own. …` (line 95) to the line `let setup = setup_zitadel(&zitadel_api).await;` (line 192), both included. Put this in its place:

```rust
    // Zitadel and its own Postgres (see `start_zitadel`). This test keeps the instance defaults.
    let Some(zitadel) = start_zitadel(&[]).await else {
        return;
    };
    // Owned copies, so that the rest of this test is unchanged. A `reqwest::Client` clone shares
    // the same connection pool.
    let http = zitadel.http.clone();
    let issuer = zitadel.issuer.clone();
    let setup = setup_zitadel(&zitadel.api()).await;
```

Lines 90-93 (`start_migrated_postgres` first) stay as they are. Everything from `// The human flow: code + PKCE through Login v1, …` to the end of the test stays byte-identical. `zitadel` lives to the end of the test, so both containers live to the end.

- [ ] **Step 4: Compile, format and lint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo fmt --check
cargo clippy -p paigasus-iam --locked --all-targets -- -D warnings
```

Expected: both exit 0. If `cargo fmt --check` prints a diff, run `cargo fmt` and check that the diff only touches the lines of this task.

- [ ] **Step 5: Prove the SMA-703 test is unchanged in behaviour**

```bash
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test zitadel_e2e --no-capture
```

Expected: `1 test run: 1 passed`, not `FLAKY`. Then check that the diff touches no assertion of the SMA-703 test:

```bash
git diff -U0 -- crates/services/paigasus-iam/tests/zitadel_e2e.rs | grep -E '^-.*(assert|expect_err|matches!)' || echo "no assertion removed"
```

Expected: `no assertion removed`. (The removed `assert_eq!(discovery["issuer"], …)` line is an exception only if the grep shows it: it moved into `start_zitadel` unchanged. Check that the same line is on a `+` line.)

- [ ] **Step 6: Commit**

```bash
git add crates/services/paigasus-iam/tests/zitadel_e2e.rs
git commit -m "test(rs): extract the Zitadel container start into start_zitadel (SMA-732)

The SMA-703 test now calls start_zitadel with no extra env vars. Its
assertions do not change. The SMA-732 refresh test needs its own
instance with a different access token lifetime (spec D2).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The refresh test, the module doc, the nextest comment and the mutation battery

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/zitadel_e2e.rs` (module doc lines 1-31, imports lines 35-53, constants after line 86, new test after the SMA-703 test, new helpers after `start_zitadel`)
- Modify: `rs/.config/nextest.toml:73-81` (the `binary(zitadel_e2e)` comment)

**Interfaces:**
- Consumes: `start_zitadel(&[(&str, &str)]) -> Option<ZitadelInstance>`, `ZitadelInstance::api`, `ZitadelInstance { http, issuer, .. }` (Task 1); `setup_zitadel(&ZitadelApi<'_>) -> ZitadelSetup`; `human_login(&reqwest::Client, &str, &ZitadelSetup) -> Value`; `token_request(&reqwest::Client, &str, Option<(&str, &str)>, &[(&str, &str)]) -> Result<Value, (StatusCode, String)>`; `jwt_payload(&str) -> Value`; `ZitadelSetup { client_id: String, client_secret: String, .. }`.
- Produces: the test `zitadel_refresh_extends_access_token_exp`; the printed lines `SMA-732 T0: …`, `SMA-732 Tq: …`, `SMA-732 T1: …`, `SMA-732 T2: …`, `SMA-732 waiting …`, `SMA-732 outcome: …` that Task 3 pastes into the measurement doc.

TDD framing: this is a measurement against a real Zitadel. A red-before-green step is not possible, because the test measures an external fact. The "failing first" evidence is the mutation battery (Steps 9-12, spec § 7): each mutation must red the named checks.

- [ ] **Step 1: Add the imports**

In the `use` block, add these two lines (keep the block sorted the way `cargo fmt` sorts it):

```rust
use std::collections::BTreeSet;
use tokio::time::Instant;
```

The file has no `std::time::Instant` import, so `Instant` means `tokio::time::Instant` everywhere in this file.

- [ ] **Step 2: Add the constants**

Insert after `ADD_EMAIL_CLAIM_SCRIPT` (after the current line 86):

```rust
/// SMA-732: the env var that sets the access token lifetime of the default instance
/// (`cmd/defaults.yaml:1302` of Zitadel v4.15.3, spec D6).
const ACCESS_TOKEN_LIFETIME_ENV: &str = "ZITADEL_DEFAULTINSTANCE_OIDCSETTINGS_ACCESSTOKENLIFETIME";
/// SMA-732: the access token lifetime `L` of the refresh test's own instance, in seconds (spec D3).
/// If `iat1 < exp0` reds under load, the fix is a larger `L`, and only after Sven agrees.
const REFRESH_LIFETIME_SECS: i64 = 10;
/// SMA-732: the tolerance of A1 and A2 for the gap between two clock reads in one request (K1, K2).
const CLOCK_READ_TOLERANCE_SECS: i64 = 2;
/// SMA-732: the lower bound of A3 and A4, in seconds of Zitadel time.
const MIN_REFRESH_STEP_SECS: i64 = 6;
/// SMA-732 step 4: refresh 1 starts this long after the login response.
const REFRESH_1_AT: Duration = Duration::from_secs(7);
/// SMA-732 step 5: refresh 2 starts at the later of this time after the login response …
const REFRESH_2_AT: Duration = Duration::from_secs(13);
/// … and this time after the refresh 1 response.
const REFRESH_2_AFTER_REFRESH_1: Duration = Duration::from_secs(6);
```

- [ ] **Step 3: Add the helpers**

Insert after `start_zitadel` (from Task 1):

```rust
/// The times of one access token of the SMA-732 test, from its JWT and from its token response.
/// It holds no token: only claims, `expires_in`, a host offset and a boolean.
struct TokenTimes {
    label: &'static str,
    iat: i64,
    nbf: i64,
    exp: i64,
    jti: String,
    expires_in: i64,
    /// Host milliseconds since the login response. Only the waits use host time; no check reads it.
    offset_ms: u128,
    /// True when the response's refresh token differs from the one that the request sent.
    /// `None` for the login, which sent no refresh token.
    refresh_rotated: Option<bool>,
}

impl TokenTimes {
    /// Reads the access token times of `response`. Returns them with the response's refresh
    /// token. The caller must never put that refresh token into a message.
    fn read(label: &'static str, response: &Value, start: Instant, previous_refresh: Option<&str>) -> (Self, String) {
        let access = secret_field(response, "access_token", label);
        let refresh = secret_field(response, "refresh_token", label);
        let claims = jwt_payload(&access);
        let times = Self {
            label,
            iat: int_claim(&claims, "iat", label),
            nbf: int_claim(&claims, "nbf", label),
            exp: int_claim(&claims, "exp", label),
            jti: claims["jti"].as_str().unwrap_or_else(|| panic!("{label}: the access token has no string jti claim: {claims}")).to_string(),
            expires_in: response["expires_in"]
                .as_i64()
                .unwrap_or_else(|| panic!("{label}: the token response has no integer expires_in. Keys: {:?}", response_keys(response))),
            offset_ms: start.elapsed().as_millis(),
            refresh_rotated: previous_refresh.map(|previous| previous != refresh),
        };
        (times, refresh)
    }
}

impl std::fmt::Display for TokenTimes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let rotated = match self.refresh_rotated {
            Some(rotated) => rotated.to_string(),
            None => "n/a".to_string(),
        };
        write!(
            f,
            "{}: iat={} nbf={} exp={} exp-iat={} expires_in={} jti={} offset_ms={} refresh_rotated={rotated}",
            self.label,
            self.iat,
            self.nbf,
            self.exp,
            self.exp - self.iat,
            self.expires_in,
            self.jti,
            self.offset_ms
        )
    }
}

/// One refresh grant of the console's confidential web app (HTTP Basic client auth).
async fn refresh(http: &reqwest::Client, issuer: &str, setup: &ZitadelSetup, refresh_token: &str) -> Result<Value, (StatusCode, String)> {
    token_request(
        http,
        issuer,
        Some((&setup.client_id, &setup.client_secret)),
        &[("grant_type", "refresh_token"), ("refresh_token", refresh_token)],
    )
    .await
}

/// Sleeps until `deadline` and prints how long it waits, so that the run output shows the real
/// schedule. Returns at once when the deadline has passed.
async fn sleep_until(deadline: Instant, what: &str) {
    let wait = deadline.saturating_duration_since(Instant::now());
    println!("SMA-732 waiting {} ms for {what}", wait.as_millis());
    tokio::time::sleep_until(deadline).await;
}

/// A string field of a token response that holds a secret. The panic names the keys of the
/// response, never the response, because the response holds tokens.
fn secret_field(response: &Value, field: &str, label: &str) -> String {
    response[field]
        .as_str()
        .unwrap_or_else(|| panic!("{label}: no string {field} in the token response. Keys: {:?}", response_keys(response)))
        .to_string()
}

/// An integer claim of a decoded access token payload. Claims hold no secret, so the panic
/// prints them.
fn int_claim(claims: &Value, name: &str, label: &str) -> i64 {
    claims[name].as_i64().unwrap_or_else(|| panic!("{label}: the access token has no integer {name} claim: {claims}"))
}

/// The top-level keys of a JSON object, for the panic messages of `secret_field`.
fn response_keys(response: &Value) -> Vec<String> {
    response.as_object().map(|object| object.keys().cloned().collect()).unwrap_or_default()
}
```

- [ ] **Step 4: Add the test**

Insert directly after the closing `}` of `zitadel_id_tokens_are_refused_by_the_marker_claims`:

```rust
/// SMA-732: does a refresh give an access token with a new `exp`? See the module doc and the
/// spec `2026-10-05-sma-732-zitadel-refresh-exp-design.md` (§ 4.2: the steps and A1-A8).
#[tokio::test]
async fn zitadel_refresh_extends_access_token_exp() {
    // Step 1: an instance of its own, with L = 10 s (spec D2, D6). No IAM Postgres (spec D7).
    let lifetime = format!("{REFRESH_LIFETIME_SECS}s");
    let Some(zitadel) = start_zitadel(&[(ACCESS_TOKEN_LIFETIME_ENV, lifetime.as_str())]).await else {
        return;
    };
    let setup = setup_zitadel(&zitadel.api()).await;
    let http = &zitadel.http;
    let issuer = zitadel.issuer.as_str();

    // Step 2: the login gives T0 and R0. `start` is the host base of every wait.
    let login = human_login(http, issuer, &setup).await;
    let start = Instant::now();
    let (t0, r0) = TokenTimes::read("T0", &login, start, None);
    println!("SMA-732 {t0}");

    // Step 3: an immediate refresh (K4: the M4a case of SMA-703). Only A1, A2, A6 and A7 read Tq.
    let tq_response = refresh(http, issuer, &setup, &r0)
        .await
        .unwrap_or_else(|(status, body)| panic!("the immediate refresh (Tq) failed ({status}): {body}\n{t0}"));
    let (tq, rq) = TokenTimes::read("Tq", &tq_response, start, Some(&r0));
    println!("SMA-732 {tq}");

    // Step 4: refresh 1, before the first exp.
    sleep_until(start + REFRESH_1_AT, "refresh 1 (before exp0)").await;
    let t1_response = refresh(http, issuer, &setup, &rq)
        .await
        .unwrap_or_else(|(status, body)| panic!("refresh 1 (T1) failed ({status}): {body}\n{t0}\n{tq}"));
    let t1_arrived = Instant::now();
    let (t1, r1) = TokenTimes::read("T1", &t1_response, start, Some(&rq));
    println!("SMA-732 {t1}");

    // Step 5: refresh 2, after the first exp. The wait is relative to t1 too, so A4 is a lower
    // bound under any load. Mutation M3 replaces `&r1` with `&r0` in the next call.
    sleep_until((start + REFRESH_2_AT).max(t1_arrived + REFRESH_2_AFTER_REFRESH_1), "refresh 2 (after exp0)").await;
    let t2_response = refresh(http, issuer, &setup, &r1).await.unwrap_or_else(|(status, body)| {
        panic!("A8: outcome \"refuses after exp\" (spec D5): refresh 2 (T2) failed ({status}): {body}\n{t0}\n{tq}\n{t1}")
    });
    let (t2, _) = TokenTimes::read("T2", &t2_response, start, Some(&r1));
    println!("SMA-732 {t2}");

    // Step 6: the checks. Each failure is collected, so one run shows every red check.
    let tokens = [&t0, &tq, &t1, &t2];
    let report = tokens.iter().map(|t| t.to_string()).collect::<Vec<_>>().join("\n");
    let mut failures: Vec<String> = Vec::new();

    // A1 (K1). Mutation M1 inserts ` + 3` after `REFRESH_LIFETIME_SECS` on the next line.
    let a1_lifetime = REFRESH_LIFETIME_SECS;
    for t in tokens {
        let lifetime = t.exp - t.iat;
        if !(a1_lifetime - CLOCK_READ_TOLERANCE_SECS..=a1_lifetime).contains(&lifetime) {
            failures.push(format!(
                "A1: {}: exp - iat = {lifetime}, expected {}..={a1_lifetime} (spec K1)",
                t.label,
                a1_lifetime - CLOCK_READ_TOLERANCE_SECS
            ));
        }
        if t.nbf != t.iat {
            failures.push(format!("A1: {}: nbf {} != iat {} (spec K1)", t.label, t.nbf, t.iat));
        }
    }

    // A2 (K2).
    for t in tokens {
        if !(REFRESH_LIFETIME_SECS - CLOCK_READ_TOLERANCE_SECS..=REFRESH_LIFETIME_SECS).contains(&t.expires_in) {
            failures.push(format!(
                "A2: {}: expires_in = {}, expected {}..={REFRESH_LIFETIME_SECS} (spec K2)",
                t.label,
                t.expires_in,
                REFRESH_LIFETIME_SECS - CLOCK_READ_TOLERANCE_SECS
            ));
        }
    }

    // A3-A5 compare T1 and T2 with T0. Mutation M2 inserts `.map(|_| &t0)` after `[&t1, &t2]`.
    let [later1, later2] = [&t1, &t2];
    let keeps = later1.exp == t0.exp || later2.exp == t0.exp;
    let keeps_note = if keeps { " Outcome \"keeps\" (spec D5): exp did not move." } else { "" };

    // A3.
    if later1.iat - t0.iat < MIN_REFRESH_STEP_SECS || later1.exp - t0.exp < MIN_REFRESH_STEP_SECS {
        failures.push(format!(
            "A3: {} vs T0: iat +{}, exp +{}, expected both >= {MIN_REFRESH_STEP_SECS}.{keeps_note}",
            later1.label,
            later1.iat - t0.iat,
            later1.exp - t0.exp
        ));
    }
    // A4.
    if later2.iat - later1.iat < MIN_REFRESH_STEP_SECS || later2.exp - later1.exp < MIN_REFRESH_STEP_SECS {
        failures.push(format!(
            "A4: {} vs {}: iat +{}, exp +{}, expected both >= {MIN_REFRESH_STEP_SECS}.{keeps_note}",
            later2.label,
            later1.label,
            later2.iat - later1.iat,
            later2.exp - later1.exp
        ));
    }
    // A5.
    let late_refresh_1 = later1.iat >= t0.exp;
    if late_refresh_1 {
        failures.push(format!(
            "A5: iat1 {} >= exp0 {}: refresh 1 came after the first exp. Spec D3: do not remove this check; a larger L needs Sven's decision.",
            later1.iat, t0.exp
        ));
    }
    if later2.iat <= t0.exp {
        failures.push(format!("A5: iat2 {} <= exp0 {}: refresh 2 came before the first exp.{keeps_note}", later2.iat, t0.exp));
    }

    // A6.
    let jtis: BTreeSet<&str> = tokens.iter().map(|t| t.jti.as_str()).collect();
    if jtis.len() != tokens.len() {
        failures.push(format!("A6: the jti values are not all different: {jtis:?}"));
    }

    // A7 (K3: rotation). Printed as booleans, never as tokens.
    for t in [&tq, &t1, &t2] {
        if t.refresh_rotated != Some(true) {
            failures.push(format!("A7: {}: refresh_rotated = {:?}, expected Some(true) (spec K3)", t.label, t.refresh_rotated));
        }
    }

    let verdict = if keeps {
        "Outcome \"keeps\" (spec D5): Zitadel did not extend exp. STOP: report to the coordinator; do not change L or the scope."
    } else if late_refresh_1 {
        "iat1 >= exp0 (spec D3): the step-4 refresh was too slow. STOP: report to the coordinator; do not change L."
    } else {
        "No D5 outcome matched: a red check here is a defect in the test (spec D5)."
    };
    assert!(
        failures.is_empty(),
        "SMA-732: {} check(s) failed. {verdict}\n{}\n--- tokens ---\n{report}",
        failures.len(),
        failures.join("\n")
    );
    println!("SMA-732 outcome: \"extends\" (spec D5): A1-A8 pass");
}
```

Notes for the implementer:
- `later1` and `later2` exist only for mutation M2. `[&t1, &t2]` is an array of `&TokenTimes`, and `.map(|_| &t0)` keeps that type. So M2 compiles, and `t1` and `t2` stay in use through `tokens`.
- The `_` in `let (t2, _)` drops R2. The test does not need R2: A7 reads the boolean that `read` computed.
- The step-5 panic message does not contain `r0`, `rq` or `r1`. The OAuth error body holds no token.

- [ ] **Step 5: Update the module doc**

Replace the module doc lines 22-24:

```rust
//! Docker gating is the single policy of `tests/support/docker.rs`'s `start_or_skip` (SMA-538).
//! Three containers start: IAM's own Postgres, a second Postgres for Zitadel, and Zitadel. The
//! two Zitadel containers share a Docker network, so Zitadel reaches its Postgres by name.
```

with:

```rust
//! Docker gating is the single policy of `tests/support/docker.rs`'s `start_or_skip` (SMA-538).
//! The SMA-703 test starts three containers: IAM's own Postgres, a second Postgres for Zitadel,
//! and Zitadel. The SMA-732 test starts the two Zitadel containers only. The two Zitadel
//! containers share a Docker network, so Zitadel reaches its Postgres by name.
```

Then add this paragraph after line 31 (the end of the issuer paragraph, `//! asserts that the discovery `issuer` is exactly that value.`):

```rust
//!
//! SMA-732: `zitadel_refresh_extends_access_token_exp` measures whether a refresh gives an access
//! token with a new `exp` (spec `2026-10-05-sma-732-zitadel-refresh-exp-design.md`). It pins, as
//! measured facts for the pinned version: `exp - iat` is `L` or up to 2 s less, and `nbf == iat`
//! (K1: two clock reads); `expires_in` is in the same range (K2); a refresh 7 s and 13 s after the
//! login moves `iat` and `exp` by at least 6 s each, before and after the first `exp`; each
//! refresh issues a new refresh token (K3). It has its own Zitadel instance, because it sets the
//! access token lifetime `L` to 10 s with an env var, and the SMA-703 test must keep the defaults
//! (spec D2, D6). Its panic message names the outcome of spec D5: "keeps" when `exp` did not move,
//! "refuses after exp" when the refresh after the first `exp` fails. Both outcomes stop the work
//! until Sven decides the fix scope. Any other red check is a defect in the test.
```

- [ ] **Step 6: Check that no token reaches a message**

```bash
grep -nE '\{(r0|rq|r1|access|refresh|login|tq_response|t1_response|t2_response)[}:]' crates/services/paigasus-iam/tests/zitadel_e2e.rs || echo "no token in a format string"
```

Expected: `no token in a format string`. If the grep prints a line, remove that value from the message.

- [ ] **Step 7: Update the nextest comment**

In `rs/.config/nextest.toml`, replace the comment block above `filter = 'package(=paigasus-iam) and binary(zitadel_e2e)'` (the lines that start with `# SMA-703: zitadel_e2e starts Zitadel …` and end with `# `test-group`; it inherits `docker-containers` from the general block below.`) with:

```toml
# SMA-703 and SMA-732: zitadel_e2e holds two tests. Each test starts its own Zitadel and its own
# Postgres, because the SMA-732 test sets a different access token lifetime (SMA-732 spec D2).
# nextest runs the two tests as parallel processes, so two `start-from-init` runs share the
# runner. A failing start waits for the Postgres check (60 s), two readiness polls of 180
# attempts each (discovery, then the management API) and the PAT read (30 s). So one failing
# attempt can take about 7.5 minutes, and the general block's three attempts about 22.5 minutes,
# against ci.yml's 45-minute budget. One retry is kept for a transient container start. A timing
# flake in the SMA-732 test then shows as FLAKY, which is visible (SMA-732 spec D8).
# Scoped to the whole binary, for the same reason as keycloak_e2e above. Does NOT set
# `test-group`; it inherits `docker-containers` from the general block below.
```

Keep the `filter` and `retries = 1` lines unchanged.

- [ ] **Step 8: Format, lint and run the binary two times**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo fmt --check
cargo clippy -p paigasus-iam --locked --all-targets -- -D warnings
mkdir -p "${TMPDIR:-/tmp}/sma-732"
set -o pipefail
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test zitadel_e2e --no-capture 2>&1 | tee "${TMPDIR:-/tmp}/sma-732/run1.log"
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test zitadel_e2e --no-capture 2>&1 | tee "${TMPDIR:-/tmp}/sma-732/run2.log"
grep -h 'SMA-732' "${TMPDIR:-/tmp}/sma-732/run1.log" "${TMPDIR:-/tmp}/sma-732/run2.log"
grep -c FLAKY "${TMPDIR:-/tmp}/sma-732/run1.log" "${TMPDIR:-/tmp}/sma-732/run2.log"
```

Expected: fmt and clippy exit 0. Each run ends with `2 tests run: 2 passed`. Each run prints four token lines (`SMA-732 T0: …`, `Tq`, `T1`, `T2`), two `SMA-732 waiting …` lines and `SMA-732 outcome: "extends" …`. The `FLAKY` count is 0 for both logs. If a log shows `FLAKY`, record it for the measurement doc; it is a timing flake that D8 makes visible.

Apply the STOP rule here. If a run names "keeps" or "refuses after exp", or prints a failure line that starts with `A5: iat1`, stop and report. Do not change any value.

- [ ] **Step 9: Commit the test**

The mutation battery restores against this commit, so commit before you mutate.

```bash
git add crates/services/paigasus-iam/tests/zitadel_e2e.rs .config/nextest.toml
git commit -m "test(rs): measure whether a Zitadel refresh extends the access token exp (SMA-732)

A new e2e test starts its own Zitadel with an access token lifetime of
10 s, logs in, and refreshes before and after the first exp. It checks
the four access tokens with Zitadel token times only (spec A1-A8), and
its panic message names the D5 outcome.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 10: Mutation M1 (A1 must red)**

Edit the line `let a1_lifetime = REFRESH_LIFETIME_SECS;` to `let a1_lifetime = REFRESH_LIFETIME_SECS + 3;` (insert ` + 3`). Then run only the new test:

```bash
set -o pipefail
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test zitadel_e2e --no-capture -E 'test(=zitadel_refresh_extends_access_token_exp)' 2>&1 | tee "${TMPDIR:-/tmp}/sma-732/m1.log"
grep -E '^(A[0-9]|SMA-732)' "${TMPDIR:-/tmp}/sma-732/m1.log"
```

Expected: the test FAILS (both attempts, because of `retries = 1`). The panic lists four `A1: … exp - iat = …, expected 11..=13` lines, one per token, and says `No D5 outcome matched`. No other check fails. If the test passes, A1 does not bite: stop and report.

Restore: delete ` + 3`. Then:

```bash
git diff --exit-code -- crates/services/paigasus-iam/tests/zitadel_e2e.rs && echo "M1 restored"
```

Expected: `M1 restored`.

- [ ] **Step 11: Mutation M2 ("refresh keeps exp": A3, A4 and `iat2 > exp0` must red, naming "keeps")**

Edit the line `let [later1, later2] = [&t1, &t2];` to `let [later1, later2] = [&t1, &t2].map(|_| &t0);` (insert `.map(|_| &t0)`). Run:

```bash
set -o pipefail
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test zitadel_e2e --no-capture -E 'test(=zitadel_refresh_extends_access_token_exp)' 2>&1 | tee "${TMPDIR:-/tmp}/sma-732/m2.log"
grep -E '^(A[0-9]|SMA-732)' "${TMPDIR:-/tmp}/sma-732/m2.log"
```

Expected: the test FAILS. The panic says `Outcome "keeps" (spec D5)` and lists exactly three failures: `A3: T0 vs T0: iat +0, exp +0 …`, `A4: T0 vs T0: iat +0, exp +0 …` and `A5: iat2 … <= exp0 …`, each with the "keeps" note. `A5: iat1` must NOT appear (`iat0 < exp0`). If the list differs, stop and report.

Restore: delete `.map(|_| &t0)`. Then:

```bash
git diff --exit-code -- crates/services/paigasus-iam/tests/zitadel_e2e.rs && echo "M2 restored"
```

- [ ] **Step 12: Mutation M3 (refresh 2 with R0: record what Zitadel returns)**

In the step-5 call `refresh(http, issuer, &setup, &r1)`, change `&r1` to `&r0`. Change only that call; `TokenTimes::read("T2", …, Some(&r1))` stays. Run:

```bash
set -o pipefail
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test zitadel_e2e --no-capture -E 'test(=zitadel_refresh_extends_access_token_exp)' 2>&1 | tee "${TMPDIR:-/tmp}/sma-732/m3.log"
grep -E '^(A[0-9]|SMA-732)|refresh 2' "${TMPDIR:-/tmp}/sma-732/m3.log"
```

Expected by code reading (K3, NOT a requirement): an `A8:` panic with an HTTP 400 and an error body with `OIDCS-28ubl`. The "refuses after exp" label in that message comes from the mutation, not from Zitadel's `exp` handling. This is information, not an assertion: record the exact status and body, whatever they are. If Zitadel accepts R0, record that too, with the token lines and the check result.

Restore: change `&r0` back to `&r1` in that call. Then:

```bash
git diff --exit-code -- crates/services/paigasus-iam/tests/zitadel_e2e.rs && echo "M3 restored"
```

- [ ] **Step 13: Re-run the whole binary after the last restore**

```bash
set -o pipefail
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test zitadel_e2e --no-capture 2>&1 | tee "${TMPDIR:-/tmp}/sma-732/after-mutations.log"
```

Expected: `2 tests run: 2 passed`, the outcome line `"extends"`, and no `FLAKY`. Keep all six logs (`run1`, `run2`, `m1`, `m2`, `m3`, `after-mutations`) for Task 3. Nothing to commit in this step: `git status --short` shows a clean tree.

---

### Task 3: The measurement doc and the SMA-703 notes

**Files:**
- Create: `docs/superpowers/specs/2026-10-05-sma-732-zitadel-refresh-exp-measurements.md`
- Modify: `docs/superpowers/specs/2026-10-02-sma-703-zitadel-id-token-marker-claims-design.md` (§ 9 Q3 at line 356, table row at line 395)
- Modify: `docs/superpowers/specs/2026-10-02-sma-703-zitadel-measurements.md` (after line 519)

**Interfaces:**
- Consumes: the six logs of Task 2 in `${TMPDIR:-/tmp}/sma-732/`.
- Produces: the measurement doc that the PR links.

Rule for this task: every number, `jti`, status and error body in the doc comes from a log of Task 2, pasted as printed. Never invent, round or "clean up" a value. If a log does not hold a value that a section asks for, write "not measured" and say why.

This task runs only when Task 2 ended with outcome "extends". The STOP rule covers the other outcomes.

- [ ] **Step 1: Collect the facts**

Run every command of Task 3 from the worktree root, not from `rs/`.

```bash
L=${TMPDIR:-/tmp}/sma-732
date -u '+%Y-%m-%d %H:%M'
grep -h 'SMA-732' "$L/run1.log"
grep -h 'SMA-732' "$L/run2.log"
grep -hE '^(A[0-9]|SMA-732)' "$L/m1.log" "$L/m2.log"
grep -hE '^(A[0-9]|SMA-732)|refresh 2' "$L/m3.log"
grep -hE 'FLAKY|tests run' "$L"/*.log
```

- [ ] **Step 2: Write the measurement doc**

Create `docs/superpowers/specs/2026-10-05-sma-732-zitadel-refresh-exp-measurements.md` with these sections, in this order. The text in angle brackets says what to paste; replace each one with the real output, and delete the angle-bracket text.

````markdown
# SMA-732 Zitadel refresh `exp` measurement

- Date (UTC): <the `date -u` output of Step 1>
- Zitadel: `ghcr.io/zitadel/zitadel:v4.15.3`, Login v1, a confidential web app, JWT access tokens.
- Access token lifetime `L`: 10 s, set with `ZITADEL_DEFAULTINSTANCE_OIDCSETTINGS_ACCESSTOKENLIFETIME=10s`.
- Spec: `2026-10-05-sma-732-zitadel-refresh-exp-design.md`.
- Command (from `rs/`, Docker running):

  ```bash
  PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test zitadel_e2e --no-capture
  ```

## Result

Outcome: "extends" (spec D5). A refresh gives an access token with `exp` = refresh time + `L`, before
and after the first `exp`.

## Run 1

```text
<every `SMA-732` line of run1.log, as printed>
```

## Run 2

```text
<every `SMA-732` line of run2.log, as printed>
```

<If a run showed FLAKY, say so here with its log line. Else write: "Neither run was FLAKY.">

## What the run confirms

| Fact | Source before | Result of this run |
|---|---|---|
| K1: a refresh sets `exp` to refresh time + `L`; `exp - iat` is `L` or less; `nbf == iat` | code reading | <"confirmed" when A1, A3 and A4 passed in both runs; give the `exp-iat` values of the eight token lines> |
| K2: `expires_in` is in `L - 2 ..= L`; `iat + expires_in` is usually `exp - 1` | code reading | <give each token's `iat + expires_in` and `exp`, computed from the lines above, and say how many equal `exp - 1`> |
| K3: each refresh issues a new refresh token, and Zitadel refuses the old one | code reading | <"rotation confirmed" when every `refresh_rotated=true`; for the refusal, refer to M3 below> |
| K4: the SMA-703 M4a refresh came about 10 ms after the login, so its `iat`/`exp` looked equal | inference from `jti` values | <compare the `Tq` and `T0` lines: if `iat` and `exp` are equal and `offset_ms` of Tq is below 1000, write "consistent with K4: an immediate refresh shows the same second"; else give the values and say that K4 is not confirmed> |

## Mutations (spec § 7)

- M1, `L` in A1 only set to 13 s: <paste the `A1:` lines and the verdict line of m1.log>
- M2, T0 given to A3-A5 in place of T1 and T2: <paste the failure lines and the verdict line of m2.log>
- M3, refresh 2 with R0 in place of R1: <paste the status and the error body of the `A8:` line of m3.log, or the token lines if Zitadel accepted R0>. The "refuses after exp" label in this message comes from the mutation: R0 was already used, so the refusal is the K3 rotation check, not the `exp` check.
- After the last restore, the whole binary passed: <paste the `tests run` line of after-mutations.log>.

## Limits

- Login v1 only, a confidential web app, JWT access tokens, `L = 10 s`, Zitadel v4.15.3.
- The production default of `L` is 12 h. A deployment can use Login v2 or opaque access tokens.
  The code path for these is the same (K1), but that is code reading, not a measurement.
- The waits use host time; every check uses Zitadel token times only (spec § 4.2).
- `--no-capture` runs the two tests one at a time. The parallel run of Task 4 is the CI case.

## Observation: `paigasus-auth` (K5)

`ts/packages/paigasus-auth` never decodes the access token. It sets the session expiry to
`Date.now() + expires_in` (`src/core/single-flight.ts:325-331`, `src/http/routes.ts:380`). This run
shows that a refresh gives a new `expires_in` near `L`, so a refresh extends the session as
expected. No test of the package covers a refresh whose `expires_in` does not increase. The F7
floor raises a TTL below 31 s to 31 s. This is an observation only (spec D4); no change is made.
````

- [ ] **Step 3: Add the answer under SMA-703 Q3**

In `docs/superpowers/specs/2026-10-02-sma-703-zitadel-id-token-marker-claims-design.md`, keep the Q3 bullet (lines 356-358) as it is. Insert directly after it:

```markdown
  - Answer (SMA-732, 2026-10-05): MEASURED, Zitadel v4.15.3. A refresh gives an access token with
    `exp` = refresh time + lifetime, before and after the first `exp`. M4 showed equal `iat` and
    `exp` because its refresh came about 10 ms after the login. Limits: Login v1, a confidential
    web app, JWT access tokens, a lifetime of 10 s. The 12 h default, Login v2 and opaque tokens
    use the same code path, but that is code reading, not a measurement. See
    `2026-10-05-sma-732-zitadel-refresh-exp-measurements.md`.
```

Use the "about 10 ms" text only if the K4 row of the measurement doc says "consistent with K4". Else write the `Tq` result as the measurement doc states it.

Change the table row at line 395 from:

```markdown
| Refreshed access token keeps `iat`/`exp` | QUESTION | Q3. |
```

to:

```markdown
| Refreshed access token keeps `iat`/`exp` | QUESTION | Q3. Answered by SMA-732: a refresh extends `exp` (measured, see the Q3 answer). |
```

- [ ] **Step 4: Add the note under SMA-703 M4**

In `docs/superpowers/specs/2026-10-02-sma-703-zitadel-measurements.md`, keep line 519 (`Result: the refresh response returns …`) as it is. Insert after it, with one blank line before:

```markdown
SMA-732 (2026-10-05): the equal `iat`/`exp` above is not general. A refresh 7 s and 13 s after the login gives a new `exp` = refresh time + lifetime (`2026-10-05-sma-732-zitadel-refresh-exp-measurements.md`).
```

- [ ] **Step 5: Check the docs**

```bash
git diff --stat
grep -n '<' docs/superpowers/specs/2026-10-05-sma-732-zitadel-refresh-exp-measurements.md | grep -vE '<=|<\?|`' || echo "no angle-bracket instruction left"
grep -nE 'eyJ[A-Za-z0-9_-]{10,}' docs/superpowers/specs/2026-10-05-sma-732-zitadel-refresh-exp-measurements.md || echo "no JWT in the doc"
```

Expected: three files changed (one new). `no angle-bracket instruction left` and `no JWT in the doc`.

- [ ] **Step 6: Commit**

```bash
git add docs/superpowers/specs/2026-10-05-sma-732-zitadel-refresh-exp-measurements.md \
        docs/superpowers/specs/2026-10-02-sma-703-zitadel-id-token-marker-claims-design.md \
        docs/superpowers/specs/2026-10-02-sma-703-zitadel-measurements.md
git commit -m "docs(rs): record the SMA-732 Zitadel refresh exp measurement

The measurement doc holds the printed lines of two local runs, the
K1-K4 check and the M1-M3 mutation results. SMA-703 Q3 and M4 now refer
to it. The original text stays.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Final verification

**Files:** none changed. If a step reds, fix the cause in the task that owns the file and commit there.

- [ ] **Step 1: Format and lint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo fmt --check
cargo clippy -p paigasus-iam --locked --all-targets -- -D warnings
```

Expected: both exit 0.

- [ ] **Step 2: The parallel run (the CI case, spec § 6)**

```bash
set -o pipefail
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test zitadel_e2e 2>&1 | tee "${TMPDIR:-/tmp}/sma-732/parallel.log"
grep -E 'FLAKY|tests run' "${TMPDIR:-/tmp}/sma-732/parallel.log"
```

Expected: `2 tests run: 2 passed`, with no `FLAKY`. Here nextest runs the two tests at the same time, so two `start-from-init` runs share the host. If the new test fails here, read its panic (nextest prints a failing test's output) and apply the STOP rule. If it shows `FLAKY`, report it with the panic of the failed attempt; do not change `L` (D3).

- [ ] **Step 3: The Docker canary, unfiltered**

```bash
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test docker_preflight
```

Expected: `1 passed`. This proves that Docker was reachable, so no run above passed by a silent skip.

- [ ] **Step 4: Check the branch**

```bash
git status --short
git log --oneline origin/main..HEAD
git diff origin/main --stat
```

Expected: a clean tree. Three new commits after the spec and plan commits: the `start_zitadel` refactor, the test, and the measurement doc. The diff touches only `rs/crates/services/paigasus-iam/tests/zitadel_e2e.rs`, `rs/.config/nextest.toml`, the new measurement doc, the two SMA-703 docs, and the spec and plan files.

- [ ] **Step 5: Report**

Report to the coordinator: the outcome line, the four token lines of run 1, the M1-M3 results, the `FLAKY` count of every run, and the commit hashes.
