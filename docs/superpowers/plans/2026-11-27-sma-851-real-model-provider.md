# SMA-851 Real Model Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The agent host runs an `[[agents]]` entry on Anthropic Claude through the Paigasus AI
Gateway `POST /v1/messages`, and a fault of the gateway link never moves events to `dead`.

**Architecture:** Part U upgrades Helikon (and jiff 1, croner 5) in its own commit. Part C adds
`[gateway]` and `[[models]]` to `Config`. Part F adds the host-fault class: a new `RunEnd`
variant, a new `RunCommitter::host_fault` method that backs off and refunds the attempt in one
transaction, and a pool pause. Part P adds `adapters/helikon/gateway_model.rs`, which builds one
shared reqwest client and one Helikon `AnthropicModel` per `[[models]]` entry, wrapped in
`RetryingModel`. Part T tests it against an axum fake gateway, in process and with the real binary.

**Tech Stack:** Rust 2024, Helikon (`paigasus-helikon`, `paigasus-helikon-providers-anthropic`,
`runtime-tokio`), reqwest 0.13.5 (`rustls-no-provider`, ring), axum 0.8, sea-orm, Postgres
testcontainers, `metrics`, `tracing`.

**Spec:** `docs/superpowers/specs/2026-11-27-sma-851-real-model-provider-design.md` (approved by
Sven on 2026-11-27 with D1 (a), D2 (a), D3 (a)). Read §5, §7.2 to §7.7 and §9 before each task.

## Resolved versions (Task 0 fills this table)

| Name in this plan | Crate | Version |
|---|---|---|
| `<facade>` | `paigasus-helikon` | 0.10.0 |
| `<core>` | `paigasus-helikon-core` | 0.8.4 |
| `<provider>` | `paigasus-helikon-providers-anthropic` | 0.4.0 |
| `<runtime-tokio>` | `paigasus-helikon-runtime-tokio` | 0.3.3 |
| `<testkit>` | `paigasus-helikon-sessions-testkit` | 0.3.3 |

## Global Constraints

- Every new source file starts with `// SPDX-License-Identifier: Apache-2.0`.
- Edition 2024 and rust-version 1.95 (workspace values). Workspace lints are `warnings = deny`, so
  no task leaves dead code for a later task.
- Commits: `feat(rs): … (SMA-851)` (part U: `build(rs): …`). Body lines at most 100 characters.
  No body line that starts with `#` and a number. End each message with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Before each cargo or moon command in the Bash tool:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. Run cargo from `rs/`.
- Helikon comes from crates.io only: no `[patch]`, no path and no git dependency. Exact pins (`=`).
- `paigasus-helikon` keeps `default-features = false, features = ["runtime-tokio"]`.
- The provider dependency is
  `paigasus-helikon-providers-anthropic = { version = "=<provider>", default-features = false, features = ["rustls-no-provider"] }`.
- `rs/Cargo.lock` never holds `aws-lc-rs`, `aws-lc-sys`, `openssl-sys` or `native-tls`. No waiver
  and no `skip` in `rs/deny.toml` (A1).
- One rustls provider: `ring`. Call `let _ = rustls::crypto::ring::default_provider().install_default();`
  before each `reqwest::Client` build, in production code and in tests.
- The gateway key never appears in a log line, a `Debug` output, a serialized config, a header
  `Debug` or an error text (A4). Errors name the path, never the content.
- The gateway model sends no `cache_control` (`CacheStrategy::None`, the provider default) and
  calls `.native_tool_types(false)`.
- Config values (spec §7.4): `connect_timeout_ms` default 5000, range 100 to 60000;
  `read_timeout_ms` default 60000, range 1000 to 600000; `max_output_tokens` required, range 1 to
  128000; `worker.host_fault_pause_after` default 3; `worker.host_fault_pause_secs` default 30,
  doubling to a cap of 600.
- `provider = "anthropic"` is the only value in v1. `scripted` is a reserved model name.
- In-run retry: `RetryingModel` with 2 retries (3 attempts) that obey `retry_after_ms`.
- No database migration. `run.error_class` is free `text` with no CHECK (m0005), so
  `'host_fault'` needs no schema change.
- User-facing text (README, example config, CHANGELOG, log lines, error texts) is in ASD-STE100
  Simplified Technical English.

## Review Focus

1. **A long `retry-after` inside a run.** A 429 `billing_error` or budget refusal with
   `retry-after: 120` must not make `RetryingModel` sleep in the run until `run.timeout_secs`
   ends it as a deterministic `RunError::Timeout`. Expected: one request, a host fault, and
   `not_before` at least 120 s later. Tests: Task 6 `a_long_retry_after_leaves_the_run_at_once`,
   Task 7 `a_429_billing_error_is_a_host_fault_with_its_retry_after`.
2. **A redirect from the gateway address** (for example a misrouted ingress). With
   `redirect::Policy::none()` the provider sees the 3xx as an unknown status and maps it to
   `Other`, which is transient and reaches `dead`. Expected: no request to the `Location` host,
   and a host fault. Test: Task 7 `a_redirect_is_not_followed_and_is_a_host_fault`.
3. **A stall before the response head on every in-run attempt.** Three attempts of
   `read_timeout_ms` each must end before `run.timeout_secs`, or the run ends as a deterministic
   `Timeout`. Expected: the boot refuses `3 × read_timeout_ms >= run.timeout_secs × 1000`.
   Test: Task 2 `three_read_timeouts_must_fit_in_the_run_timeout`.
4. **A key file from an editor or a secret mount.** A trailing `\n` or `\r\n` must be accepted. A
   BOM, an inner space and a non-UTF-8 byte must be refused with a path-only error. Test: Task 6
   `the_key_file_rules_hold`.
5. **A slow stream that sends only `ping` frames for longer than `read_timeout_ms` in total.** The
   read timeout applies to each read, so pings keep the stream alive. Expected: the run completes.
   Test: Task 7 `pings_keep_a_slow_stream_alive`.

## File map

| File | Change | Task |
|---|---|---|
| `rs/Cargo.toml` | `jiff` 1, `croner` 5.0.1 | 1 |
| `rs/Cargo.lock` | lock update | 1, 6 |
| `rs/crates/services/paigasus-agent-host/Cargo.toml` | Helikon pins, provider dep, `paigasus-logging` test-support dev-dep | 1, 6 |
| `rs/crates/libs/paigasus-cloudevents/src/build.rs` | jiff 1 rename | 1 |
| `rs/crates/libs/paigasus-agent-host-core/src/timer/{cron.rs,dst_tests.rs}` | jiff 1, croner 5 | 1 |
| `rs/crates/libs/paigasus-agent-host-core/src/ports.rs` | `RunCommitter::host_fault` | 3 |
| `src/adapters/helikon/session.rs` | golden row test | 1 |
| `src/config.rs` | `GatewayConfig`, `ModelConfig`, pause keys, rules | 2, 5 |
| `src/adapters/persistence/run_commit/{mod.rs,host_fault.rs}` | the host-fault transaction | 3 |
| `src/application/lease_keeper.rs` | `LeaseKeeper::host_fault` | 3 |
| `src/application/worker/{ports.rs,run.rs,pool.rs,breaker.rs,test_fakes.rs,mod.rs}` | `RunEnd::HostFault`, worker end, pause | 4, 5 |
| `src/adapters/helikon/classify.rs` | the §7.5 table, `GatewayBackOff` | 4 |
| `src/adapters/helikon/{gateway_model.rs,mod.rs,model_registry.rs,runner.rs}` | part P | 6 |
| `src/main.rs` | wiring, `describe_*` texts | 4, 5, 6 |
| `rs/crates/libs/paigasus-observability/src/names.rs` | label doc, new gauge | 4, 5 |
| `tests/run_commit_pg.rs`, `tests/worker_crash_pg.rs` | host-fault tests, fake forward | 3 |
| `tests/support/{mod.rs,fake_gateway.rs}`, `tests/gateway_model_pg.rs` | in-process tests | 7 |
| `tests/gateway_e2e_pg.rs`, `moon.yml`, `ci/affected-graph/cargo_moon_parity.py` | E2E | 8 |
| `README.md`, `agent-host.toml.example`, `CHANGELOG.md`, `rs/CLAUDE.md` | docs | 9 |

Paths that start with `src/` or `tests/` are under `rs/crates/services/paigasus-agent-host/`.

---

### Task 0: Resolve the Helikon versions and check the new API (no code)

**Files:**
- Modify: `docs/superpowers/plans/2026-11-27-sma-851-real-model-provider.md` (the table above)

**Interfaces:**
- Produces: the five versions `<facade>`, `<core>`, `<provider>`, `<runtime-tokio>`, `<testkit>`.
  Every later task names them.

- [ ] **Step 1: Read the published versions**

Run (any directory): `for c in paigasus-helikon paigasus-helikon-core paigasus-helikon-providers-anthropic paigasus-helikon-runtime-tokio paigasus-helikon-sessions-testkit; do cargo info $c | grep -E '^(version|rust-version)'; done`
Expected: `paigasus-helikon-providers-anthropic` is a `0.4.x`, and every `rust-version` is at most
1.95. If the provider is still `0.3.x`, STOP: the SMA-868 release is not out. Report to the
coordinator.

- [ ] **Step 2: Fetch the sources into a scratch project outside the repository**

Make a throwaway cargo project in the session scratchpad (name it `helikon-probe`). Give it the
dependencies `paigasus-helikon = { version = "=<facade>", default-features = false, features = ["runtime-tokio"] }`,
`paigasus-helikon-providers-anthropic = { version = "=<provider>", default-features = false, features = ["rustls-no-provider"] }`
and `paigasus-helikon-sessions-testkit = "=<testkit>"`. Run `cargo fetch` and
`grep -E '^name = "aws-lc-rs"' Cargo.lock` in it.
Expected: `grep` prints nothing. No output means that `aws-lc-rs` is absent. Do not use
`cargo tree -i aws-lc-rs` for this check: it fails with "did not match any packages" when the
crate is absent. `cargo tree -d` shows one `paigasus-helikon-core` and one `jiff`.

- [ ] **Step 3: Check the SMA-868 API in the fetched sources**

Read `~/.cargo/registry/src/*/paigasus-helikon-providers-anthropic-<provider>/src/` and
`.../paigasus-helikon-core-<core>/src/model.rs`. Check each fact. Write "yes" or the difference
into a list for the coordinator:

1. The provider has the features `rustls-aws-lc` (default) and `rustls-no-provider`.
2. `BuildError` has `NoCryptoProvider` and `InvalidAuthCredential`.
3. `ModelError::Rejected { status: u16, error_type: String, message: String }` exists, and the
   provider maps HTTP 400, 401, 403, 404 and 413 to it.
4. A 400 `invalid_request_error` whose message contains `prompt is too long` still maps to
   `ContextLengthExceeded` (the `prompt_too_long.body.json` fixture). Task 7 depends on it.
5. A 429 `billing_error` maps to `RateLimited { retry_after_ms }` with the `retry-after` value.
6. A `refusal` stop reason still ends the run as `ModelError::Refused`.
7. `AuthSource`, `AuthHeader` and `AnthropicModel` `Debug` print `<redacted>` for the key, and the
   `x-api-key` header value is marked sensitive.
8. `KNOWN_MODELS` has rows for `claude-sonnet-5-5` and `claude-opus-5-5`.
9. A send error (`reqwest::Error` from `send()`) still maps to `ModelError::Transport`. Task 6
   depends on it for the redirect rule.
10. The facade `<facade>` and the provider `<provider>` resolve to the same `<core>`.

If fact 1, 2, 3, 4, 9 or 10 is false, STOP and report. A false fact changes a class in the §7.5
table, and the coordinator must decide it.

- [ ] **Step 4: Fill the table and commit**

Write the five versions into the "Resolved versions" table of this file.

```bash
git add docs/superpowers/plans/2026-11-27-sma-851-real-model-provider.md
git commit -m "docs(rs): record the Helikon versions for SMA-851"
```

---

### Task 1: Part U — upgrade Helikon, jiff 1 and croner 5

**Files:**
- Modify: `rs/Cargo.toml:114-118` (jiff, croner and their comments), `rs/Cargo.toml:109-111` (the
  tokio-util comment names Helikon 0.6.12)
- Modify: `rs/crates/services/paigasus-agent-host/Cargo.toml:45-48,107-110` (the pins and their
  comments)
- Modify: `rs/Cargo.lock`
- Modify (compile fixes, the known sites): `rs/crates/libs/paigasus-cloudevents/src/build.rs:167`,
  `rs/crates/libs/paigasus-agent-host-core/src/timer/dst_tests.rs:143`,
  `rs/crates/libs/paigasus-agent-host-core/src/timer/cron.rs`,
  `rs/crates/services/paigasus-agent-host/tests/scheduler_pg.rs` (six `to_zoned` calls),
  `src/adapters/helikon/classify.rs` (module doc), `tests/run_session_pg.rs` (testkit API, if it
  changed)
- Test: `src/adapters/helikon/session.rs` (new golden test)

**Interfaces:**
- Consumes: the Task 0 versions.
- Produces: the workspace on `<facade>`, `<core>`, jiff 1 and croner 5. No API of this repo changes.

- [ ] **Step 1: Measure the rollback before the change (scratch, not committed)**

In the scratchpad, make two throwaway cargo projects. `rows-new` depends on
`paigasus-helikon-core = "=<core>"` and `serde_json`. It builds one value of each `SessionEvent`
variant that a run writes (`user_message`, `assistant_message`, the tool call and the tool result
kinds, and every other variant of `<core>`), with a `ts` that has a fraction
(`jiff::Timestamp::from_nanosecond(1_764_000_000_123_456_789)` or the `<core>` equivalent). It
prints each one as a JSON line. `rows-old` depends on `paigasus-helikon-core = "=0.6.0"`. It reads
the lines and calls `serde_json::from_str::<SessionEvent>` on each. It also prints the JSON line of
one `UserMessage` with the same fractional `ts`, as core 0.6.0 writes it. Keep that line for Step 4.
Expected: a pass or fail per kind. Write the result as two or three short lines. They go into the
commit body in Step 9. If any kind fails, the lines say: "A rollback of part U needs a database
restore: core 0.6 cannot read <kinds>."

- [ ] **Step 2: Change the manifests**

`rs/Cargo.toml`: `jiff = { version = "1", features = ["tzdb-bundle-always"] }` and
`croner = { version = "5.0.1", default-features = false, features = ["std", "jiff"] }`. Update the
comments: croner 5 `default-features = false` means `no_std`, so `std` is required.
Agent host: `paigasus-helikon = { version = "=<facade>", default-features = false, features = ["runtime-tokio"] }`
and `paigasus-helikon-sessions-testkit = "=<testkit>"`. Update the comments that name 0.6.12 and
0.1.2.

- [ ] **Step 3: Update the lock and fix the compile errors**

Run: `cargo update -p paigasus-helikon -p paigasus-helikon-sessions-testkit -p jiff -p croner` then
`cargo build --workspace --all-targets --features paigasus-agent-host/test-model`.
Fix each error with the jiff 1.0.0 changelog
(`~/.cargo/registry/src/*/jiff-1.0.3/CHANGELOG.md`) and the croner `MIGRATION.md`: `Zoned` is
`ZonedDateTime`, `to_zoned` is `to_zoned_datetime`, `Timestamp - Timestamp` is a `SignedDuration`,
`Timestamp` to `SystemTime` is fallible. Do not change a behaviour to make a test pass. In
`classify.rs`, change the module doc to "Every variant that Helikon core `<core>` has is named",
and name each new `RunError`, `AgentError` and `ModelError` variant of `<core>` in its existing
class (`ModelError::Rejected` goes to the wildcard arm in this task; Task 4 classifies it).
Expected: the build passes with no warning.

- [ ] **Step 4: Write the failing golden test**

In `src/adapters/helikon/session.rs` tests:

```rust
/// A row that Helikon core 0.6.0 wrote (Task 1 Step 1). The new core must read it unchanged.
const CORE_0_6_ROW: &str = r#"<the JSON line from Step 1>"#;

#[test]
fn a_core_0_6_row_with_a_fractional_ts_reads_with_the_new_core() {
    let event: SessionEvent = serde_json::from_str(CORE_0_6_ROW).expect("the new core reads the 0.6 row");
    let SessionEvent::UserMessage { ts, .. } = &event else { panic!("{event:?}") };
    assert_eq!(ts.as_nanosecond(), 1_764_000_000_123_456_789);
    let back = to_session_row(&event).expect("row").payload;
    assert_eq!(back, serde_json::from_str::<serde_json::Value>(CORE_0_6_ROW).expect("json"), "the new core writes the same ts text");
}
```

- [ ] **Step 5: Run it**

Run: `cargo nextest run --locked -p paigasus-agent-host --features test-model a_core_0_6_row`
Expected: PASS. If the second assertion fails, the `ts` text format changed. STOP and report it to
the coordinator: it is a format change that the spec forbids.

- [ ] **Step 6: Run the format proofs**

Run: `cargo nextest run --locked -p paigasus-agent-host-core -p paigasus-cloudevents`
Expected: PASS, with `timer::cron`, `timer::dst_tests`, `format_fire_at` and the `format_time`
tests of `build.rs` in the list.
Run: `cargo nextest run --locked -p paigasus-agent-host --features test-model -E 'binary(scheduler_pg) | binary(timer_store_pg) | test(timer_event) | test(timer_rows)'`
Expected: PASS (Docker needed).

- [ ] **Step 7: Check the duplicates and the banned crates**

Run: `cargo tree --locked -d -e normal --workspace | grep -E '^(jiff|paigasus-helikon-core) '`
Expected: no output (one version of each).
Run: `grep -E '^name = "(aws-lc-rs|aws-lc-sys|openssl-sys|native-tls)"' Cargo.lock`
Expected: no output.

- [ ] **Step 8: Run every existing test**

Run: `cargo nextest run --locked --workspace --features paigasus-agent-host/test-model --no-tests=pass`
Expected: PASS. Then `cargo clippy --locked --workspace --all-targets -- -D warnings` and
`cargo fmt --check`. Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add rs/Cargo.toml rs/Cargo.lock rs/crates/services/paigasus-agent-host rs/crates/libs/paigasus-agent-host-core rs/crates/libs/paigasus-cloudevents
git commit -m "build(rs): upgrade Helikon to <facade> with jiff 1 and croner 5 (SMA-851)" -m "<the rollback result lines from Step 1>"
```

---

### Task 2: Part C — `[gateway]`, `[[models]]` and the pause keys

**Files:**
- Modify: `src/config.rs` (new types after `AgentConfig` near line 418; fields on `Config` near
  line 91 and `WorkerConfig` near line 375; `Defaults` near line 802; new `ConfigError` variants;
  rules in `validate`)
- Test: `src/config.rs` `mod tests`

**Interfaces:**
- Produces (all `pub` in `paigasus_agent_host::config`):
  - `Config.gateway: Option<GatewayConfig>` (`#[serde(default)]`), `Config.models: Vec<ModelConfig>` (`#[serde(default)]`).
  - `pub struct GatewayConfig { pub base_url: String, pub api_key_file: String, pub ca_file: Option<String>, pub connect_timeout_ms: u64, pub read_timeout_ms: u64 }` with serde defaults 5000 and 60000. Derives `Debug, Clone, PartialEq, Eq, Deserialize, Serialize`.
  - `pub struct ModelConfig { pub name: String, pub provider: ModelProvider, pub model: String, pub max_output_tokens: u32 }` (no default for `max_output_tokens`).
  - `pub enum ModelProvider { Anthropic }` with `#[serde(rename_all = "lowercase")]`.
  - `WorkerConfig.host_fault_pause_after: u32` (default 3, range 1 to 100) and `WorkerConfig.host_fault_pause_secs: u64` (default 30, range 1 to 600).
  - `pub const RESERVED_MODEL_NAME: &str = "scripted";` and `pub const IN_RUN_ATTEMPTS: u32 = 3;` (the first call and 2 retries; Task 6 uses it).
  - New `ConfigError` variants: `GatewayMissing`, `GatewayUnused`, `GatewayBaseUrlUnparsable`, `GatewayBaseUrlScheme`, `GatewayBaseUrlUserinfo`, `GatewayBaseUrlQuery`, `GatewayBaseUrlFragment`, `GatewayKeyPathBlank`, `GatewayCaPathBlank`, `GatewayReadTimeoutTooLong { read_timeout_ms: u64, run_timeout_secs: u64 }`, `ModelNameEmpty`, `ModelNameDuplicate(String)`, `ModelNameReserved`, `ModelIdEmpty(String)`, `GatewayAgentsOrgMismatch { first: String, other: String }`. The ranges use the existing `ValueOutOfRange { key, … }` with the keys `gateway.connect_timeout_ms`, `gateway.read_timeout_ms`, `models.max_output_tokens`, `worker.host_fault_pause_after`, `worker.host_fault_pause_secs`.

The rules (spec §7.4), all in `validate()`, with no I/O:

- `[[models]]` not empty and no `[gateway]`: `GatewayMissing`. `[gateway]` and no `[[models]]`: `GatewayUnused`.
- `base_url`: `reqwest::Url::parse` (else `GatewayBaseUrlUnparsable`). Scheme `https`, or `http`
  only when `is_loopback_host(base_url)` is true (else `GatewayBaseUrlScheme`). No user name or
  password, no query, no fragment. A path prefix is allowed.
- `api_key_file` and `ca_file`: not empty and equal to their `trim()` (as `MemoryS3CaPathBlank`).
- `IN_RUN_ATTEMPTS × read_timeout_ms + (IN_RUN_ATTEMPTS − 1) × 5000 < run.timeout_secs × 1000`
  (Review Focus 3; the 5000 ms is `IN_RUN_RETRY_AFTER_MAX` and the largest back-off delay; the
  spec rule `read_timeout_ms < run.timeout_secs × 1000` follows from it). Else
  `GatewayReadTimeoutTooLong`.
- `[[models]].name`: not empty, unique, not `scripted`. `model`: not empty after `trim()`.
- D3: every agent whose `model` is a `[[models]].name` has the same org (`AgentId::org()`). Else
  `GatewayAgentsOrgMismatch` with the two agent ids. Agents on other models do not count.
- The error texts name the key and never a value of `api_key_file` content (there is none in
  config) or of `base_url` userinfo.

- [ ] **Step 1: Write the failing tests**

Add a helper `gateway_cfg(toml: &str) -> Config` (as `s3_cfg`, line 1568) and these tests. Use
`agent_toml` with a `model` parameter (extend the helper, line 2067) and a new
`agent_prn_in(org: u128, n: u128)`.

```rust
const GATEWAY: &str = "[gateway]\nbase_url = \"https://gw.example/prefix\"\napi_key_file = \"/run/secrets/k\"\n";
const MODEL: &str = "[[models]]\nname = \"claude\"\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-5-5\"\nmax_output_tokens = 16000\n";

#[test] fn gateway_and_pause_defaults_match_the_spec()      // connect 5000, read 60000, ca None, pause_after 3, pause_secs 30
#[test] fn models_without_a_gateway_are_refused()          // MODEL only -> Err(GatewayMissing)
#[test] fn a_gateway_without_models_is_unused()            // GATEWAY only -> Err(GatewayUnused); GATEWAY+MODEL -> Ok(())
#[test] fn the_base_url_rules_hold()
#[test] fn blank_key_and_ca_paths_are_refused()            // "" and " /k" -> GatewayKeyPathBlank; ca_file "" and "/c " -> GatewayCaPathBlank
#[test] fn the_gateway_timeouts_have_ranges()              // connect 99, 60001; read 999, 600001 -> ValueOutOfRange{key, ..}; 100, 60000, 1000 pass
#[test] fn three_read_timeouts_must_fit_in_the_run_timeout()
#[test] fn model_names_are_unique_not_empty_and_not_scripted()
#[test] fn the_model_id_must_not_be_empty()                // model = " " -> ModelIdEmpty("claude")
#[test] fn max_output_tokens_is_required_and_in_range()    // missing -> load error text contains "max_output_tokens"; 0, 128001 -> ValueOutOfRange; 1, 128000 pass
#[test] fn an_unknown_provider_fails_the_load()            // provider = "openai" -> figment error
#[test] fn agents_on_gateway_models_share_one_org()
#[test] fn the_pause_keys_have_ranges()                    // pause_after 0, 101; pause_secs 0, 601 -> ValueOutOfRange
#[test] fn config_debug_and_serialize_hold_no_key()        // key file content "sk-gw-secret" in the jail; Debug and serde_json::to_string(&cfg) do not contain it
```

The table of `the_base_url_rules_hold` (input → result):

| `base_url` | result |
|---|---|
| `https://gw.example/prefix` | `Ok(())` |
| `http://127.0.0.1:8088` | `Ok(())` |
| `http://localhost:1/p` | `Ok(())` |
| `http://gw.example` | `GatewayBaseUrlScheme` |
| `http://127.0.0.1.evil.example` | `GatewayBaseUrlScheme` |
| `ftp://gw.example` | `GatewayBaseUrlScheme` |
| `https://u:p@gw.example` | `GatewayBaseUrlUserinfo` |
| `https://gw.example/?a=1` | `GatewayBaseUrlQuery` |
| `https://gw.example/#f` | `GatewayBaseUrlFragment` |
| `not a url` | `GatewayBaseUrlUnparsable` |

`three_read_timeouts_must_fit_in_the_run_timeout`: the rule refuses the value when
`IN_RUN_ATTEMPTS x read + (IN_RUN_ATTEMPTS - 1) x 5000 >= run x 1000`. With `run.timeout_secs = 60`,
`read_timeout_ms = 16667` gives
`Err(GatewayReadTimeoutTooLong { read_timeout_ms: 16667, run_timeout_secs: 60 })`, and `16666`
gives `Ok(())`. The default pair (60000, 300) gives `Ok(())`.

`model_names_are_unique_not_empty_and_not_scripted`: two entries `claude` give
`ModelNameDuplicate("claude")`. `name = ""` gives `ModelNameEmpty`. `name = "scripted"` gives
`ModelNameReserved`.

`agents_on_gateway_models_share_one_org`: two agents on `claude` with orgs 1 and 2 give
`GatewayAgentsOrgMismatch { first: <prn org 1>, other: <prn org 2> }`. Two agents on `claude` in
org 1 give `Ok(())`. An agent on `claude` in org 1 and an agent on `scripted` in org 2 give
`Ok(())`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run --locked -p paigasus-agent-host --lib config::tests`
Expected: FAIL to compile (`GatewayConfig` not defined).

- [ ] **Step 3: Implement the types, the defaults and the rules**

Put the gateway rules in `fn GatewayConfig::validate(&self, run_timeout_secs: u64) -> Result<(), ConfigError>`
and the model and org rules in `fn validate_models(&self) -> Result<(), ConfigError>` on `Config`.
`validate` calls them after `agent_definitions()`. Doc comments: `ca_file` "REPLACES the platform
roots. The file must hold every root that the gateway chain needs." `read_timeout_ms` "applies to
each read, also to the wait for the response head. It must be more than the longest silent gap of
the gateway stream: the tool-hold `ping` interval (SMA-750 D12, `tool_interception.keepalive_secs`,
default 15 s) and the wait for the first upstream byte (`first_byte_timeout_secs`, default 30 s)."
The 60000 default is from that rule.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run --locked -p paigasus-agent-host --lib config::tests`
Expected: PASS, the existing config tests too.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-agent-host/src/config.rs
git commit -m "feat(rs): add the [gateway] and [[models]] config of the agent host (SMA-851)"
```

---

### Task 3: Part F — the `host_fault` committer method

**Files:**
- Modify: `rs/crates/libs/paigasus-agent-host-core/src/ports.rs:246-255`
- Create: `src/adapters/persistence/run_commit/host_fault.rs`
- Modify: `src/adapters/persistence/run_commit/mod.rs` (`mod host_fault;`, the trait impl)
- Modify: `src/application/lease_keeper.rs:170-180` (new method; its test `Fake` near line 508)
- Modify: `src/application/worker/test_fakes.rs` (`FakeCommitter`), `tests/worker_crash_pg.rs:196-218`
- Test: `tests/run_commit_pg.rs`, `src/application/lease_keeper.rs` tests

**Interfaces:**
- Produces:
  - `RunCommitter::host_fault(&self, h: &mut LeaseHandle, b: BackOff) -> Result<(), CommitError>`.
    Doc: "Return the events of a run that a host fault ended. Fail the run as `host_fault`, delay
    the events and give back the attempt of each, in one transaction (SMA-851 D1)."
  - `LeaseKeeper::host_fault(&self, b: BackOff) -> Result<(), CommitError>`, with the same rules as
    `LeaseKeeper::back_off`.
  - `PgRunCommitter::host_fault_run(&self, h: &mut LeaseHandle, b: BackOff)` (`pub(super)`).
  - `FakeCommitter`: `CommitterCall::HostFault(BackOff)`, `fn host_faults(&self) -> Vec<BackOff>`,
    and `pub host_fault_result: Option<Result<(), CommitError>>` (default `Ok(())`).
  - Lost-lease operation label: `"host_fault"`.

The transaction, in `host_fault.rs`, modelled on `back_off.rs` and `shutdown.rs`:

```rust
/// `$1` = run, `$2` = token, `$3` = error. No row means that the lease is lost.
const RUN_HOST_FAULT_SQL: &str = "UPDATE run SET status = 'failed', finished_at = now(), error_class = 'host_fault', error = $3 \
     WHERE id = $1 AND status = 'running' AND lease_token = $2";

/// `$1` = event, `$2` = run. The attempt of the claim goes back, as `REFUND_SQL` does after a shutdown.
const HOST_FAULT_REFUND_SQL: &str = "UPDATE inbox_event SET attempts = attempts - 1 \
     WHERE id = $1 AND claimed_run_id = $2 AND status = 'pending' AND attempts > 0";
```

Order: `check_handle`, begin, `lock_lease`, `RUN_HOST_FAULT_SQL` (0 rows is `LeaseLost`),
`retry_events`, then `HOST_FAULT_REFUND_SQL` for each event of `b.retry`, then
`commit_or_poison`. Every error path goes through `abort`.

- [ ] **Step 1: Write the failing tests in `tests/run_commit_pg.rs`**

```rust
#[tokio::test]
async fn host_fault_fails_the_run_delays_the_events_and_refunds_the_attempt() {
    // back_off_of(&s, 30 s) with error "the gateway rejected the agent host credential (status 401)"
    // -> Ok(()); run: status 'failed', error_class 'host_fault', that error, finished_at set;
    // both events: status 'pending', attempts = 0, last_error = the error, not_before > now() + 25 s.
}
#[tokio::test]
async fn a_stale_host_fault_is_lease_lost_and_changes_no_row()   // expire, snapshot equal, lost counter operation "host_fault" = 1
#[tokio::test]
async fn a_failed_commit_of_host_fault_is_outcome_unknown()      // refuse_event_update_at_commit -> Err(OutcomeUnknown), handle poisoned
#[tokio::test]
async fn repeated_host_faults_never_reach_dead() {
    // Six cycles with ClaimConfig { max_attempts: 5, .. }: host_fault with delay ZERO,
    // handle.record_complete(), then claim_events. Each claim returns Some(claim) with the same events.
    // After the loop: 0 rows with status 'dead'; both events have attempts = 1 (claimed again).
}
```

In `lease_keeper.rs` tests, mirror the existing `back_off` test for a cancelled keeper:
`a_host_fault_on_a_cancelled_keeper_is_lease_lost` (expects `Err(CommitError::LeaseLost)` and no
committer call).

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run --locked -p paigasus-agent-host --features test-model -E 'binary(run_commit_pg) | test(host_fault)'`
Expected: FAIL to compile (`host_fault` not found).

- [ ] **Step 3: Implement the trait method, the Postgres transaction, the keeper method and the fakes**

`RecordingCommitter` in `worker_crash_pg.rs` and the keeper test `Fake` forward or record the call
as they do for `back_off`.

- [ ] **Step 4: Run the tests**

Run the Step 2 command. Expected: PASS. Then
`cargo nextest run --locked -p paigasus-agent-host --features test-model -E 'binary(worker_crash_pg)'`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-agent-host-core/src/ports.rs rs/crates/services/paigasus-agent-host
git commit -m "feat(rs): add the host-fault back-off that gives back the attempt (SMA-851)"
```

---

### Task 4: Part F — `RunEnd::HostFault`, the class table and the worker end

**Files:**
- Modify: `src/application/worker/ports.rs:51-58`
- Modify: `src/adapters/helikon/classify.rs` (the `model_error` arms, a new error type, the tests)
- Modify: `src/application/worker/run.rs` (the match near line 226, a new `host_fault` method
  next to `transient`, its tests)
- Modify: `src/main.rs` (`describe_counter!` texts of `AGENT_HOST_RUNS_TOTAL` and
  `AGENT_HOST_LEASE_LOST_TOTAL`)
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs:171` (the `class` list in the doc)

**Interfaces:**
- Consumes: `LeaseKeeper::host_fault` (Task 3), `FakeCommitter::host_faults` (Task 3).
- Produces:
  - `RunEnd::HostFault { error: String, retry_after: Option<Duration> }`.
  - In `classify.rs`:
    ```rust
    /// A rate limit or budget refusal whose `retry-after` is longer than an in-run retry may wait.
    /// The gateway adapter puts it into `ModelError::Other`, so that `RetryingModel` does not sleep
    /// for it inside the run (Review Focus 1).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
    #[error("the gateway refused the call for {retry_after_ms} ms (a rate limit or a budget)")]
    pub struct GatewayBackOff { pub retry_after_ms: u64 }
    ```
  - `pub const HOST_FAULT_DELAY_CAP: Duration = Duration::from_secs(86_400);` in `run.rs`.
  - Run counter label `class = "host_fault"`.

The class table (spec §7.5 with D1 (a), D2 (a)):

| `ModelError` | `RunEnd` | error text starts with |
|---|---|---|
| `Rejected { status: 401 \| 403, .. }` | `HostFault { retry_after: None }` | `the gateway rejected the agent host credential (status {status}, {error_type})` |
| `Rejected { status: 404, .. }` | `HostFault { retry_after: None }` | `the gateway route is off or missing (status 404, {error_type})` |
| `Rejected { status: 400 \| 413, .. }` | `Deterministic { close_thread: false }` | the `Display` text of the error |
| `Rejected` with any other status | `Transient { retry_after: None }` | the `Display` text |
| `RateLimited { retry_after_ms }` | `HostFault { retry_after: retry_after_ms }` | `the gateway rate limit or budget refused the call` |
| `Unavailable`, `Transport(_)` | `HostFault { retry_after: None }` | `the gateway link failed` |
| `Other(e)` with `e.downcast_ref::<GatewayBackOff>()` | `HostFault { retry_after: Some(ms) }` | `the gateway rate limit or budget refused the call` |
| `Other(_)` | `Transient` (as today) | |
| `Refused { .. }` | `Deterministic { close_thread: false }` (as today) | |
| `ContextLengthExceeded` | `Deterministic { close_thread: true }` (as today) | |

Each error text ends with `: {message}` when the error has one. It goes through `replace_nul`.

The worker end:
- `RunEnd::HostFault { error, retry_after } => return self.host_fault(keeper, claim, &passed, error, retry_after).await`.
- `async fn host_fault(&self, keeper, claim, targets, error, retry_after) -> Step`: build the
  `BackOff` with `self.host_fault_delays(targets, retry_after)`, call `keeper.host_fault`. On `Ok`:
  `count_run("host_fault")` and one line
  `tracing::error!(run = %claim.run.uuid(), %error, "a host fault ended the run; the attempt is given back")`,
  then `Step::Next`. On `Err`: the same warn and `Step::Drop` as `transient`.
- `fn host_fault_delays(&self, targets, retry_after) -> Vec<EventDelay>`: per event
  `self.settings.retry.delay(event.attempts, jitter, None).max(retry_after.unwrap_or_default()).min(HOST_FAULT_DELAY_CAP)`.
  The `retry_after` is not cut by `retry.max_delay_secs`: the spec says "delay at least
  `retry_after`".

- [ ] **Step 1: Write the failing tests**

`classify.rs`: change `label()` to give `"host_fault"` and `"host_fault+{ms}"`, and change the rows
of `every_named_variant_has_its_class`:

```rust
(Err(agent_model(ModelError::RateLimited { retry_after_ms: Some(5000) })), "host_fault+5000"),
(Err(agent_model(ModelError::RateLimited { retry_after_ms: None })), "host_fault"),
(Err(agent_model(ModelError::Unavailable)), "host_fault"),
(Err(agent_model(ModelError::Transport("x".into()))), "host_fault"),
(Err(agent_model(rejected(401))), "host_fault"),
(Err(agent_model(rejected(403))), "host_fault"),
(Err(agent_model(rejected(404))), "host_fault"),
(Err(agent_model(rejected(400))), "deterministic"),
(Err(agent_model(rejected(413))), "deterministic"),
(Err(agent_model(rejected(418))), "transient"),
(Err(agent_model(ModelError::Other(anyhow::Error::new(GatewayBackOff { retry_after_ms: 120_000 })))), "host_fault+120000"),
(Err(agent_model(ModelError::Other(anyhow::anyhow!("x")))), "transient"),
```

`rejected(status)` builds `ModelError::Rejected { status, error_type: "e".into(), message: "m".into() }`.
Add `a_credential_rejection_says_so`: the error text of `rejected(401)` starts with
`the gateway rejected the agent host credential (status 401, e)`.

`run.rs` tests:

```rust
#[tokio::test] async fn a_host_fault_backs_off_with_a_refund_and_counts_host_fault()
// runner ends HostFault { retry_after: Some(60 s) }; committer.host_faults().len() == 1 and
// back_offs() is empty; each delay >= 60 s; counter class "host_fault" == 1; LeaseEnd::Released.
#[tokio::test] async fn a_host_fault_delay_keeps_a_retry_after_above_the_retry_cap()
// retry_after Some(3600 s) with retry.max 1800 s -> each delay == 3600 s.
#[tokio::test] async fn a_failed_host_fault_drops_the_lease()
// host_fault_result Some(Err(CommitError::Store("x".into()))) -> LeaseEnd::Dropped.
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run --locked -p paigasus-agent-host --lib -E 'test(classify) | test(worker::run)'`
Expected: FAIL to compile (`RunEnd::HostFault` not found).

- [ ] **Step 3: Implement the variant, the table and the worker end**

Add the `RunEnd::HostFault` arm to every exhaustive match on `RunEnd` (the compiler lists them).
`runner.rs` `finish` needs no change: a host fault carries no session rows. Update the
`describe_counter!` texts: runs "… (completed, stopped, deterministic, transient, host_fault,
screened, discarded or shutdown)", lost leases "… back_off, host_fault, release_after_shutdown or
deadline)". Update the doc of `AGENT_HOST_RUNS_TOTAL` in `names.rs` with `host_fault`.

- [ ] **Step 4: Run the tests**

Run the Step 2 command, then `cargo nextest run --locked -p paigasus-agent-host --features test-model`
and `cargo nextest run --locked -p paigasus-observability`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-agent-host rs/crates/libs/paigasus-observability/src/names.rs
git commit -m "feat(rs): classify gateway link faults as host faults (SMA-851)"
```

---

### Task 5: Part F — the pool pause

**Files:**
- Create: `src/application/worker/breaker.rs`
- Modify: `src/application/worker/mod.rs` (`pub mod breaker;`)
- Modify: `src/application/worker/run.rs` (`WorkerSettings.host_fault`, a breaker in `RunWorker`,
  `pub fn pause_left`, the calls in `host_fault` and `commit`)
- Modify: `src/application/worker/pool.rs:122-145` (`WorkerTask::run`), its tests
- Modify: `src/application/worker/test_fakes.rs` (`FakeRunner::sequence`, `gauge`)
- Modify: `src/config.rs` (`WorkerConfig::host_fault_settings`), `src/main.rs:340-352` and
  `describe_agent_host_metrics`
- Modify: every `WorkerSettings { … }` literal: `run.rs:454`, `pool.rs:225`,
  `tests/worker_crash_pg.rs:248`, `tests/worker_pool_pg.rs:89`, `tests/worker_pg.rs:100`
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (new gauge, `ALL`, the worker
  families test)

**Interfaces:**
- Consumes: `WorkerConfig.host_fault_pause_after`, `.host_fault_pause_secs` (Task 2); the
  `host_fault` path of `RunWorker` (Task 4).
- Produces:
  ```rust
  pub const HOST_FAULT_PAUSE_CAP: Duration = Duration::from_secs(600);
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct HostFaultSettings { pub pause_after: u32, pub pause: Duration }
  pub struct HostFaultBreaker { /* std Mutex<State { in_row: u32, until: Option<Instant> }> */ }
  impl HostFaultBreaker {
      pub fn new(settings: HostFaultSettings) -> Self;
      /// Count one host fault. When the count reaches `pause_after`, start a pause and return it.
      pub fn record_host_fault(&self, now: Instant) -> Option<Duration>;
      /// A completed or stopped run: the count goes to 0 and a pause ends.
      pub fn record_completed(&self);
      /// The rest of the current pause, or `None`. It sets the gauge to 0 when a pause is over.
      pub fn pause_left(&self, now: Instant) -> Option<Duration>;
  }
  ```
  `Instant` is `tokio::time::Instant`, so the tests run on paused time.
  - `WorkerSettings.host_fault: HostFaultSettings`; `RunWorker::pause_left(&self) -> Option<Duration>`.
  - `WorkerConfig::host_fault_settings(&self) -> HostFaultSettings`.
  - `names::AGENT_HOST_WORKER_HOST_FAULT_PAUSED = "agent_host_worker_host_fault_paused"`: a gauge,
    1 while the pool pauses after host faults, else 0. No label.

The pause length: with `n = in_row - pause_after` (0 for the fault that reaches the count),
`pause = min(settings.pause × 2^n, HOST_FAULT_PAUSE_CAP)`, with saturating math. Each host fault
at or over the count starts a new pause from `now`. Only `record_completed` resets the count:
a transient or deterministic end does not change it. When a pause starts, the worker writes one
`tracing::warn!(pause_secs, in_row, "the worker pool pauses after host faults in a row")` and sets
the gauge to 1.

`WorkerTask::run`: at the top of each loop pass, before `acquire_next`,
`if let Some(left) = self.worker.pause_left() { if !self.pause(left).await { return; } continue; }`.
`RunWorker::commit` calls `record_completed` after an `Ok` commit of a `Completed` or `Stopped`
record. `RunWorker::host_fault` calls `record_host_fault(Instant::now())` after an `Ok` back-off.

- [ ] **Step 1: Write the failing tests**

`breaker.rs`:

```rust
#[test] fn three_host_faults_start_a_pause_of_the_base()
// after 3, base 30 s: faults 1 and 2 return None; fault 3 returns Some(30 s); pause_left(now) == Some(30 s).
#[test] fn each_further_host_fault_doubles_the_pause_to_the_cap()
// faults 3..=9 return 30, 60, 120, 240, 480, 600, 600 s.
#[test] fn a_completed_run_resets_the_count_and_ends_the_pause()
// 3 faults, record_completed -> pause_left None; 2 more faults -> None; the 3rd -> Some(30 s).
#[test] fn the_pause_ends_after_its_length()
// pause_left(now + 30 s) == None; the gauge (local DebuggingRecorder) is 1 after the start, 0 after.
```

`pool.rs`:

```rust
#[tokio::test(start_paused = true)]
async fn three_host_faults_pause_the_pool_and_a_completed_run_resets_it() {
    // concurrency 1, poll 1 s, pause_after 3, pause 30 s.
    // Runner: FakeRunner::sequence([HostFault x3, Completed]); leases with enough handles and claims.
    // At 10 s: runner.calls() == 3, gauge == 1.
    // At 29 s: runner.calls() == 3 (no claim during the pause).
    // At 40 s: runner.calls() == 4, gauge == 0.
}
```

`config.rs`: `host_fault_settings_come_from_the_worker_keys` (3 and 30 give
`HostFaultSettings { pause_after: 3, pause: 30 s }`).

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run --locked -p paigasus-agent-host --lib -E 'test(breaker) | test(pool) | test(host_fault_settings)'`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the breaker, the settings, the pool check, the gauge and the wiring**

`main.rs` sets `host_fault: config.worker.host_fault_settings()` and adds
`describe_gauge!(names::AGENT_HOST_WORKER_HOST_FAULT_PAUSED, "1 while the worker pool pauses after host faults in a row, else 0.")`.
The test literals of `WorkerSettings` use `HostFaultSettings { pause_after: 3, pause: Duration::from_secs(30) }`.

- [ ] **Step 4: Run the tests**

Run the Step 2 command, then `cargo nextest run --locked -p paigasus-agent-host --features test-model`
and `cargo nextest run --locked -p paigasus-observability`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-agent-host rs/crates/libs/paigasus-observability/src/names.rs
git commit -m "feat(rs): pause the worker pool after host faults in a row (SMA-851)"
```

---

### Task 6: Part P — the gateway model adapter, the boot probe and the wiring

**Files:**
- Modify: `rs/crates/services/paigasus-agent-host/Cargo.toml` (the provider dependency; dev-dep
  `paigasus-logging = { path = "../../libs/paigasus-logging", version = "0.0.0", features = ["test-support"] }`
  with the comment that the gateway and IAM use)
- Modify: `rs/Cargo.lock`
- Create: `src/adapters/helikon/gateway_model.rs`
- Modify: `src/adapters/helikon/mod.rs` (`pub mod gateway_model;`, `build_agent` signature and its
  two tests)
- Modify: `src/adapters/helikon/model_registry.rs` (the per-model output limit, the module doc)
- Modify: `src/adapters/helikon/runner.rs:289-298` (pass the limit)
- Modify: `src/main.rs` (`model_registry`, the probe spawn, the module doc boot order step 3)

**Interfaces:**
- Consumes: `GatewayConfig`, `ModelConfig`, `IN_RUN_ATTEMPTS` (Task 2); `GatewayBackOff` (Task 4).
- Produces:
  - `ModelRegistry::register_with_limit(&mut self, id: impl Into<String>, model: SharedModel, max_output_tokens: u32)`
    and `ModelRegistry::max_output_tokens(&self, id: &str) -> Option<u32>`. `register` keeps its
    signature and stores no limit.
  - `build_agent(def: &AgentDefinition, model: SharedModel, max_output_tokens: Option<u32>, ws: Arc<Mutex<MemoryWorkspace>>) -> LlmAgent<(), SharedModel, String>`.
    With `Some(n)`, it calls `.model_settings(s)` where `s = ModelSettings::default()` and
    `s.max_output_tokens = Some(n)` (`ModelSettings` is `#[non_exhaustive]`: set the field on a
    default value). The provider sends that value as `max_tokens`.
  - In `gateway_model.rs`:
    ```rust
    pub const IN_RUN_RETRY_AFTER_MAX: Duration = Duration::from_secs(5);
    pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
    pub const ANTHROPIC_VERSION: &str = "2023-06-01";
    /// The key. `Debug` and `Serialize` write `<redacted>`.
    pub struct GatewayKey(String);           // fn as_str(&self) -> &str
    #[derive(Debug, thiserror::Error)]
    pub enum GatewayModelError {             // every text names the path, never the content
        KeyRead { path: String }, KeyEmpty { path: String }, KeyNotUtf8 { path: String },
        KeyBom { path: String }, KeyInnerWhitespace { path: String }, KeyNotHeaderValue { path: String },
        CaRead { path: String }, CaParse { path: String }, CaEmpty { path: String },
        Client, Model { name: String, source: BuildError },
    }
    pub fn read_key(path: &str) -> Result<GatewayKey, GatewayModelError>;
    pub fn gateway_client(gateway: &GatewayConfig) -> Result<reqwest::Client, GatewayModelError>;
    /// Wraps the provider inside `RetryingModel`. It turns `RateLimited { retry_after_ms: Some(ms) }`
    /// with `ms > IN_RUN_RETRY_AFTER_MAX` into `ModelError::Other(GatewayBackOff { retry_after_ms: ms })`,
    /// from `invoke` and from each stream item. Every other result passes unchanged.
    pub struct RetryAfterCap<M>(pub M);      // impl Model
    pub fn register(models: &mut ModelRegistry, gateway: &GatewayConfig, entries: &[ModelConfig]) -> anyhow::Result<GatewayProbe>;
    #[derive(Debug)]
    pub struct GatewayProbe { /* client, base_url: String, key: GatewayKey */ }
    impl GatewayProbe { pub async fn run(self) -> ProbeOutcome; }
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ProbeOutcome { Passed(u16), CredentialRejected(u16), RouteMissing, Unexpected(u16), Unreachable }
    ```

The rules:

- `read_key`: read the bytes (`KeyRead`). Not UTF-8: `KeyNotUtf8`. Starts with `'\u{feff}'`:
  `KeyBom`. Then `trim()`. Empty: `KeyEmpty`. Any `char::is_whitespace` left: `KeyInnerWhitespace`.
  `reqwest::header::HeaderValue::from_str` refuses it: `KeyNotHeaderValue`.
- `gateway_client`: install `ring` first. Then
  `reqwest::Client::builder().redirect(redirect::Policy::custom(|a| a.error(RedirectRefused))).no_proxy().connect_timeout(..).read_timeout(..)`.
  With `ca_file`: read it (`CaRead`), `reqwest::Certificate::from_pem_bundle` (`CaParse`), empty
  (`CaEmpty`), then `.tls_certs_only(certs)` (REPLACE). `RedirectRefused` is a private error type
  with the text "the gateway answered with a redirect; the agent host follows no redirect". The
  custom policy makes `send()` fail, so the provider maps it to `Transport`, which is a host fault.
  `Policy::none()` would give the 3xx to the provider as an unknown status, which is `Other` and
  transient (Review Focus 2). A build error is `Client` (no inner text: it can hold a path).
- `register`: `read_key` once, `gateway_client` once. For each entry:
  `AnthropicModel::messages(&entry.model).base_url(&gateway.base_url).api_key(key.as_str()).http_client(client.clone()).native_tool_types(false).build()`
  (`Model { name, source }` on error). Then
  `RetryingModel::new(RetryAfterCap(model), RetryPolicy::new().max_attempts(IN_RUN_ATTEMPTS).base_delay(Duration::from_millis(500)).max_delay(Duration::from_secs(5)))`,
  then `SharedModel::new(..)`, then `models.register_with_limit(&entry.name, shared, entry.max_output_tokens)`.
  `RetryingModel` and its `RetryPolicy` come from `paigasus_helikon::runtime_tokio`; import the
  policy as `HelikonRetryPolicy`, because the core crate has a `RetryPolicy` too.
- `GatewayProbe::run`: `GET {base_url without a trailing '/'}/v1/models` with `x-api-key` (a
  `HeaderValue` with `set_sensitive(true)`) and `anthropic-version: 2023-06-01`, per-request
  timeout `PROBE_TIMEOUT`. Log lines (one per outcome, never the key):
  - 2xx: `info!(status, "the gateway probe passed")` → `Passed`.
  - 401, 403: `warn!(status, "the gateway probe failed: the gateway rejected the agent host credential")` → `CredentialRejected`.
  - 404: `warn!("the gateway probe failed: the route is off or missing; set [messages] enabled = true on the gateway")` → `RouteMissing`.
  - another status: `warn!(status, "the gateway probe failed: unexpected status")` → `Unexpected`.
  - a send error: `warn!(error = %e, "the gateway probe failed: the gateway is not reachable")` → `Unreachable`.
- `main.rs`: `fn model_registry(config: &Config) -> anyhow::Result<(ModelRegistry, Option<GatewayProbe>)>`
  calls `gateway_model::register` when `config.gateway` is `Some`, before the database step, also
  when `worker.enabled` is false. The `test-model` registration stays. `serve` spawns the probe
  with a detached `tokio::spawn(async move { let _ = probe.run().await; })`. Do not put it in the
  `servers` set: an ended task there stops the process. The boot does not fail on the probe.

- [ ] **Step 1: Write the failing unit tests in `gateway_model.rs`**

```rust
#[test] fn the_key_file_rules_hold()
// Files in a tempdir -> result:
//   "sk-gw-1\n" and "sk-gw-1\r\n" -> Ok(key) with as_str() == "sk-gw-1"
//   "" and "  \n" -> KeyEmpty; "\u{feff}sk-gw-1" -> KeyBom; "sk gw" -> KeyInnerWhitespace;
//   [0xff, 0xfe] -> KeyNotUtf8; "sk-\u{7f}" -> KeyNotHeaderValue; a missing file -> KeyRead.
// For every error: Display and Debug contain the path and do not contain the file content.
#[test] fn the_key_debug_and_serialize_are_redacted()       // format!("{key:?}") and serde_json::to_string(&key) give "<redacted>" forms, no "sk-gw-1"
#[test] fn the_ca_file_rules_hold()                         // rcgen root in a PEM -> Ok; missing -> CaRead; "x" -> CaParse or CaEmpty; empty file -> CaEmpty
#[test] fn register_registers_each_entry_with_its_limit()  // two entries -> model("a"), model("b") are Some; max_output_tokens("a") == Some(16000)
#[test] fn no_built_value_or_boot_error_shows_the_key()
// format!("{probe:?}"), the Debug of the AnthropicModelBuilder before build(), of the built
// AnthropicModel (build it as register does) and of a Model { .. } error do not contain the key.
#[tokio::test] async fn a_long_retry_after_leaves_the_run_at_once()
// inner model returns RateLimited { retry_after_ms: Some(120_000) } as the first stream item:
// RetryAfterCap maps it to Other(GatewayBackOff { retry_after_ms: 120_000 }) (downcast check);
// Some(1_000) and None pass as RateLimited unchanged.
#[tokio::test] async fn the_probe_sends_the_key_and_the_version_and_logs_the_outcome()
// axum fake on 127.0.0.1:0: GET /v1/models records headers and answers 401.
// let (logs, _guard) = paigasus_logging::test_support::capture_logs();
// outcome == CredentialRejected(401); the recorded x-api-key == the key, anthropic-version == "2023-06-01";
// logs contain "the gateway rejected the agent host credential" and do not contain the key.
// Second fake answers 200 -> Passed(200), no "probe failed" line. A closed port -> Unreachable.
```

`model_registry.rs`: `a_limit_is_stored_per_model` (`register_with_limit("m", …, 512)` gives
`max_output_tokens("m") == Some(512)`; `register` gives `None`).
`mod.rs`: `build_agent_sets_the_output_limit` (`Some(512)` gives
`agent.model_settings.max_output_tokens == Some(512)`, or the equivalent public accessor of
`<core>`; `None` keeps `None`).

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run --locked -p paigasus-agent-host --lib -E 'test(gateway_model) | test(model_registry) | test(build_agent)'`
Expected: FAIL to compile.

- [ ] **Step 3: Add the dependency and implement**

Add the provider line (Global Constraints) under the `paigasus-helikon` line, with the comment
"`adapters::helikon::gateway_model` — the Anthropic Messages provider (SMA-851). No default
features: the default `rustls-aws-lc` links aws-lc-rs, which `rs/deny.toml` bans. The workspace
reqwest line gives `rustls-no-provider`; the client constructor installs `ring`." Run
`cargo update -p paigasus-helikon-providers-anthropic`. Implement the module, the registry change,
`build_agent`, `runner.rs` (`self.models.max_output_tokens(&spec.agent.model_id)`) and `main.rs`.

- [ ] **Step 4: Run the tests and check the graph**

Run the Step 2 command. Expected: PASS.
Run: `grep -E '^name = "(aws-lc-rs|aws-lc-sys|openssl-sys|native-tls)"' Cargo.lock`. Expected: no output.
Run: `moon run repo:deny`. Expected: PASS.
Run: `cargo nextest run --locked -p paigasus-agent-host --features test-model`. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/Cargo.lock rs/crates/services/paigasus-agent-host
git commit -m "feat(rs): add the gateway model adapter of the agent host (SMA-851)"
```

---

### Task 7: Part T — in-process tests against a fake gateway

**Files:**
- Create: `tests/support/fake_gateway.rs`
- Modify: `tests/support/mod.rs` (`pub mod fake_gateway;`)
- Create: `tests/gateway_model_pg.rs`

**Interfaces:**
- Consumes: `gateway_model::register` (Task 6), the worker (Tasks 3 to 5), the support helpers
  `migrated_postgres`, `insert_inbox`, `insert_pending_event`, `committer`, `scalar_i64`, `scalar_bool`.
- Produces (in `tests/support/fake_gateway.rs`, every item `#[allow(dead_code, reason = …)]`):
  ```rust
  pub enum Reply {
      Sse(String),
      Error { status: u16, body: String, retry_after: Option<u64> },
      /// Send the first frame of the SSE text, then keep the connection open with no more bytes.
      Stall(String),
      /// Send `count` ping frames, `gap` apart, after `message_start`, then the rest.
      Pings { sse: String, gap: Duration, count: usize },
      Redirect { location: String },
  }
  pub struct Recorded { pub headers: axum::http::HeaderMap, pub body: serde_json::Value }
  pub struct FakeGateway { pub addr: SocketAddr, /* … */ }
  impl FakeGateway {
      /// `POST /v1/messages` answers the replies in order and repeats the last one.
      /// `GET /v1/models` records its headers and answers 200 with `models.body.json`.
      pub async fn start(replies: Vec<Reply>) -> FakeGateway;
      pub fn requests(&self) -> Vec<Recorded>;
      pub fn models_requests(&self) -> Vec<axum::http::HeaderMap>;
  }
  /// A listener that counts the TCP connections it accepts. It answers nothing.
  pub async fn counting_listener() -> (SocketAddr, Arc<AtomicUsize>);
  pub const TEXT_STREAM: &str;   // include_str! of text_stream.sse
  pub const PARALLEL_TOOL_USE: &str;
  pub const PROMPT_TOO_LONG: &str;
  pub fn error_body(error_type: &str, message: &str) -> String;     // the SMA-750 D7 envelope
  pub fn text_sse(text: &str) -> String;                            // TEXT_STREAM with its four text deltas replaced by one
  pub fn with_stop_reason(sse: &str, stop_reason: &str) -> String;  // the message_delta stop_reason replaced
  pub fn tool_use_sse(id: &str, name: &str, input: &serde_json::Value) -> String;
  ```
  The fixtures come from `rs/crates/services/paigasus-gateway/tests/fixtures/anthropic/` with
  `include_str!("../../../paigasus-gateway/tests/fixtures/anthropic/<file>")` (SMA-750 D21: one
  provenance rule). `tool_use_sse` keeps `message_start`, the frames of block 0 and the closing
  frames of `parallel_tool_use.sse`, sets the block id and the tool name, and replaces the
  `input_json_delta` frames with one frame that holds `input`. The message_delta keeps
  `stop_reason: "tool_use"`. The SSE helpers parse and rewrite `data:` JSON with `serde_json`; they
  do not do text search on JSON.

The rig in `gateway_model_pg.rs`: one inbox with one pending event (as `worker_pg.rs`), a key file
`sk-gw-test-0123456789` in a temp dir, a `GatewayConfig` on `http://{fake.addr}` with
`read_timeout_ms` 1000 (unless a test says other), one `ModelConfig { name: "claude", provider:
Anthropic, model: "claude-sonnet-5-5", max_output_tokens: 16000 }`, `register` into a
`ModelRegistry`, and a `RunWorker` as `worker_pg.rs::worker` builds it, with `model_id: "claude"`,
`tools: ["memory"]`, `run` timeout 60 s, `retry` 30 s / 1800 s, `host_fault` 3 / 30 s. A helper
`serve_once` takes the lease and serves it once. A helper `outcome(db, f)` reads the run row
(`status`, `error_class`, `final_output`, `input_tokens`, `output_tokens`, `thread_id`) and the
event row (`status`, `attempts`, seconds from `now()` to `not_before`).

- [ ] **Step 1: Write the request-shape, tool-round, block and ping tests**

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_request_has_the_key_the_version_the_model_and_the_limit()
// Reply::Sse(TEXT_STREAM). First request: x-api-key == the key file content; anthropic-version
// present; no authorization header; body.model == "claude-sonnet-5-5"; body.max_tokens == 16000;
// the body text has no "cache_control"; body.tools[0].name == "memory" and has an "input_schema" object.
// Run: completed, final_output "Hello, it's nice to meet you!", input_tokens 19, output_tokens 15.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_round_sends_the_tool_result_back()
// Replies: tool_use_sse("toolu_01", "memory", {"command":"create","path":"/memories/a.md","file_text":"x"}),
// then Sse(TEXT_STREAM). 2 requests; request 2 has a user content block of type "tool_result"
// with tool_use_id "toolu_01"; run completed; memory_file has path '/a.md' in the scope.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gateway_tool_block_notice_completes_the_run()
// Reply::Sse(text_sse("[paigasus] tool call \"memory\" was blocked (policy).")) (stop end_turn).
// Run completed with that text as final_output; event done.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pings_keep_a_slow_stream_alive()
// read_timeout_ms 1000; Reply::Pings { sse: TEXT_STREAM, gap: 600 ms, count: 4 } -> completed.
```

- [ ] **Step 2: Write one class test per §7.5 row**

One `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` per row. Expected values:

| Test | Replies | run `status` / `error_class` | event `status` / `attempts` | requests | extra |
|---|---|---|---|---|---|
| `a_401_is_a_host_fault` | `Error 401 authentication_error` | `failed` / `host_fault` | `pending` / 0 | 1 | `run.error` starts with "the gateway rejected the agent host credential" |
| `a_403_is_a_host_fault` | `Error 403 permission_error` | `failed` / `host_fault` | `pending` / 0 | 1 | same |
| `a_404_is_a_host_fault` | `Error 404 not_found_error` | `failed` / `host_fault` | `pending` / 0 | 1 | |
| `a_400_is_deterministic` | `Error 400 invalid_request_error "bad request"` | `failed` / `deterministic` | `pending` / 1 | 1 | |
| `a_413_is_deterministic` | `Error 413 request_too_large` | `failed` / `deterministic` | `pending` / 1 | 1 | |
| `a_long_prompt_is_deterministic_and_closes_the_thread` | `Error 400` body `PROMPT_TOO_LONG` | `failed` / `deterministic` | `pending` / 1 | 1 | `thread.closed` is true |
| `a_429_billing_error_is_a_host_fault_with_its_retry_after` | `Error 429 billing_error`, `retry_after: Some(120)` | `failed` / `host_fault` | `pending` / 0 | 1 | `not_before` at least 115 s from now |
| `a_529_is_absorbed_by_the_in_run_retry` | `Error 529 overloaded_error`, then `Sse(TEXT_STREAM)` | `completed` / NULL | `done` / 1 | 2 | |
| `a_503_on_every_attempt_is_a_host_fault` | `Error 503 api_error` | `failed` / `host_fault` | `pending` / 0 | 3 | |
| `a_stalled_stream_is_a_host_fault` | `Stall(TEXT_STREAM)` | `failed` / `host_fault` | `pending` / 0 | 1 | the run ends in less than 10 s |
| `a_refusal_is_deterministic` | `Sse(with_stop_reason(TEXT_STREAM, "refusal"))` | `failed` / `deterministic` | `pending` / 1 | 1 | |
| `an_unknown_status_is_transient` | `Error 418 api_error` | `failed` / `transient` | `pending` / 1 | 1 | the `Other` row |

The stall reply gives 1 request: the first frame arrived, so `RetryingModel` does not retry.

- [ ] **Step 3: Write the redirect and key-safety tests**

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redirect_is_not_followed_and_is_a_host_fault()
// (second, accepted) = counting_listener(); Reply::Redirect { location: format!("http://{second}/v1/messages") }.
// accepted == 0; run failed / host_fault; attempts 0; fake requests == 3 (Transport is retried in the run).
#[tokio::test]   // current_thread: capture_logs sees only this thread
async fn the_key_is_not_in_any_trace_line_of_a_401_run()
// let (logs, _guard) = paigasus_logging::test_support::capture_logs();
// Reply Error 401; serve_once; positive control: logs contain "a host fault ended the run";
// the logs do not contain "sk-gw-test-0123456789".
```

- [ ] **Step 4: Run them**

Run: `cargo nextest run --locked -p paigasus-agent-host --features test-model -E 'binary(gateway_model_pg)'`
Expected: PASS (Docker needed). If a test fails, fix the code of Tasks 3 to 6, not the expected
value. A changed expected value needs the coordinator.

- [ ] **Step 5: Prove that the class tests bite**

Change the `Rejected { status: 401 | 403 }` arm of `classify.rs` to `deterministic(text, false)`.
Run the Step 4 command. Expected: `a_401_is_a_host_fault` and `a_403_is_a_host_fault` FAIL.
Change `.redirect(Policy::custom(..))` to `.redirect(Policy::none())`. Expected:
`a_redirect_is_not_followed_and_is_a_host_fault` FAILS. Remove the `RetryAfterCap` wrap in
`register`. Expected: `a_429_billing_error_is_a_host_fault_with_its_retry_after` FAILS (it waits in
the run). Restore each change by an exact reverse edit, not by `git checkout`, and run Step 4 again.
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-agent-host/tests
git commit -m "test(rs): test the gateway model classes against a fake gateway (SMA-851)"
```

---

### Task 8: Part T — the spawned-binary end-to-end test and the Moon inputs

**Files:**
- Create: `tests/gateway_e2e_pg.rs`
- Modify: `rs/crates/services/paigasus-agent-host/moon.yml` (`test.inputs`)
- Modify: `ci/affected-graph/cargo_moon_parity.py:611-622` (`ALLOW_OVER_APPROXIMATION`), only if
  `repo:affected-smoke` reds on the new input

**Interfaces:**
- Consumes: `support::spawn_service_in`, `support::wait_for_status`, `support::start_raw_postgres`,
  `support::fake_gateway` (Task 7), `insert_inbox`, `insert_pending_event`.

- [ ] **Step 1: Write the test**

```rust
//! The real binary on a fake gateway (spec G5, §7.6). It fails when the `register()` call in
//! `main.rs` is deleted: the boot then refuses the agent, because its model is not registered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_real_binary_runs_an_agent_through_the_gateway() {
    // Postgres; FakeGateway::start(vec![Reply::Sse(TEXT_STREAM)]); key file in a temp dir.
    // agent-host.toml in the temp dir:
    //   [gateway] base_url = "http://{fake.addr}", api_key_file = "{dir}/gateway-key", read_timeout_ms = 5000
    //   [[models]] name = "claude", provider = "anthropic", model = "claude-sonnet-5-5", max_output_tokens = 16000
    //   [[agents]] id = <prn>, name = "e2e", model = "claude", instructions = "Answer.", tools = []
    //   [worker] concurrency = 1, poll_interval_ms = 200
    // spawn_service_in; wait_for_status(/readyz, 200, 90 s); insert_inbox; insert_pending_event.
    // Wait (30 s budget, 50 ms poll) until the event is done.
    // Run row: completed, final_output "Hello, it's nice to meet you!", input_tokens 19, output_tokens 15.
    // fake.requests()[0]: x-api-key == key, body.max_tokens == 16000.
    // fake.models_requests() has one entry with anthropic-version (the boot probe).
    // child.tail() does not contain the key.
}
```

- [ ] **Step 2: Run it**

Run: `cargo nextest run --locked -p paigasus-agent-host --features test-model -E 'binary(gateway_e2e_pg)'`
Expected: PASS.

- [ ] **Step 3: Prove that it bites**

Delete the `gateway_model::register` call in `model_registry` (`main.rs`). Run Step 2.
Expected: FAIL with "the service exited … before /readyz answered 200" and an "is not registered"
log line. Restore the line by an exact reverse edit. Run Step 2. Expected: PASS.

- [ ] **Step 4: Add the fixture input to the Moon test task**

In `moon.yml` `tasks.test.inputs`, add
`- '/rs/crates/services/paigasus-gateway/tests/fixtures/anthropic/**/*'` with the comment
"SMA-851: the gateway model tests read the SMA-750 Anthropic fixtures with `include_str!`."
Run `/bin/bash ci/affected-graph/run.sh` (system bash 3.2; see the root CLAUDE.md). If A6 reds on
`("paigasus-agent-host-rs", "rs/crates/services/paigasus-gateway")`, add that key to
`ALLOW_OVER_APPROXIMATION` with the reason "the gateway model tests read the SMA-750 Anthropic
fixtures with include_str!", and run the suite again. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-agent-host ci/affected-graph/cargo_moon_parity.py
git commit -m "test(rs): run the real agent host binary on a fake gateway (SMA-851)"
```

---

### Task 9: Docs

**Files:**
- Modify: `rs/crates/services/paigasus-agent-host/README.md` (a new `## Model gateway` section
  after `## Configuration`, and rows in the configuration table)
- Modify: `rs/crates/services/paigasus-agent-host/agent-host.toml.example` (`[gateway]`,
  `[[models]]`, the two worker keys, `model = "claude"` in the `[[agents]]` example)
- Modify: `rs/crates/services/paigasus-agent-host/CHANGELOG.md` (`## [Unreleased]`)
- Modify: `rs/CLAUDE.md:101-107` (the CA-knob rule)

- [ ] **Step 1: Write the README section**

The section says, in STE:
- The agent host calls Claude only through the gateway `POST /v1/messages`. It never calls
  `api.anthropic.com`.
- The `[gateway]` and `[[models]]` keys, their defaults and ranges (the Global Constraints values).
- `ca_file` REPLACES the platform roots, as `nats.ca_file` does. The file must hold every root of
  the gateway chain.
- The client uses no proxy: `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY` do not apply to
  the gateway link. The client follows no redirect.
- The key file: one line, read once at boot. A trailing newline is removed. A BOM or an inner space
  stops the boot. Rotation (N6): issue the new key, restart every host, then revoke the old key.
- Gateway prerequisites: `[messages] enabled = true`, an `[upstream.anthropic]` section, streaming
  on, and a service-account API key with the `gateway_user` role on the target scope.
- The org rule (D3): every agent on a gateway model has the same org, and the key must have that
  org's scope or a project in it.
- The host-fault class: which errors are host faults (the §7.5 table), that a host fault gives the
  attempt back and never moves an event to `dead`, the pool pause and its keys, the
  `agent_host_runs_total{class="host_fault"}` label, the `agent_host_worker_host_fault_paused`
  gauge, and the boot probe (a `warn` line only).

- [ ] **Step 2: Update the example config, the CHANGELOG and `rs/CLAUDE.md`**

`rs/CLAUDE.md`: "six" becomes "seven", and the REPLACE list gets
"the agent host's `gateway.ca_file` (SMA-851)".

- [ ] **Step 3: Check that the example still parses**

Run: `cargo nextest run --locked -p paigasus-agent-host --lib config::tests`. Expected: PASS.
Read the diff once for STE: short sentences, active voice, no idiom.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-agent-host/README.md rs/crates/services/paigasus-agent-host/agent-host.toml.example rs/crates/services/paigasus-agent-host/CHANGELOG.md rs/CLAUDE.md
git commit -m "docs(rs): document the gateway model and the host-fault class (SMA-851)"
```

---

### Task 10: Coordinator only — amend ADR-0028 in Notion (no code)

The coordinator does this task. An implementer subagent skips it. The ADR status does not change.

The amendment says:

- **Title:** "Amendment (SMA-851, D1 (a)): a host fault keeps its attempt."
- **Rule.** A run that a fault of the shared gateway link ends is a host fault: `Rejected` 401, 403
  or 404; `RateLimited` (rate limit or budget) after the in-run retries; `Unavailable` or
  `Transport` (connect failure, read timeout, refused redirect) after the in-run retries; a
  `retry-after` longer than 5 s. The worker fails the run with `error_class = 'host_fault'`, backs
  off the events with the delay `max(retry delay, retry_after)` (cap 24 h), and gives back the
  attempt in the same transaction, as `release_after_shutdown` does. No memory commit. A host
  fault never moves an event toward `dead`. This changes the SMA-790 rule "every failed attempt
  counts".
- **Pool pause.** After `worker.host_fault_pause_after` host faults in a row (default 3), every
  worker task waits `worker.host_fault_pause_secs` (default 30 s, doubling to 600 s) before its
  next claim. One completed or stopped run resets the count.
- **Unchanged.** `Rejected` 400 and 413 and the model `refusal` are deterministic.
  `ContextLengthExceeded` is deterministic and closes the thread. `Other` is transient.
- **Risk.** A fault that the host misclassifies as a host fault retries without end. The
  `agent_host_runs_total{class="host_fault"}` counter, the `agent_host_worker_host_fault_paused`
  gauge and one `error` line per host fault make it visible.

---

### Task 11: Final verification

**Files:** none.

- [ ] **Step 1: Run the full gate graph**

Make a shim directory that holds only `bash` → `/bin/bash` (system bash 3.2), and put it first on
`PATH` (see memory "affected-smoke hang: shim bash, don't prepend /bin"). Then run from the repo
root:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :moon-diagnosis-exec :wasm-lockstep :test-e2e \
  --base origin/main --include-relations
```

Expected: PASS, except the gates of Step 2 when they ran under bash 3.2. On a failure, follow the
"Diagnosing an unattributed `moon ci` failure" steps of the root CLAUDE.md (Step 0 first).

- [ ] **Step 2: Re-run the bash-version gates directly**

Read the result of these runs, not the `moon ci` verdict for them:

- `/opt/homebrew/bin/bash ci/ruff/run.sh`, `/opt/homebrew/bin/bash ci/next-public/run.sh`,
  `/opt/homebrew/bin/bash ci/publish-metadata/run.sh`: each PASS.
- `/opt/homebrew/bin/bash ops/nats/check-subjects.sh`, then
  `( cd rs && cargo nextest run --locked --no-tests=pass -p paigasus-iam --test nats_permissions --test docker_preflight --profile iam-nats )`:
  each PASS.
- `/opt/homebrew/bin/bash ci/actionlint/run.sh`: read its pipe preflight line first. With
  `pipe capacity 65536 bytes`, expect PASS. With a `small` message (rc 2), there is no local
  verdict: write that down for the coordinator, and rely on CI.
- `repo:affected-smoke` keeps its bash 3.2 verdict from Step 1.

- [ ] **Step 3: Check the dependency graph**

Run: `moon run repo:deny`. Expected: PASS.
Run: `grep -E '^name = "(aws-lc-rs|aws-lc-sys|openssl-sys|native-tls)"' rs/Cargo.lock`. Expected:
no output.
Run (in `rs/`): `cargo tree --locked -d -e normal --workspace | grep -E '^(jiff|paigasus-helikon-core) '`.
Expected: no output.

- [ ] **Step 4: Report to the coordinator**

Report: the five versions, the rollback result of Task 1, the Task 0 fact list, the actionlint
preflight state, and any gate with no local verdict.
