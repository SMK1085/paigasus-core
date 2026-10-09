# SMA-700: IAM and gateway DPoP proof check — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** With DPoP on in IAM and in the gateway, a client that holds a DPoP-bound access token and its private key can call the gateway's protected routes with `Authorization: DPoP <token>` and a `DPoP` proof. IAM checks the proof (RFC 9449) before any identity lookup, refuses a replay, and accepts the one `IsAuthorized` follow-up of the gateway. With DPoP off, nothing changes.

**Architecture:** Hexagonal. `paigasus-iam-core` gets the types (`TokenScheme`, `Jkt`, `ProofDefect`, two `AuthnError` variants) and two ports (`DpopProofChecker`, `ReplayStore`). IAM gets one JOSE adapter (`adapters/oidc/dpop.rs`, checks 1-9), one in-memory store adapter (`adapters/dpop_replay.rs`), and one application service (`application/dpop.rs`, checks 10-13 and the follow-up ticket). `AuthenticateToken::resolve_dpop` runs the proof check before the identity lookup (D18). The gRPC `Introspect` reads a new `dpop` field, and `AuthEnforce` accepts the `DPoP` scheme only for the `IsAuthorized` follow-up. The gateway parses the `DPoP` scheme, forwards the context, appends a `WWW-Authenticate: DPoP` challenge, and maps the two new IAM reasons.

**Tech Stack:** Rust 2024 (tonic 0.14.6, tonic-types, axum, jsonwebtoken 11.1.0 with `rust_crypto`, `sha2` 0.11, `blake3`, `url` 2, figment), protobuf with buf, TypeScript (vitest, tsc) for the SDK presentation table, Helm (chart scripts in bash and python3).

**Spec:** `docs/superpowers/specs/2026-10-04-sma-700-dpop-proof-check-design.md` (approved at GATE 1 on 2026-10-04). Read the spec with this plan. The spec is the source of truth. If the code and this plan disagree with the spec, STOP and report to the coordinator.

## Decisions this plan made (the spec did not fix them)

| # | Decision | Why |
|---|---|---|
| P1 | RS256 test keys are **committed fixtures**, not an `rsa` dev-dependency: `src/adapters/oidc/testdata/dpop-rs256-test-key.pem` (PKCS#1, generated once by Task 4 Step 1) and `dpop-rs256-test-jwk.json` (its public `n` and `e`). `rs/deny.toml` does not change. | Key generation with `rsa` 0.9 needs a `rand_core` 0.6 RNG that the workspace does not carry, and a 2048-bit key generation in a debug test build is slow. A fixture adds no dependency edge. The key is a test key only and signs nothing outside the unit tests. |
| P2 | The `DpopProofChecker` port has a third method, `ath_matches(ath, token) -> bool`. | § 4.4 says the follow-up "reuses the check-8 code for `ath` in the verifier". The SHA-256 code is in the adapter, and the application layer must not import the adapter. |
| P3 | `RecordOutcome::CapacityFull` carries `{ entries: usize }`. | § 4.8 requires the `warn` line to show the entry count. |
| P4 | When both a quota and the global capacity are reached, `record` returns `QuotaExceeded` (429), not `CapacityFull` (503). Key quota is checked before subject quota. | A client over its own quota must not see an outage answer. |
| P5 | The request checks of § 4.8 (sizes, empty proof, forwarded path) run first in `resolve_dpop`, before `authenticate`. Their log line has the issuer `-`, because no token was verified yet. | § 4.8 lists them before `resolve_dpop`. Putting them at the start of `resolve_dpop` keeps one code path for the gRPC adapter. |
| P6 | A follow-up whose `ath` does not match the token gives `ProofDefect::Ath`; a refused redeem gives `ProofDefect::FollowUp`. | Both map to `invalid-dpop-proof` (D9). `Ath` names the real cause in the log line. |
| P7 | The gateway parser is `credentials(headers, dpop_enabled)`: it takes the switch. | § 4.10 says the `DPoP` scheme gives `None` when gateway DPoP is off. The parser needs the switch to decide that. |
| P8 | `IamConfig::validate` checks the `forwarded_base_urls` entries also when `enabled` is false. The chart checks them also when `enabled` is false. | A wrong entry is a wrong entry. The two sides stay equal. |
| P9 | The chart refuses a `forwardedBaseUrls` entry with a character outside printable ASCII, a space, `"` or `\` (the `idTokenMarkerClaims` rule), and accepts loopback `http` only for `localhost`, a dotted four-part `127.x.y.z` and `::1`. | Go's `%q` writes other characters as escapes that figment does not read. The chart may be stricter than IAM, never looser, or a render that passes stops the one IAM pod at boot. |
| P10 | Proof headers and payloads refuse a repeated top-level member name (`Malformed`). | Review Focus 1. `serde_json::Map` keeps the last of two equal names with no error. |
| P11 | The memory measurement for R8 is an `#[ignore]` integration test, `tests/dpop_replay_memory.rs`, with a counting global allocator. | A `#[global_allocator]` in the library's unit tests would apply to every unit test of the crate. |
| P12 | A JSON `e` member with a leading zero byte is refused (`Jwk`), like `n`. | RFC 7518 § 6.3.1 encodes both as the minimal octets. The thumbprint then has one form for one key. |

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for YAML, TOML, shell, Python; `{{/* SPDX-License-Identifier: Apache-2.0 */}}` for a Helm template). Fixture files (`.pem`, `.json`, golden `.yaml`) carry no header.
- Rust crates use edition 2024 and rust-version 1.95 (inherited). `rustfmt.toml` sets `max_width = 200`. Run `cargo fmt` in `rs/` after every Rust edit.
- `[workspace.lints.rust] warnings = "deny"`: dead code, an unused import and an unused variable are hard compile errors. Every task ends with the workspace compiling with no warning. A new item that no code uses yet must be `pub` in a library module (`paigasus-iam-core`, the `paigasus_iam` and `paigasus_gateway` libraries), never `pub(crate)`. A parameter that a later task starts to read is named `_scheme` until that task. Never add `#[allow(dead_code)]` to production code. (`tests/support/mod.rs` already marks its helpers `#[allow(dead_code)]`, because each test binary uses a subset; keep that convention there.)
- Clippy runs with `-D warnings`: use `u64::is_multiple_of`, not `% 2 == 0`.
- Prefix every shell with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `moon`, `cargo`, `buf`, `pnpm` resolve to the pinned versions.
- Work in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop` on branch `feature/sma-700-dpop-proof-check`. Check `git -C <worktree> branch --show-current` before the first commit of each task. Use absolute paths.
- After an edit to a `.proto` file, run `buf format -w` in `contracts/`, then `moon run contracts:generate --force`. Commit the regenerated Rust, Python and TypeScript output in the same commit. Never run a bare `buf generate`. In a worktree, `moon run contracts:breaking` cannot read `.git#branch=main` (`.git` is a file there). Run `cd contracts && buf breaking --against "$(git rev-parse --git-common-dir)#branch=main,subdir=contracts"` instead.
- Docker-backed IAM suites skip quietly when Docker is unreachable. Every IAM integration run in this plan sets `PAIGASUS_REQUIRE_DOCKER=1`, so a skip panics. Never record a Docker test as passed from a run that did not prove Docker was reachable.
- Never `git commit --amend`, never `git reset`, never `git rebase`, never `git stash`. Commit only the files of your own task. Never `--no-verify`. Never `--no-gpg-sign`: if signing fails with "failed to fill whole buffer", 1Password is locked; STOP and report.
- Conventional commits with a scope: `feat(contracts): …`, `feat(rs): …`, `test(rs): …`, `chore(charts): …`, `docs(rs): …`. No line in a commit body starts with `#` followed by digits, and no body line has the form `token: value` (commitlint `footer-leading-blank`). End every commit message with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Do not install software on the host (`brew`, `cargo install`, `npm -g`). If a tool is missing, STOP and report.
- If the Bash sandbox refuses a command that holds a variable or a pipe ("cannot be shown not to be git"), put the commands into a script file in the scratchpad and run it with `/bin/bash <script>`.
- **Logs.** No log line contains the proof, the token, the `jti`, the `jkt`, the forwarded path or a URL from a request. Refusal lines carry the issuer and a static defect name only (§ 4.8). `Jkt`, `DpopRequest`, `Credentials`, `ProofHeader` and `CallerCredential` implement `Debug` by hand and print no secret.
- **Registry codes.** `"invalid-dpop-proof"` and `"dpop-quota-exceeded"` may be spelled only in files that `ci/error-registry/check.py` already lists: `paigasus-iam/src/adapters/http/authn.rs` (emits), `paigasus-gateway/src/adapters/http/error.rs` (emits), `paigasus-iam/src/adapters/grpc/convert.rs`, `paigasus-gateway/src/adapters/http/auth.rs`, `paigasus-proto/src/error.rs` (asserts). Every new `src/` file reaches a reason through `ErrorReason::…::as_wire_reason()`. `tests/` files may spell codes. The challenge text `invalid_dpop_proof` (with underscores) is not a registry code.
- **IAM order (D18).** On the `DPoP` scheme, no identity lookup, provisioning, bootstrap seeding or API-key branch runs before the proof check.
- **Defaults off (D4, D11).** With `authn.dpop.enabled = false` and `dpop.enabled = false`, every response of both services and the default chart render are byte-identical to today.
- Verification commands for Rust: `cd rs && cargo nextest run --locked -p <crate> …` and Moon targets `paigasus-iam-rs:{test,lint,fmt}`, `paigasus-gateway-rs:{test,lint,fmt}`, `paigasus-iam-core-rs:{test,lint}`.
- The local bash rules of the development Mac apply to Task 17 (root `CLAUDE.md`, "This development Mac only"): `repo:affected-smoke` needs `/bin/bash` 3.2; `repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`, `repo:version-lockstep` and `repo:nats-permissions` need bash 4+; `repo:actionlint` needs bash 5 and a pipe of at least 8192 bytes (read its preflight line).

## Review Focus

The spec does not test these five inputs directly. Each one is likely to hurt a real client or operator. Each line names the task that adds its test.

1. **A proof with a repeated member name** (`{"typ":"dpop+jwt","typ":"JWT",…}` or two `jwk` members). A reasonable person expects a refusal, not "the last value wins". Task 4 adds `a_repeated_member_name_is_malformed` (decision P10).
2. **A `forwardedBaseUrls` entry with a trailing slash** (`https://gw.example.com/` or `https://gw.example.com/api/`). Operators write it so. The proof for `https://gw.example.com/api/v1/chat/completions` must still match. Task 7 adds `a_base_url_with_a_trailing_slash_still_matches`.
3. **A client request with a query string** (`POST /v1/chat/completions?trace=1`). The gateway must forward the path with no query, or IAM refuses it as `Malformed`. Task 13 adds `a_query_string_is_not_forwarded_in_the_path`.
4. **The same `jti` from two different keys** (two clients that both count from `1`). The replay key includes the `jkt`, so neither proof is a replay. Task 7 adds `the_same_jti_under_two_keys_is_not_a_replay`.
5. **An IPv6 loopback base URL for a local gateway** (`http://[::1]:8088`). `validate()` must accept it, the chart must accept it, and the `htu` comparison must work with the bracketed host. Task 6 adds `validate_accepts_the_loopback_forms`, Task 7 adds `an_ipv6_loopback_base_url_matches`, and Task 14 adds the refusals row `dpop IPv6 loopback http`.

---

## File structure

| File | Task | Responsibility |
|---|---|---|
| `contracts/proto/paigasus/iam/v1/iam.proto` | 1 | `DpopContext`, `IntrospectRequest.dpop = 2`, `IsAuthorized` comment |
| `contracts/proto/paigasus/common/v1/error.proto` | 1 | reasons 46 and 909 |
| `rs/crates/libs/paigasus-proto/src/generated/**`, `py/packages/paigasus-proto/**/generated/**`, `ts/packages/paigasus-proto/src/generated/**` | 1 | regenerated |
| `rs/crates/libs/paigasus-proto/src/error.rs`, `ts/packages/paigasus-proto/src/error.test.ts`, `ts/packages/paigasus-sdk/{src/errors/presentation.ts,tests/presentation.test.ts}` | 1 | counts 67 → 69, presentation entries |
| `rs/crates/libs/paigasus-iam-core/src/{authn,ports,lib}.rs` | 2 | types and ports |
| `rs/crates/services/paigasus-iam/src/adapters/{http/authn.rs,grpc/convert.rs,retryable.rs,http/mod.rs}` | 2 | exhaustive arms, `RetryInfo` |
| `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs`, `tests/authn_private_ca.rs`, `src/application/authenticate_token.rs` | 2, 3 | the scheme, step 7 |
| `rs/crates/services/paigasus-iam/src/adapters/oidc/{dpop.rs,mod.rs,testdata/*}` | 4 | `JoseDpopProofChecker` |
| `rs/crates/services/paigasus-iam/src/adapters/{dpop_replay.rs,mod.rs}`, `tests/dpop_replay_memory.rs` | 5 | `InMemoryReplayStore`, R8 measurement |
| `rs/crates/services/paigasus-iam/src/config.rs`, `iam.toml.example`, `src/service_info.rs`, `tests/{support/mod.rs,keycloak_e2e.rs,zitadel_e2e.rs}` | 6 | `DpopConfig` |
| `rs/crates/services/paigasus-iam/src/application/{dpop.rs,mod.rs}` | 7 | `DpopProofVerifier` |
| `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs`, `src/adapters/grpc/authn.rs`, `src/adapters/http/mod.rs` | 8 | `resolve_dpop`, `Introspect`, wiring |
| `rs/crates/services/paigasus-iam/src/adapters/{auth.rs,grpc/authn.rs,grpc/authz.rs,grpc/mod.rs}`, `src/main.rs` | 9 | the follow-up |
| `rs/crates/services/paigasus-iam/tests/{support/mod.rs,grpc_authn.rs,grpc_whoami.rs,http_authn.rs,keycloak_e2e.rs}` | 10 | integration tests |
| `rs/crates/services/paigasus-gateway/src/{config.rs,service_info.rs,main.rs}`, `src/adapters/http/{auth.rs,mod.rs}`, `tests/*.rs` | 11 | config, `AuthState`, parser |
| `rs/crates/services/paigasus-gateway/src/adapters/iam/{client.rs,mod.rs}` and every `Iam` implementer | 12 | `Iam` trait |
| `rs/crates/services/paigasus-gateway/src/adapters/http/{auth.rs,error.rs}`, `tests/chat_proxy.rs`, `gateway.toml.example` | 13 | the flow, errors, challenge |
| `charts/paigasus/{values.yaml,README.md}`, `templates/{_iam-backend.tpl,backend-deployment.yaml}`, `tests/{refusals.sh,env.sh,render.sh,golden/iam-dpop.yaml}` | 14 | chart |
| `docs/ops/RUNBOOK-chart.md`, `rs/crates/services/{paigasus-iam,paigasus-gateway}/CHANGELOG.md`, the spec R8 row | 15 | docs |
| `docs/superpowers/plans/2026-10-04-sma-700-mutation-results.md` | 16 | mutation battery |

---

### Task 1: The contract, the two registry codes, and every literal that must compile

The new enum values make the TypeScript `PRESENTATION` table (a total `Record`) fail to compile, and the new `IntrospectRequest` field makes five Rust struct literals fail to compile. So the proto change, the regenerated output, the TS entries, the counts and the five literals land in ONE commit.

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto:242-244` and the `rpc IsAuthorized` line (`:412`)
- Modify: `contracts/proto/paigasus/common/v1/error.proto` (after `ERROR_REASON_EXTERNAL_IDENTITY_EXISTS = 45;` near line 208, and after `ERROR_REASON_INVALID_PATH_SEGMENT = 908;` near line 300)
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/**`, `py/packages/paigasus-proto/src/paigasus_proto/generated/**`, `ts/packages/paigasus-proto/src/generated/**`
- Modify: `rs/crates/libs/paigasus-proto/src/error.rs:154-243` (and a new test)
- Modify: `ts/packages/paigasus-sdk/src/errors/presentation.ts:12`, `:67`, `:92`
- Modify: `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`, `ts/packages/paigasus-proto/src/error.test.ts:56-60`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs:125`
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_authn.rs:94`, `:126`; `tests/grpc_whoami.rs:119`, `:132`

**Interfaces:**
- Consumes: nothing.
- Produces: proto message `paigasus_proto::paigasus::iam::v1::DpopContext { proof: String, method: String, path: String }`; `IntrospectRequest { token: String, dpop: Option<DpopContext> }`; `ErrorReason::InvalidDpopProof` (909, wire `invalid-dpop-proof`); `ErrorReason::DpopQuotaExceeded` (46, wire `dpop-quota-exceeded`); TS `ErrorReason.INVALID_DPOP_PROOF`, `ErrorReason.DPOP_QUOTA_EXCEEDED`.

- [ ] **Step 1: Write the failing Rust registry test**

In `rs/crates/libs/paigasus-proto/src/error.rs`, add to the `EXPECTED_REASONS` list. After the `// IAM: identity links (SMA-712)` block (after `"external-identity-exists",`):

```rust
        // IAM: DPoP (SMA-700)
        "dpop-quota-exceeded",
```

At the end of the `// Shared` block (after `"invalid-path-segment",`):

```rust
        "invalid-dpop-proof",
```

Change the count line:

```rust
        assert_eq!(actual.len(), 69, "the registry should hold 69 reasons");
```

Add this test at the end of the `tests` module (before the closing `}`):

```rust
    /// SMA-700: the two DPoP reasons, by wire string AND by number. The number is the
    /// SMA-498 D3 range rule: `invalid-dpop-proof` is emitted by IAM and by the gateway, so it is
    /// in the shared range (D19); `dpop-quota-exceeded` is IAM-only. The registry is append-only,
    /// so a wrong number is permanent.
    #[test]
    fn the_dpop_reasons_resolve_both_ways_in_their_ranges() {
        assert_eq!(ErrorReason::InvalidDpopProof.as_wire_reason().as_deref(), Some("invalid-dpop-proof"));
        assert_eq!(ErrorReason::from_wire_reason("invalid-dpop-proof"), Some(ErrorReason::InvalidDpopProof));
        assert_eq!(ErrorReason::InvalidDpopProof as i32, 909);
        assert_eq!(ErrorReason::DpopQuotaExceeded.as_wire_reason().as_deref(), Some("dpop-quota-exceeded"));
        assert_eq!(ErrorReason::from_wire_reason("dpop-quota-exceeded"), Some(ErrorReason::DpopQuotaExceeded));
        assert_eq!(ErrorReason::DpopQuotaExceeded as i32, 46);
    }
```

- [ ] **Step 2: Run it to see it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo nextest run --locked -p paigasus-proto -E 'test(the_dpop_reasons) | test(the_registry_contains_exactly)'
```

Expected: compile error `no variant or associated item named InvalidDpopProof found for enum ErrorReason`.

- [ ] **Step 3: Edit `error.proto`**

After `ERROR_REASON_EXTERNAL_IDENTITY_EXISTS = 45;` add:

```proto

  // ---- IAM: DPoP (1-299) ---------------------------------------------------
  // SMA-700. Emitted by Introspect when the replay store refuses a proof.

  // "dpop-quota-exceeded" — the caller sent more DPoP proofs than its quota allows inside the
  // proof window. Retry after the RetryInfo delay. The gateway answers with its own
  // "rate-limited".
  ERROR_REASON_DPOP_QUOTA_EXCEEDED = 46;
```

After `ERROR_REASON_INVALID_PATH_SEGMENT = 908;` add:

```proto
  // In the shared range (900-999). Emitted by IAM and by the gateway (SMA-700 D19).
  // "invalid-dpop-proof" — the DPoP proof is missing, malformed, or does not match the
  // request or the token.
  ERROR_REASON_INVALID_DPOP_PROOF = 909;
```

- [ ] **Step 4: Edit `iam.proto`**

Replace lines 242-244 (`message IntrospectRequest { string token = 1; }`) with:

```proto
// The DPoP context of a client request that the gateway authenticates (RFC 9449, SMA-700).
// IAM checks the proof against authn.dpop.forwarded_base_urls.
message DpopContext {
  // The value of the client's `DPoP` header: one JWS in compact form. At most 8192 bytes.
  string proof = 1;
  // The HTTP method of the client's request, for example "POST". At most 16 bytes.
  string method = 2;
  // The path of the client's request as the gateway received it, with no query. It starts
  // with "/". At most 2048 bytes.
  string path = 3;
}

message IntrospectRequest {
  string token = 1;
  // Present when the client used the DPoP scheme (SMA-700). Absent: the Bearer scheme.
  DpopContext dpop = 2;
}
```

Replace the line `  rpc IsAuthorized(IsAuthorizedRequest) returns (IsAuthorizedResponse);` with:

```proto
  // Bearer-enforced. SMA-700: when IAM has DPoP on, this RPC alone also accepts a one-time
  // follow-up of an Introspect with a DPoP context: the metadata `authorization: DPoP <token>`
  // plus `dpop: <proof>`, with the same token and the same proof bytes. IAM accepts it once,
  // only as a self-query (principal_prn is the caller), and only before the ticket deadline.
  rpc IsAuthorized(IsAuthorizedRequest) returns (IsAuthorizedResponse);
```

- [ ] **Step 5: Format, lint, regenerate, check the breaking gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/contracts
buf format -w
moon run contracts:fmt contracts:lint
moon run contracts:generate --force
buf breaking --against "$(git rev-parse --git-common-dir)#branch=main,subdir=contracts"
cd .. && git status --short
```

Expected: `buf breaking` prints nothing and exits 0. `git status` shows the two `.proto` files and generated files under `rs/crates/libs/paigasus-proto/src/generated/`, `py/packages/paigasus-proto/src/paigasus_proto/generated/` and `ts/packages/paigasus-proto/src/generated/`. If `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` shows as DELETED, the second `buf generate` failed (a BSR outage): run `moon run contracts:generate --force` again; never commit that deletion.

- [ ] **Step 6: Add `dpop: None` to the five `IntrospectRequest` literals**

`rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs:125`:

```rust
        let resp = self.authn.clone().introspect(with_correlation(Request::new(IntrospectRequest { token: token.to_owned(), dpop: None }))).await?;
```

`rs/crates/services/paigasus-iam/tests/grpc_authn.rs:94` and `:126`:

```rust
    let ctx = authn.introspect(IntrospectRequest { token: token.clone(), dpop: None }).await.unwrap().into_inner();
```

```rust
    let ctx = authn.introspect(IntrospectRequest { token, dpop: None }).await.unwrap().into_inner();
```

`rs/crates/services/paigasus-iam/tests/grpc_whoami.rs:119` and `:132`:

```rust
    let err = client.introspect(IntrospectRequest { token: token.clone(), dpop: None }).await.unwrap_err();
```

```rust
    let ctx = client.introspect(IntrospectRequest { token, dpop: None }).await.unwrap().into_inner();
```

- [ ] **Step 7: The TypeScript presentation entries and counts**

`ts/packages/paigasus-sdk/src/errors/presentation.ts` line 12: change `68 keys rather than 67` to `70 keys rather than 69`.

After `[ErrorReason.EXTERNAL_IDENTITY_EXISTS]: 'from-transport',` add:

```ts
  // SMA-700. IAM's own DPoP quota refusal (gRPC RESOURCE_EXHAUSTED with RetryInfo). The transport
  // table already presents it as rate-limited ("try again soon").
  [ErrorReason.DPOP_QUOTA_EXCEEDED]: 'from-transport',
```

After `[ErrorReason.INVALID_PATH_SEGMENT]: 'from-transport',` add:

```ts
  // SMA-700. A 401 from both services; the transport table already presents it as relogin.
  [ErrorReason.INVALID_DPOP_PROOF]: 'from-transport',
```

`ts/packages/paigasus-sdk/tests/presentation.test.ts:13`: `expect(reasons).toHaveLength(69);`

`ts/packages/paigasus-proto/src/error.test.ts`: change the comment `asserts 67 at` to `asserts 69 at` and `expect(values).toHaveLength(67);` to `expect(values).toHaveLength(69);`.

- [ ] **Step 8: Run everything this task touches**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
(cd rs && cargo nextest run --locked -p paigasus-proto && cargo build --locked --workspace --all-targets)
moon run paigasus-proto-ts:test paigasus-sdk-ts:test paigasus-sdk-ts:typecheck ts:fmt
python3 ci/error-registry/check.py --self-test && python3 ci/error-registry/check.py --single-site
moon run paigasus-proto-py:build
```

Expected: all pass. `check.py --single-site` passes with no MANIFEST change: no new file spells a new code in this task. If it names an offender, add the row it asks for in `ci/error-registry/check.py` `MANIFEST` with the role the README table prescribes, and re-run. If `ts:fmt` reports a diff, run `pnpm -C ts exec prettier --write <file>` on the named file and re-run.

- [ ] **Step 9: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git add contracts/proto rs/crates/libs/paigasus-proto py/packages/paigasus-proto ts/packages/paigasus-proto ts/packages/paigasus-sdk \
  rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs rs/crates/services/paigasus-iam/tests/grpc_authn.rs rs/crates/services/paigasus-iam/tests/grpc_whoami.rs
git commit -m "feat(contracts): DPoP context on Introspect and the two DPoP error reasons (SMA-700)

IntrospectRequest gets an optional DpopContext. The registry gets
invalid-dpop-proof (909, shared) and dpop-quota-exceeded (46, IAM).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Core types, the two ports, the scheme on the authenticator port, and the exhaustive arms

The scheme parameter and the new `AuthnError` variants touch every `Authenticator` implementer and every exhaustive `match` on `AuthnError`. All of them change in this task, so the workspace compiles at its end. Every caller passes `TokenScheme::Bearer` for now; Task 3 makes the validator read it.

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authn.rs` (after `Issuer`'s `Display` impl at `:50`; `ValidatedClaims` `:55-65`; `TokenDefect` `:161-179`; `AuthnError` `:190-204`; tests)
- Modify: `rs/crates/libs/paigasus-iam-core/src/ports.rs:8`, `:342-347`, and new items after the `Authenticator` trait; object-safety test `:557-566`
- Modify: `rs/crates/libs/paigasus-iam-core/src/lib.rs:21` and `:28-32`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs:25`, `:372-373`, `:446-457`, and the test call sites
- Modify: `rs/crates/services/paigasus-iam/tests/authn_private_ca.rs:59`, `:83`, `:97`, `:124` and its imports
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:149-157`
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs:8-11`, `:189`, `:397-408`, `:602-607`, `:620-625`, `:646-651`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:7-10`, `:45-54`, `:77-82`, `:134-155`, and the table test `:886-915`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs:38-66` and its tests
- Modify: `rs/crates/services/paigasus-iam/src/adapters/retryable.rs:22-28`, `:39-62`, `:87-98`

**Interfaces:**
- Consumes: `ErrorReason::{InvalidDpopProof, DpopQuotaExceeded}` (Task 1).
- Produces (all in `paigasus_iam_core`, re-exported at the crate root):

```rust
pub enum TokenScheme { Bearer, Dpop }                                   // Debug, Clone, Copy, PartialEq, Eq, Hash
pub struct Jkt(String);                                                  // Clone, PartialEq, Eq, Hash; Debug prints "Jkt(..)"
impl Jkt { pub fn new(thumbprint: impl Into<String>) -> Self; pub fn as_str(&self) -> &str; }
pub struct ValidatedClaims { /* existing fields */ pub key_binding: Option<Jkt> }
pub enum TokenDefect { /* existing */ NotKeyBound }
pub enum ProofDefect { Missing, Malformed, Typ, Alg, Jwk, Signature, Htm, Htu, Iat, Ath, Thumbprint, Replayed, FollowUp } // Debug, Clone, Copy, PartialEq, Eq, Hash
impl ProofDefect { pub const ALL: [ProofDefect; 13]; pub fn as_str(self) -> &'static str; }
pub enum AuthnError { /* existing */ InvalidDpopProof(ProofDefect), DpopQuotaExceeded { retry_after_secs: u32 } }

#[async_trait] pub trait Authenticator: Send + Sync {
    async fn authenticate(&self, token: &str, scheme: TokenScheme) -> Result<ValidatedClaims, AuthnError>;
}
pub struct ProofClaims { pub jti: String, pub iat: i64, pub htm: String, pub htu: String }       // Debug, Clone, PartialEq, Eq
pub struct FollowUpClaims { pub jti: String, pub ath: String }                                   // Debug, Clone, PartialEq, Eq
pub trait DpopProofChecker: Send + Sync {
    fn check(&self, proof: &str, token: &str, jkt: &Jkt) -> Result<ProofClaims, ProofDefect>;
    fn follow_up_claims(&self, proof: &str) -> Result<FollowUpClaims, ProofDefect>;
    fn ath_matches(&self, ath: &str, token: &str) -> bool;
}
pub struct ProofKey(pub [u8; 16]);                                       // Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord
pub struct NewProof { pub key: ProofKey, pub subject: [u8; 16], pub jkt: [u8; 16], pub expires_at: i64, pub follow_up_deadline: i64, pub follow_up_digest: [u8; 32] } // Debug, Clone, Copy, PartialEq, Eq
pub enum RecordOutcome { Fresh, Replayed, QuotaExceeded { retry_after_secs: u32 }, CapacityFull { entries: usize } } // Debug, Clone, Copy, PartialEq, Eq
pub enum RedeemOutcome { Redeemed, Refused }                             // Debug, Clone, Copy, PartialEq, Eq
pub trait ReplayStore: Send + Sync {
    fn record(&self, entry: NewProof, now: i64) -> RecordOutcome;
    fn redeem_follow_up(&self, key: ProofKey, proof_digest: [u8; 32], now: i64) -> RedeemOutcome;
}
```

- In `paigasus_iam::adapters::grpc::convert`: `authn_status` maps `InvalidDpopProof(_)` → `Unauthenticated`, reason `invalid-dpop-proof`, message `"invalid DPoP proof"`, retryable `false`; `DpopQuotaExceeded { retry_after_secs }` → `ResourceExhausted`, reason `dpop-quota-exceeded`, message `"too many DPoP proofs"`, retryable `true`, plus `google.rpc.RetryInfo` with `retry_delay = max(retry_after_secs, 1)` seconds.
- In `paigasus_iam::adapters::http::authn`: `InvalidDpopProof(_)` → 401 `invalid-dpop-proof` (no `WWW-Authenticate`); `DpopQuotaExceeded` → 429 `dpop-quota-exceeded` with `Retry-After`.

- [ ] **Step 1: Write the failing core tests**

At the end of the `tests` module in `rs/crates/libs/paigasus-iam-core/src/authn.rs`, add:

```rust
    #[test]
    fn jkt_debug_never_prints_the_thumbprint() {
        // SMA-700 § 4.8: logs never contain the jkt. A `{:?}` of a `ValidatedClaims` must not leak it.
        let jkt = Jkt::new("0ZcOCORZNYy-DWpqq30jZyJGHTN0d2HglBV3uiguA4I");
        assert_eq!(format!("{jkt:?}"), "Jkt(..)");
        assert_eq!(jkt.as_str(), "0ZcOCORZNYy-DWpqq30jZyJGHTN0d2HglBV3uiguA4I");
    }

    #[test]
    fn every_proof_defect_has_one_distinct_static_name() {
        let names: std::collections::HashSet<&str> = ProofDefect::ALL.iter().map(|d| d.as_str()).collect();
        assert_eq!(names.len(), ProofDefect::ALL.len(), "two defects share a log name");
        // Exhaustiveness guard: no wildcard arm, so a new variant must be added to ALL by hand.
        for defect in ProofDefect::ALL {
            match defect {
                ProofDefect::Missing
                | ProofDefect::Malformed
                | ProofDefect::Typ
                | ProofDefect::Alg
                | ProofDefect::Jwk
                | ProofDefect::Signature
                | ProofDefect::Htm
                | ProofDefect::Htu
                | ProofDefect::Iat
                | ProofDefect::Ath
                | ProofDefect::Thumbprint
                | ProofDefect::Replayed
                | ProofDefect::FollowUp => {}
            }
        }
    }

    #[test]
    fn the_new_authn_errors_display_no_detail() {
        assert_eq!(AuthnError::DpopQuotaExceeded { retry_after_secs: 7 }.to_string(), "DPoP proof quota exceeded");
        assert_eq!(AuthnError::InvalidDpopProof(ProofDefect::Htu).to_string(), "invalid DPoP proof: Htu");
    }
```

At the end of the `tests` module in `rs/crates/libs/paigasus-iam-core/src/ports.rs`, add:

```rust
    // Compile-time proof the SMA-700 ports are object-safe (`AppState` holds them as `Arc<dyn …>`).
    #[allow(dead_code)]
    fn dpop_ports_are_object_safe(_: &dyn DpopProofChecker, _: &dyn ReplayStore) {}
```

- [ ] **Step 2: Run them to see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo nextest run --locked -p paigasus-iam-core
```

Expected: compile errors `cannot find type Jkt`, `cannot find type ProofDefect`, `cannot find trait DpopProofChecker`.

- [ ] **Step 3: Implement the types in `authn.rs`**

After the `impl fmt::Display for Issuer` block, add:

```rust
/// Which `Authorization` scheme presented an access token (SMA-700). The validator's key-binding
/// rule depends on it (spec § 4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenScheme {
    /// `Authorization: Bearer`. A token bound to a key is refused (SMA-690 D9, SMA-700 D7).
    Bearer,
    /// The DPoP scheme (RFC 9449). The token must carry `cnf.jkt` and no certificate binding.
    Dpop,
}

/// The RFC 7638 thumbprint of the key that a token is bound to: the token's `cnf.jkt` claim
/// (RFC 9449 § 6.1), as the base64url string the token carries. `Debug` does not print the value,
/// so a log line that prints a `ValidatedClaims` cannot carry it (SMA-700 § 4.8).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Jkt(String);

impl Jkt {
    #[must_use]
    pub fn new(thumbprint: impl Into<String>) -> Self {
        Jkt(thumbprint.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Jkt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Jkt(..)")
    }
}
```

In `ValidatedClaims`, after `pub zoneinfo: Option<String>,` add:

```rust
    /// The `cnf.jkt` of the token. `Some` only on the `Dpop` scheme (SMA-700 § 4.3); always
    /// `None` on the `Bearer` scheme, which refuses a bound token.
    pub key_binding: Option<Jkt>,
```

In `TokenDefect`, after `SenderConstrained,` add:

```rust
    /// The `Dpop` scheme with a token that has no `cnf.jkt`, or that is also bound to a
    /// certificate (`cnf.x5t#S256`, RFC 8705). SMA-700 § 4.3.
    NotKeyBound,
```

After the `ProvisioningDefect` enum, add:

```rust
/// Why a DPoP proof was refused (SMA-700 § 4.4, § 4.8): one variant for each check. Every defect
/// has the one wire reason `invalid-dpop-proof` (D9). `as_str` is the static name that a refusal
/// log line carries; the line never carries the proof, the token, the `jti` or the `jkt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProofDefect {
    /// No proof (an empty `proof`, or no `dpop` metadata on the follow-up).
    Missing,
    /// Too large, not three base64url parts, not a JSON object, a repeated member, or a missing or
    /// wrong-typed claim (checks 1, 2 and 7), or a bad forwarded request (§ 4.8).
    Malformed,
    Typ,
    Alg,
    Jwk,
    Signature,
    Htm,
    Htu,
    Iat,
    Ath,
    Thumbprint,
    Replayed,
    /// The `IsAuthorized` follow-up ticket is unknown, used, expired, for other proof bytes, or the
    /// follow-up is not a self-query (§ 4.8).
    FollowUp,
}

impl ProofDefect {
    /// Every variant, for exhaustive tests and the log-name check.
    pub const ALL: [ProofDefect; 13] = [
        ProofDefect::Missing,
        ProofDefect::Malformed,
        ProofDefect::Typ,
        ProofDefect::Alg,
        ProofDefect::Jwk,
        ProofDefect::Signature,
        ProofDefect::Htm,
        ProofDefect::Htu,
        ProofDefect::Iat,
        ProofDefect::Ath,
        ProofDefect::Thumbprint,
        ProofDefect::Replayed,
        ProofDefect::FollowUp,
    ];

    /// The static defect name of a refusal log line.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ProofDefect::Missing => "missing",
            ProofDefect::Malformed => "malformed",
            ProofDefect::Typ => "typ",
            ProofDefect::Alg => "alg",
            ProofDefect::Jwk => "jwk",
            ProofDefect::Signature => "signature",
            ProofDefect::Htm => "htm",
            ProofDefect::Htu => "htu",
            ProofDefect::Iat => "iat",
            ProofDefect::Ath => "ath",
            ProofDefect::Thumbprint => "thumbprint",
            ProofDefect::Replayed => "replayed",
            ProofDefect::FollowUp => "follow_up",
        }
    }
}
```

In `AuthnError`, after `Unavailable,` (before `Backend`), add:

```rust
    /// A DPoP proof failed a check (SMA-700). gRPC `Unauthenticated`, reason `invalid-dpop-proof`.
    #[error("invalid DPoP proof: {0:?}")]
    InvalidDpopProof(ProofDefect),
    /// The replay store refused the proof because a key or subject quota is full (SMA-700 D10).
    /// gRPC `ResourceExhausted`, reason `dpop-quota-exceeded`, with `RetryInfo`.
    #[error("DPoP proof quota exceeded")]
    DpopQuotaExceeded { retry_after_secs: u32 },
```

- [ ] **Step 4: Implement the ports in `ports.rs`**

Change line 8 to:

```rust
use crate::authn::{AuthnError, ExternalIdentity, Issuer, Jkt, ProofDefect, TokenScheme, ValidatedClaims};
```

Replace the `Authenticator` trait (`:342-347`) with:

```rust
/// Verifies a presented access token and extracts its claims.
#[async_trait]
pub trait Authenticator: Send + Sync {
    /// The pluggable port (ADR-0015). OIDC validator is the v1 impl. `scheme` decides the
    /// key-binding rule (SMA-700 § 4.3): `Bearer` refuses a bound token, `Dpop` requires one and
    /// returns its `cnf.jkt` in `ValidatedClaims::key_binding`.
    async fn authenticate(&self, token: &str, scheme: TokenScheme) -> Result<ValidatedClaims, AuthnError>;
}

/// The claims of a DPoP proof that passed checks 1-9 of SMA-700 § 4.4. `htu` stays a string: the
/// URL parse is in the application service, so this crate needs no `url` dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofClaims {
    pub jti: String,
    pub iat: i64,
    pub htm: String,
    pub htu: String,
}

/// The `jti` and `ath` of a follow-up proof, read with no signature check (SMA-700 § 4.8). The
/// follow-up relies on the digest of the bytes that `Introspect` verified, not on the signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowUpClaims {
    pub jti: String,
    pub ath: String,
}

/// The stateless checks of a DPoP proof (SMA-700 D2). The JOSE code is in the adapter
/// (`adapters/oidc/dpop.rs`), so the application service needs no JOSE crate.
pub trait DpopProofChecker: Send + Sync {
    /// Checks 1-9 of SMA-700 § 4.4 against the access token and the token's `cnf.jkt`.
    fn check(&self, proof: &str, token: &str, jkt: &Jkt) -> Result<ProofClaims, ProofDefect>;
    /// Checks 1 and 2 and the `jti` and `ath` part of check 7, with no signature check.
    fn follow_up_claims(&self, proof: &str) -> Result<FollowUpClaims, ProofDefect>;
    /// Check 8: `ath` is base64url, with no padding, of SHA-256 over the token's ASCII bytes.
    fn ath_matches(&self, ath: &str, token: &str) -> bool;
}

/// The replay key of one proof: the first 16 bytes of `blake3(jkt || 0x00 || jti)` (SMA-700
/// § 4.5). The application service derives it; the core has no hash dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProofKey(pub [u8; 16]);

/// One accepted proof for [`ReplayStore::record`] (SMA-700 § 4.5). `subject` and `jkt` are
/// 16-byte hashes, so the store holds no claim value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewProof {
    pub key: ProofKey,
    /// The first 16 bytes of `blake3(issuer || 0x00 || sub)`.
    pub subject: [u8; 16],
    /// The first 16 bytes of `blake3(jkt)`.
    pub jkt: [u8; 16],
    /// `iat + iat_window_secs`: the entry is live while `now <= expires_at`.
    pub expires_at: i64,
    /// `max(expires_at, now + 30)`: the follow-up ticket is live while `now <= follow_up_deadline`.
    pub follow_up_deadline: i64,
    /// `blake3` of the whole proof string, as received, with no trim.
    pub follow_up_digest: [u8; 32],
}

/// The answer of [`ReplayStore::record`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordOutcome {
    Fresh,
    Replayed,
    /// A `jkt` or subject quota is full. `retry_after_secs` is the time until the oldest entry of
    /// that `jkt` or subject is removed (at least 1).
    QuotaExceeded { retry_after_secs: u32 },
    /// The global capacity is full. `entries` is the occupancy, for the `warn` line (D12).
    CapacityFull { entries: usize },
}

/// The answer of [`ReplayStore::redeem_follow_up`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedeemOutcome {
    Redeemed,
    Refused,
}

/// The DPoP replay store (SMA-700 D3, § 4.5). Synchronous: the one adapter is in memory, and a
/// shared (Redis) adapter would change the port to async with it. Time comes only from `now`
/// (Unix seconds from the `Clock` port).
pub trait ReplayStore: Send + Sync {
    fn record(&self, entry: NewProof, now: i64) -> RecordOutcome;
    /// `Redeemed` only when the ticket of `key` is live, not used, and `proof_digest` equals the
    /// recorded digest. It then marks the ticket as used. A digest mismatch does not use it.
    fn redeem_follow_up(&self, key: ProofKey, proof_digest: [u8; 32], now: i64) -> RedeemOutcome;
}
```

- [ ] **Step 5: Re-export from `lib.rs`**

Replace line 21 with:

```rust
pub use authn::{AuthnError, AuthnPrincipal, Credential, ExternalIdentity, Issuer, Jkt, PrincipalContext, ProofDefect, ProvisioningDefect, TokenDefect, TokenScheme, ValidatedClaims};
```

Replace the `pub use ports::{ … };` block with:

```rust
pub use ports::{
    ApiKeyRepository, AuditLog, Authenticator, Clock, ConflictKind, DpopProofChecker, EmailChange, EntityGenBumper, EventPublisher, ExternalIdentityRepository, FollowUpClaims, IdGenerator,
    IdentityLinkStore, KeyEntropy, MembershipAxis, MembershipKindQuery, MembershipRecord, MembershipRepository, Mutated, NewProof, NodeView, OrganizationRepository, Outbox, PolicyGenBumper,
    PreconditionKind, PrincipalRepository, ProjectRepository, ProofClaims, ProofKey, PublishError, RecordOutcome, RedeemOutcome, ReplayStore, RepositoryError, Savepoint, SecretHasher,
    ServiceAccountRepository, TeamRepository, Transaction, UnitOfWork, UserWithIdentities,
};
```

Run `cargo nextest run --locked -p paigasus-iam-core`. Expected: PASS (the service crate does not compile yet; the next steps fix it).

- [ ] **Step 6: Pass the scheme through every `Authenticator` implementer and caller**

`validator.rs` line 25:

```rust
use paigasus_iam_core::{Authenticator, AuthnError, Clock, Issuer, TokenDefect, TokenScheme, ValidatedClaims};
```

`validator.rs:373` (the signature; Task 3 renames the parameter):

```rust
    async fn authenticate(&self, token: &str, _scheme: TokenScheme) -> Result<ValidatedClaims, AuthnError> {
```

`validator.rs`, in the final `Ok(ValidatedClaims { … })`, after `zoneinfo: claims.zoneinfo,` add `key_binding: None,`.

In the `validator.rs` test module and in `tests/authn_private_ca.rs`, every call `authenticator.authenticate(&token)`, `authn.authenticate(&token)` and their siblings passes `TokenScheme::Bearer`. Run this once (BSD `sed` on macOS):

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs/crates/services/paigasus-iam
sed -i '' -E 's/\.authenticate\((&[a-z_]+)\)\.await/.authenticate(\1, TokenScheme::Bearer).await/g' src/adapters/oidc/validator.rs tests/authn_private_ca.rs
grep -n '\.authenticate(' src/adapters/oidc/validator.rs tests/authn_private_ca.rs | grep -v 'TokenScheme::Bearer'
```

Expected: the `grep` prints only `validator.rs:373` (the `fn authenticate` signature line). The validator test module uses `use super::*;`, so `TokenScheme` is in scope there. In `tests/authn_private_ca.rs`, add `use paigasus_iam_core::TokenScheme;` to its imports (it already names `paigasus_iam` items; put the line beside them).

`adapters/http/mod.rs:149-157` (`WiredAuthenticator`):

```rust
#[async_trait]
impl Authenticator for WiredAuthenticator {
    async fn authenticate(&self, token: &str, scheme: TokenScheme) -> Result<ValidatedClaims, AuthnError> {
        match self {
            WiredAuthenticator::Memory(inner) => inner.authenticate(token, scheme).await,
            WiredAuthenticator::Redis(inner) => inner.authenticate(token, scheme).await,
        }
    }
}
```

Add `TokenScheme` to that file's existing `use paigasus_iam_core::{…}` line.

`application/authenticate_token.rs`: add `TokenScheme` to the `use paigasus_iam_core::{…}` block, and change line 189 to:

```rust
        let claims = self.authenticator.authenticate(token, TokenScheme::Bearer).await?;
```

In its test module, `claims_with_profile` (`:397-408`) gets `key_binding: None,` after `zoneinfo: …`. The three test authenticators change their signature to:

```rust
        async fn authenticate(&self, _token: &str, _scheme: TokenScheme) -> Result<ValidatedClaims, AuthnError> {
```

(`FakeAuthenticator` `:604`, `QueueAuthenticator` `:622`, `PanicIfCalledAuthenticator` `:648`; the bodies do not change.)

- [ ] **Step 7: Write the failing mapping tests**

`adapters/grpc/convert.rs`, in `every_authn_status_carries_a_registered_reason_and_its_original_message`, change the import line to `use paigasus_iam_core::{ProofDefect, ProvisioningDefect, TokenDefect};` and add two rows to `cases` (after the `Unavailable` row):

```rust
            (AuthnError::InvalidDpopProof(ProofDefect::Htu), Code::Unauthenticated, "invalid-dpop-proof", "invalid DPoP proof"),
            (AuthnError::DpopQuotaExceeded { retry_after_secs: 7 }, Code::ResourceExhausted, "dpop-quota-exceeded", "too many DPoP proofs"),
            (AuthnError::InvalidToken(TokenDefect::NotKeyBound), Code::Unauthenticated, "invalid-token", "invalid bearer token"),
```

Change the doc line above that test from `five codes` to `the authn codes`. After that test add:

```rust
    /// SMA-700 D10: the quota refusal carries `google.rpc.RetryInfo`, so the gateway can send
    /// `Retry-After`. It is retryable, and no other authn status carries RetryInfo.
    #[test]
    fn the_dpop_quota_refusal_carries_retry_info_and_is_retryable() {
        use tonic_types::StatusExt;

        let status = authn_status(&AuthnError::DpopQuotaExceeded { retry_after_secs: 7 });
        let details = status.get_error_details();
        let retry = details.retry_info().expect("RetryInfo");
        assert_eq!(retry.retry_delay, Some(std::time::Duration::from_secs(7)));
        assert_eq!(details.error_info().expect("ErrorInfo").metadata.get("retryable").map(String::as_str), Some("true"));

        // A zero is never sent: a client would retry at once.
        let zero = authn_status(&AuthnError::DpopQuotaExceeded { retry_after_secs: 0 });
        assert_eq!(zero.get_error_details().retry_info().expect("RetryInfo").retry_delay, Some(std::time::Duration::from_secs(1)));

        let proof = authn_status(&AuthnError::InvalidDpopProof(paigasus_iam_core::ProofDefect::Replayed));
        assert!(proof.get_error_details().retry_info().is_none());
        assert_eq!(proof.get_error_details().error_info().expect("ErrorInfo").metadata.get("retryable").map(String::as_str), Some("false"));
    }
```

`adapters/http/authn.rs` tests: after `unavailable_is_503`, add:

```rust
    #[tokio::test]
    async fn an_invalid_dpop_proof_is_401_with_no_bearer_challenge() {
        // SMA-700: no IAM HTTP route accepts the DPoP scheme, but the funnel is exhaustive. The
        // Bearer challenge belongs to `invalid-token` only.
        let (status, challenge, body) = rendered(AuthnError::InvalidDpopProof(paigasus_iam_core::ProofDefect::Signature)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(challenge, None);
        assert_eq!(body["error"]["code"], "invalid-dpop-proof");
        assert_eq!(body["error"]["message"], "invalid DPoP proof");
    }

    #[tokio::test]
    async fn a_dpop_quota_refusal_is_429_with_retry_after() {
        let response = AuthnApiError(AuthnError::DpopQuotaExceeded { retry_after_secs: 9 }).into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "9");
        assert_eq!(response.headers()["paigasus-retryable"], "true");
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "dpop-quota-exceeded");
        assert_eq!(body["error"]["message"], "too many DPoP proofs");
    }
```

Also add `TokenDefect::NotKeyBound` to the defect list of `invalid_token_is_401_with_bearer_challenge`.

`adapters/retryable.rs`: in `tests_support::all_authn_errors`, add two entries to `all` (after `AuthnError::Unavailable,`):

```rust
            AuthnError::InvalidDpopProof(paigasus_iam_core::ProofDefect::Malformed),
            AuthnError::DpopQuotaExceeded { retry_after_secs: 1 },
```

and two arms to its exhaustiveness `match` (after `| AuthnError::Unavailable`):

```rust
                | AuthnError::InvalidDpopProof(_)
                | AuthnError::DpopQuotaExceeded { .. }
```

Replace the test `only_the_unavailable_authn_error_is_retryable` with:

```rust
    /// SMA-700 D10: a quota hit is a rate limit, so it is retryable like `Unavailable`. A bad proof
    /// is not: the same proof can never pass.
    #[test]
    fn only_unavailable_and_the_dpop_quota_are_retryable() {
        for err in all_authn_errors() {
            let want = match &err {
                AuthnError::Unavailable | AuthnError::DpopQuotaExceeded { .. } => Retryable::Yes,
                AuthnError::Backend(_) => Retryable::Unknown,
                _ => Retryable::No,
            };
            assert_eq!(authn_retryable(&err), want, "{err:?}");
        }
    }
```

- [ ] **Step 8: Run them to see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo nextest run --locked -p paigasus-iam --lib
```

Expected: compile errors `non-exhaustive patterns: &AuthnError::InvalidDpopProof(_) and &AuthnError::DpopQuotaExceeded { .. } not covered` in `convert.rs`, `http/authn.rs` and `retryable.rs`.

- [ ] **Step 9: Implement the arms**

`adapters/retryable.rs`, `authn_retryable`:

```rust
pub(crate) fn authn_retryable(err: &AuthnError) -> Retryable {
    match err {
        AuthnError::Unavailable | AuthnError::DpopQuotaExceeded { .. } => Retryable::Yes,
        AuthnError::Backend(_) => Retryable::Unknown,
        AuthnError::InvalidToken(_) | AuthnError::IdentityNotProvisioned | AuthnError::ProvisioningFailed(_) | AuthnError::PrincipalInactive | AuthnError::InvalidDpopProof(_) => Retryable::No,
    }
}
```

Update its doc comment: `` `Unavailable` names a transient dependency failure, and `DpopQuotaExceeded` a rate limit of one client (SMA-700 D10). ``

`adapters/http/authn.rs`, in `into_response`, add two arms before `AuthnError::Backend(_)`:

```rust
            // SMA-700. No IAM HTTP route produces these today (IAM's own API refuses the DPoP
            // scheme), but the funnel is exhaustive.
            AuthnError::InvalidDpopProof(_) => (StatusCode::UNAUTHORIZED, "invalid-dpop-proof", "invalid DPoP proof"),
            AuthnError::DpopQuotaExceeded { .. } => (StatusCode::TOO_MANY_REQUESTS, "dpop-quota-exceeded", "too many DPoP proofs"),
```

and after the `if matches!(self.0, AuthnError::InvalidToken(_)) { … }` block:

```rust
        if let AuthnError::DpopQuotaExceeded { retry_after_secs } = self.0 {
            response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(retry_after_secs.max(1)));
        }
```

`adapters/grpc/convert.rs`: add two statics after `AUTHN_INTERNAL` (`:54`):

```rust
static INVALID_DPOP_PROOF: LazyLock<String> = LazyLock::new(|| ErrorReason::InvalidDpopProof.as_wire_reason().expect("a declared reason is never the sentinel"));
static DPOP_QUOTA_EXCEEDED: LazyLock<String> = LazyLock::new(|| ErrorReason::DpopQuotaExceeded.as_wire_reason().expect("a declared reason is never the sentinel"));
```

Replace `iam_status` (`:77-82`) with two functions. The second stays the single construction point of `ErrorDetails`:

```rust
/// Builds a `Status` carrying `google.rpc.ErrorInfo` in the `grpc-status-details-bin` trailer.
/// Every IAM gRPC error goes through [`iam_status_with_retry`], so no site can forget the details.
pub fn iam_status(code: Code, reason: &str, message: impl Into<String>, retryable: Retryable, extra: &[(&str, &str)]) -> Status {
    iam_status_with_retry(code, reason, message, retryable, extra, None)
}

/// [`iam_status`], plus `google.rpc.RetryInfo` when `retry_after` is `Some` (SMA-700: the
/// `dpop-quota-exceeded` refusal). The single place that builds `ErrorDetails`.
fn iam_status_with_retry(code: Code, reason: &str, message: impl Into<String>, retryable: Retryable, extra: &[(&str, &str)], retry_after: Option<std::time::Duration>) -> Status {
    let mut details = ErrorDetails::with_error_info(reason, &*IAM_DOMAIN, error_metadata(retryable, extra));
    if let Some(delay) = retry_after {
        details.set_retry_info(Some(delay));
    }
    Status::with_error_details(code, message, details)
}
```

Update the module doc (lines 7-10): `` `iam_status_with_retry` (SMA-504, SMA-700) is the single construction point … it is the only place that builds `ErrorDetails` ``.

Replace `authn_status` (`:140-155`) with:

```rust
pub fn authn_status(err: &AuthnError) -> Status {
    let mut retry_after = None;
    let (code, reason, message) = match err {
        AuthnError::InvalidToken(_) => (Code::Unauthenticated, INVALID_TOKEN.as_str(), "invalid bearer token"),
        AuthnError::IdentityNotProvisioned => (Code::PermissionDenied, IDENTITY_NOT_PROVISIONED.as_str(), "identity not provisioned"),
        AuthnError::ProvisioningFailed(_) => (Code::PermissionDenied, PROVISIONING_FAILED.as_str(), "provisioning failed"),
        AuthnError::PrincipalInactive => (Code::PermissionDenied, PRINCIPAL_INACTIVE.as_str(), "principal inactive"),
        AuthnError::Unavailable => (Code::Unavailable, AUTHN_UNAVAILABLE.as_str(), "authentication backend unavailable"),
        // SMA-700 D9: every proof defect is one reason. The defect is never on the wire.
        AuthnError::InvalidDpopProof(_) => (Code::Unauthenticated, INVALID_DPOP_PROOF.as_str(), "invalid DPoP proof"),
        // SMA-700 D10: a rate limit of one client, not an outage. Never a zero delay.
        AuthnError::DpopQuotaExceeded { retry_after_secs } => {
            retry_after = Some(std::time::Duration::from_secs(u64::from((*retry_after_secs).max(1))));
            (Code::ResourceExhausted, DPOP_QUOTA_EXCEEDED.as_str(), "too many DPoP proofs")
        }
        AuthnError::Backend(_) => {
            // `Debug` carries the boxed repository/infra source (never token or claim
            // material, by `AuthnError`'s own contract) — logged here, never surfaced.
            tracing::error!(error = ?err, "internal error handling a gRPC authn request");
            (Code::Internal, AUTHN_INTERNAL.as_str(), "internal error")
        }
    };
    iam_status_with_retry(code, reason, message, authn_retryable(err), &[], retry_after)
}
```

Update the comment above the statics (`authn_status's six reasons`) to `authn_status's reasons`.

- [ ] **Step 10: Run the whole crate and the other users of the port**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo fmt
cargo build --locked --workspace --all-targets
cargo nextest run --locked -p paigasus-iam-core
cargo nextest run --locked -p paigasus-iam --lib
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test authn_private_ca
cargo clippy --locked -p paigasus-iam-core -p paigasus-iam --all-targets -- -D warnings
python3 ../ci/error-registry/check.py --single-site
```

Expected: all pass, no warning. (`authn_private_ca` needs no Docker; the variable is harmless there.)

- [ ] **Step 11: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git add rs/crates/libs/paigasus-iam-core rs/crates/services/paigasus-iam/src rs/crates/services/paigasus-iam/tests/authn_private_ca.rs
git commit -m "feat(rs): DPoP types and ports in iam-core, and the scheme on the authenticator port (SMA-700)

TokenScheme, Jkt, ProofDefect, the NotKeyBound token defect and two
AuthnError variants, the DpopProofChecker and ReplayStore ports. Every
caller passes the Bearer scheme. The gRPC and HTTP funnels map the two
new errors; the quota refusal carries RetryInfo.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Validator step 7 by scheme

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` (module doc `:3-16`; `RefusalDetail` `:61-71`; `log_refusal` `:141-171`; new fn after `sender_constraint_marker` `:361-369`; `authenticate` `:373`, `:435-457`; tests after `:998`)

**Interfaces:**
- Consumes: `TokenScheme`, `Jkt`, `TokenDefect::NotKeyBound`, `ValidatedClaims::key_binding` (Task 2).
- Produces: `OidcAuthenticator::authenticate(token, TokenScheme::Dpop)` returns `key_binding: Some(Jkt)` for a token with `cnf` an object holding a non-empty string `jkt` and no `x5t#S256`; else `Err(InvalidToken(NotKeyBound))`. `TokenScheme::Bearer` is unchanged (`SenderConstrained` for any marker, `key_binding: None`).

- [ ] **Step 1: Write the failing tests**

In the validator test module, after `sender_constraint_marker_names_the_marker` (`:998`), add:

```rust
    // ---- SMA-700: step 7 on the DPoP scheme ------------------------------------------------

    const JKT: &str = "0ZcOCORZNYy-DWpqq30jZyJGHTN0d2HglBV3uiguA4I";

    /// Signs `claims` with a fresh ES256 key and runs the full pipeline on the given scheme.
    async fn authenticate_json_as(claims: &serde_json::Value, scheme: TokenScheme) -> Result<ValidatedClaims, AuthnError> {
        let (encoding_key, jwk, kid) = es256_keypair();
        let token = sign(&encoding_key, Some(&kid), claims);
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);
        authenticator.authenticate(&token, scheme).await
    }

    #[tokio::test]
    async fn dpop_scheme_accepts_a_jkt_bound_token_and_returns_the_binding() {
        // Spec § 5.1 validator case 1: the Keycloak shape (typ DPoP plus cnf.jkt), and cnf.jkt alone.
        for extra in [serde_json::json!({ "typ": "DPoP", "cnf": { "jkt": JKT } }), serde_json::json!({ "cnf": { "jkt": JKT } })] {
            let validated = authenticate_json_as(&claims_with(extra.clone()), TokenScheme::Dpop)
                .await
                .unwrap_or_else(|err| panic!("{extra}: a jkt-bound token must pass the DPoP scheme, got {err:?}"));
            assert_eq!(validated.key_binding, Some(Jkt::new(JKT)), "{extra}");
            assert_eq!(validated.subject, "sub-1");
        }
    }

    #[tokio::test]
    async fn dpop_scheme_refuses_a_token_that_is_not_bound_to_exactly_one_key() {
        // Spec § 5.1 validator cases 2-5 (§ 4.3): cnf with no jkt, jkt plus x5t#S256, typ DPoP
        // only, no binding, and the shapes of a jkt that is not a non-empty string.
        let cases = [
            ("cnf with no jkt", serde_json::json!({ "cnf": { "jwk": { "kty": "EC" } } })),
            ("cnf empty object", serde_json::json!({ "cnf": {} })),
            ("jkt and x5t#S256", serde_json::json!({ "cnf": { "jkt": JKT, "x5t#S256": "abc" } })),
            ("x5t#S256 only", serde_json::json!({ "cnf": { "x5t#S256": "abc" } })),
            ("typ DPoP only", serde_json::json!({ "typ": "DPoP" })),
            ("no binding", serde_json::json!({})),
            ("cnf null", serde_json::json!({ "cnf": null })),
            ("cnf a string", serde_json::json!({ "cnf": JKT })),
            ("jkt empty", serde_json::json!({ "cnf": { "jkt": "" } })),
            ("jkt a number", serde_json::json!({ "cnf": { "jkt": 1 } })),
            ("jkt null", serde_json::json!({ "cnf": { "jkt": null } })),
        ];
        for (name, extra) in cases {
            let err = authenticate_json_as(&claims_with(extra), TokenScheme::Dpop).await.unwrap_err();
            assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotKeyBound)), "{name}: want NotKeyBound, got {err:?}");
        }
    }

    #[tokio::test]
    async fn bearer_scheme_still_refuses_a_bound_token_and_binds_nothing() {
        // D7: with DPoP on, the Bearer scheme does not change.
        let err = authenticate_json_as(&claims_with(serde_json::json!({ "typ": "DPoP", "cnf": { "jkt": JKT } })), TokenScheme::Bearer)
            .await
            .unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::SenderConstrained)), "got {err:?}");
        let plain = authenticate_json_as(&claims_with(serde_json::json!({})), TokenScheme::Bearer).await.expect("a plain token passes Bearer");
        assert_eq!(plain.key_binding, None);
    }

    #[tokio::test]
    async fn the_dpop_scheme_keeps_the_earlier_defects_first() {
        // Steps 1-6b do not change (§ 4.3): an ID token stays NotAnAccessToken, an expired bound
        // token stays Expired.
        let id = authenticate_json_as(&claims_with(serde_json::json!({ "typ": "ID", "cnf": { "jkt": JKT } })), TokenScheme::Dpop).await.unwrap_err();
        assert!(matches!(id, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)), "got {id:?}");
        let expired = claims_with(serde_json::json!({ "cnf": { "jkt": JKT }, "exp": Utc::now().timestamp() - 120 }));
        let err = authenticate_json_as(&expired, TokenScheme::Dpop).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Expired)), "got {err:?}");
    }

    #[tokio::test]
    async fn a_not_key_bound_refusal_logs_its_own_static_message_and_no_claim() {
        let (logs, _guard) = capture_logs();
        let err = authenticate_json_as(&claims_with(serde_json::json!({ "cnf": { "jkt": JKT, "x5t#S256": "cert-thumb" } })), TokenScheme::Dpop)
            .await
            .unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotKeyBound)));
        let text = logs.text();
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| line.contains("refused a DPoP request: the token is not bound to a key, or is also bound to a certificate"))
            .collect();
        assert_eq!(lines.len(), 1, "exactly one NotKeyBound line expected, got:\n{text}");
        assert!(lines[0].contains("INFO") && lines[0].contains(ISSUER), "{}", lines[0]);
        for secret in [JKT, "cert-thumb", "sub-1", "alice@example.com"] {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }
```

- [ ] **Step 2: Run them to see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo nextest run --locked -p paigasus-iam --lib -E 'test(dpop_scheme) | test(bearer_scheme_still) | test(the_dpop_scheme_keeps) | test(a_not_key_bound)'
```

Expected: `dpop_scheme_accepts…` fails with `SenderConstrained` (the parameter is ignored), `dpop_scheme_refuses…` fails on the first case, and the log test fails. `bearer_scheme_still…` and `the_dpop_scheme_keeps…` may already pass.

- [ ] **Step 3: Implement**

Add `Jkt` to the `use paigasus_iam_core::{…}` line (line 25).

In `RefusalDetail`, after `Claim(&'a str),` add:

```rust
    /// The DPoP scheme with a token that is not bound to exactly one key (SMA-700 § 4.3). Its own
    /// static message, with no marker.
    NotKeyBound,
```

In `log_refusal`, add an arm after `RefusalDetail::Claim(name) => { … }`:

```rust
            RefusalDetail::NotKeyBound => {
                tracing::info!(issuer = issuer.as_str(), suppressed, "refused a DPoP request: the token is not bound to a key, or is also bound to a certificate");
            }
```

and extend its doc: `` (SMA-686 D8, D11, D14, D15; SMA-690 D8; SMA-700 § 4.3): only `NotAnAccessToken`, `AudienceMismatch`, `SenderConstrained` and `NotKeyBound` ``.

After `sender_constraint_marker`, add:

```rust
/// The `cnf.jkt` of a signature-verified token on the DPoP scheme (SMA-700 § 4.3), or `None` when
/// the token is not bound to exactly one key: `cnf` must be a JSON object with a `jkt` member that
/// is a non-empty string, and no `x5t#S256` member (an mTLS binding stays refused, D4).
fn dpop_key_binding(claims: &WireClaims) -> Option<Jkt> {
    let cnf = claims.cnf.as_ref()?.as_object()?;
    if cnf.contains_key("x5t#S256") {
        return None;
    }
    let jkt = cnf.get("jkt")?.as_str()?;
    (!jkt.is_empty()).then(|| Jkt::new(jkt))
}
```

Rename the parameter `_scheme` → `scheme` in `authenticate`, and replace step 7 (the `if let Some(marker) = sender_constraint_marker(&claims) { … }` block) with:

```rust
        // 7. The key binding (SMA-690, SMA-700 § 4.3). Bearer: a bound token is refused, because
        //    IAM cannot check the binding on that scheme (D7). DPoP: the token must be bound to
        //    exactly one key, and the caller checks the proof against `key_binding`.
        let key_binding = match scheme {
            TokenScheme::Bearer => {
                if let Some(marker) = sender_constraint_marker(&claims) {
                    self.log_refusal(&issuer, TokenDefect::SenderConstrained, RefusalDetail::Binding(marker));
                    return Err(invalid(TokenDefect::SenderConstrained));
                }
                None
            }
            TokenScheme::Dpop => match dpop_key_binding(&claims) {
                Some(jkt) => Some(jkt),
                None => {
                    self.log_refusal(&issuer, TokenDefect::NotKeyBound, RefusalDetail::NotKeyBound);
                    return Err(invalid(TokenDefect::NotKeyBound));
                }
            },
        };
```

In the final `Ok(ValidatedClaims { … })`, replace `key_binding: None,` with `key_binding,`.

Update the module doc (`:3-16`): after "…because IAM cannot check the binding." add "On the DPoP scheme (SMA-700) step 7 instead requires a `cnf.jkt` binding and returns it; the proof check is the caller's job (`application::dpop`)."

- [ ] **Step 4: Run them to see them pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo fmt
cargo nextest run --locked -p paigasus-iam --lib -E 'test(validator)'
cargo clippy --locked -p paigasus-iam --all-targets -- -D warnings
```

Expected: every validator test passes, the SMA-690 `Bearer` tests included.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git add rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs
git commit -m "feat(rs): validator step 7 by scheme, the DPoP key binding (SMA-700)

The Bearer scheme keeps refusing a bound token. The DPoP scheme requires
cnf.jkt and no x5t#S256, and returns the binding. A refusal writes its own
rate-limited info line with the issuer only.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: `JoseDpopProofChecker` — checks 1-9

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/oidc/dpop.rs`
- Create: `rs/crates/services/paigasus-iam/src/adapters/oidc/testdata/dpop-rs256-test-key.pem`, `testdata/dpop-rs256-test-jwk.json` (generated in Step 1)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/mod.rs`

**Interfaces:**
- Consumes: `DpopProofChecker`, `ProofClaims`, `FollowUpClaims`, `Jkt`, `ProofDefect` (Task 2).
- Produces: `paigasus_iam::adapters::oidc::dpop::JoseDpopProofChecker` (unit struct, `Debug, Clone, Copy, Default`) implementing `DpopProofChecker`; `pub const MAX_PROOF_BYTES: usize = 8192;`.

- [ ] **Step 1: Generate the RS256 test-key fixtures (decision P1)**

The key signs only unit-test proofs. Generate it once and commit both files. Do not print the key.

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs/crates/services/paigasus-iam/src/adapters/oidc
mkdir -p testdata
openssl genrsa -traditional -out testdata/dpop-rs256-test-key.pem 2048
python3 - <<'EOF'
import base64, json, re, subprocess
text = subprocess.run(["openssl", "pkey", "-in", "testdata/dpop-rs256-test-key.pem", "-noout", "-text"], capture_output=True, text=True, check=True).stdout
hexs = re.search(r"modulus:\s*\n((?:\s+[0-9a-f:]+\n)+)", text).group(1)
n = bytes.fromhex(re.sub(r"[\s:]", "", hexs)).lstrip(b"\x00")
assert len(n) == 256, len(n)
assert "publicExponent: 65537" in text
b64 = lambda raw: base64.urlsafe_b64encode(raw).rstrip(b"=").decode()
with open("testdata/dpop-rs256-test-jwk.json", "w") as fh:
    json.dump({"kty": "RSA", "n": b64(n), "e": "AQAB"}, fh)
    fh.write("\n")
EOF
grep -c "BEGIN RSA PRIVATE KEY" testdata/dpop-rs256-test-key.pem
```

Expected: the last line prints `1`. (If the sandbox refuses the heredoc, put the lines into a scratchpad script and run it with `/bin/bash`.)

- [ ] **Step 2: Write the module with its tests first (the implementation bodies come in Step 4)**

Create `src/adapters/oidc/dpop.rs` with this test module at its end. Steps 3-4 add the code above it.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::EncodingKey;
    use p256::elliptic_curve::Generate;
    use p256::elliptic_curve::sec1::ToSec1Point;
    use p256::pkcs8::{EncodePrivateKey, LineEnding};
    use serde_json::json;

    const TOKEN: &str = "eyJhbGciOiJFUzI1NiJ9.eyJzdWIiOiJhbGljZSJ9.bound-token-signature";
    const HTU: &str = "https://gw.example.test/v1/chat/completions";

    struct EsKey {
        sign: EncodingKey,
        x: String,
        y: String,
    }

    fn es_key() -> EsKey {
        let secret = p256::SecretKey::generate();
        let pem = secret.to_pkcs8_pem(LineEnding::LF).expect("pkcs8 pem");
        let point = secret.public_key().to_sec1_point(false);
        EsKey {
            sign: EncodingKey::from_ec_pem(pem.as_bytes()).expect("ec pem"),
            x: URL_SAFE_NO_PAD.encode(point.x().expect("x")),
            y: URL_SAFE_NO_PAD.encode(point.y().expect("y")),
        }
    }

    fn es_jwk(key: &EsKey) -> Value {
        json!({ "kty": "EC", "crv": "P-256", "x": key.x, "y": key.y })
    }

    fn es_jkt(key: &EsKey) -> Jkt {
        Jkt::new(thumbprint(&PublicJwk::Ec { x: key.x.clone(), y: key.y.clone() }))
    }

    /// The committed RS256 test key (decision P1) and its public JWK.
    fn rs_key() -> (EncodingKey, Value) {
        let sign = EncodingKey::from_rsa_pem(include_bytes!("testdata/dpop-rs256-test-key.pem")).expect("rsa pem fixture");
        let jwk: Value = serde_json::from_str(include_str!("testdata/dpop-rs256-test-jwk.json")).expect("jwk fixture");
        (sign, jwk)
    }

    fn rs_jkt(jwk: &Value) -> Jkt {
        let (n, e) = (jwk["n"].as_str().unwrap().to_owned(), jwk["e"].as_str().unwrap().to_owned());
        Jkt::new(thumbprint(&PublicJwk::Rsa { n, e, n_bytes: Vec::new(), e_bytes: Vec::new() }))
    }

    fn header(alg: &str, jwk: Value) -> Value {
        json!({ "typ": "dpop+jwt", "alg": alg, "jwk": jwk })
    }

    fn payload() -> Value {
        json!({ "jti": "jti-1", "htm": "POST", "htu": HTU, "iat": 1_700_000_000i64, "ath": ath_of(TOKEN) })
    }

    fn b64_json(value: &str) -> String {
        URL_SAFE_NO_PAD.encode(value.as_bytes())
    }

    /// Signs the raw header and payload JSON text, so a test can write any member, also a repeated one.
    fn sign_raw(header_json: &str, payload_json: &str, key: &EncodingKey, alg: Algorithm) -> String {
        let message = format!("{}.{}", b64_json(header_json), b64_json(payload_json));
        let signature = jsonwebtoken::crypto::sign(message.as_bytes(), key, alg).expect("sign a test proof");
        format!("{message}.{signature}")
    }

    fn sign(header: &Value, payload: &Value, key: &EncodingKey, alg: Algorithm) -> String {
        sign_raw(&header.to_string(), &payload.to_string(), key, alg)
    }

    /// An unsigned proof with a fixed signature part, for the checks that run before check 6.
    fn unsigned(header: &Value, payload: &Value) -> String {
        format!("{}.{}.c2lnbmF0dXJl", b64_json(&header.to_string()), b64_json(&payload.to_string()))
    }

    fn es_proof(key: &EsKey, payload: &Value) -> String {
        sign(&header("ES256", es_jwk(key)), payload, &key.sign, Algorithm::ES256)
    }

    fn check(proof: &str, jkt: &Jkt) -> Result<ProofClaims, ProofDefect> {
        JoseDpopProofChecker.check(proof, TOKEN, jkt)
    }

    #[test]
    fn a_valid_es256_proof_passes_and_returns_its_claims() {
        let key = es_key();
        let claims = check(&es_proof(&key, &payload()), &es_jkt(&key)).expect("a valid ES256 proof");
        assert_eq!(
            claims,
            ProofClaims {
                jti: "jti-1".into(),
                iat: 1_700_000_000,
                htm: "POST".into(),
                htu: HTU.into()
            }
        );
    }

    #[test]
    fn a_valid_rs256_proof_passes() {
        let (sign_key, jwk) = rs_key();
        let proof = sign(&header("RS256", jwk.clone()), &payload(), &sign_key, Algorithm::RS256);
        check(&proof, &rs_jkt(&jwk)).expect("a valid RS256 proof");
    }

    #[test]
    fn check_1_an_oversized_proof_is_malformed() {
        let key = es_key();
        let big = json!({ "jti": "jti-1", "htm": "POST", "htu": HTU, "iat": 1_700_000_000i64, "ath": ath_of(TOKEN), "pad": "x".repeat(MAX_PROOF_BYTES) });
        let proof = es_proof(&key, &big);
        assert!(proof.len() > MAX_PROOF_BYTES);
        assert_eq!(check(&proof, &es_jkt(&key)), Err(ProofDefect::Malformed));
    }

    #[test]
    fn check_2_a_proof_that_is_not_three_base64url_json_objects_is_malformed() {
        let key = es_key();
        let good = es_proof(&key, &payload());
        let (head, rest) = good.split_once('.').unwrap();
        let cases = [
            ("two parts", format!("{head}.{}", rest.split_once('.').unwrap().0)),
            ("four parts", format!("{good}.AAAA")),
            ("padding", format!("{head}=.{rest}")),
            ("a character outside base64url", format!("{head}+.{rest}")),
            ("an empty part", format!(".{rest}")),
            ("a header that is not JSON", format!("{}.{rest}", b64_json("not json"))),
            ("a header that is a JSON array", format!("{}.{rest}", b64_json("[1,2]"))),
        ];
        for (name, proof) in cases {
            assert_eq!(check(&proof, &es_jkt(&key)), Err(ProofDefect::Malformed), "{name}");
        }
    }

    #[test]
    fn a_repeated_member_name_is_malformed() {
        // Review Focus 1, decision P10: the last value must not silently win.
        let key = es_key();
        let jwk = es_jwk(&key).to_string();
        let payload_json = payload().to_string();
        let twice_typ = format!(r#"{{"typ":"dpop+jwt","typ":"JWT","alg":"ES256","jwk":{jwk}}}"#);
        let twice_jti = format!(r#"{{"jti":"a","jti":"b","htm":"POST","htu":"{HTU}","iat":1700000000,"ath":"{}"}}"#, ath_of(TOKEN));
        let header_json = header("ES256", es_jwk(&key)).to_string();
        for (name, proof) in [
            ("typ twice in the header", sign_raw(&twice_typ, &payload_json, &key.sign, Algorithm::ES256)),
            ("jti twice in the payload", sign_raw(&header_json, &twice_jti, &key.sign, Algorithm::ES256)),
        ] {
            assert_eq!(check(&proof, &es_jkt(&key)), Err(ProofDefect::Malformed), "{name}");
        }
    }

    #[test]
    fn check_3_the_typ_must_be_dpop_jwt_in_either_form_and_any_case() {
        let key = es_key();
        for typ in ["dpop+jwt", "application/dpop+jwt", "application/DPoP+JWT", "DPOP+JWT"] {
            let proof = sign(&json!({ "typ": typ, "alg": "ES256", "jwk": es_jwk(&key) }), &payload(), &key.sign, Algorithm::ES256);
            check(&proof, &es_jkt(&key)).unwrap_or_else(|d| panic!("typ {typ:?} must pass, got {d:?}"));
        }
        for (name, hdr) in [
            ("no typ", json!({ "alg": "ES256", "jwk": es_jwk(&key) })),
            ("typ JWT", json!({ "typ": "JWT", "alg": "ES256", "jwk": es_jwk(&key) })),
            ("typ a number", json!({ "typ": 1, "alg": "ES256", "jwk": es_jwk(&key) })),
        ] {
            assert_eq!(check(&sign(&hdr, &payload(), &key.sign, Algorithm::ES256), &es_jkt(&key)), Err(ProofDefect::Typ), "{name}");
        }
    }

    #[test]
    fn check_4_only_es256_and_rs256_are_allowed() {
        let key = es_key();
        for alg in ["none", "HS256", "ES384", "PS256", "EdDSA", ""] {
            assert_eq!(check(&unsigned(&header(alg, es_jwk(&key)), &payload()), &es_jkt(&key)), Err(ProofDefect::Alg), "alg {alg:?}");
        }
        let no_alg = json!({ "typ": "dpop+jwt", "jwk": es_jwk(&key) });
        assert_eq!(check(&unsigned(&no_alg, &payload()), &es_jkt(&key)), Err(ProofDefect::Alg));
    }

    #[test]
    fn check_5_the_jwk_must_be_a_public_key_of_the_header_alg() {
        let key = es_key();
        let jkt = es_jkt(&key);
        let mut with_d = es_jwk(&key);
        with_d["d"] = json!("AAAA");
        let p384 = json!({ "kty": "EC", "crv": "P-384", "x": key.x, "y": key.y });
        let short_x = json!({ "kty": "EC", "crv": "P-256", "x": URL_SAFE_NO_PAD.encode([7u8; 31]), "y": key.y });
        let padded_x = json!({ "kty": "EC", "crv": "P-256", "x": format!("{}=", key.x), "y": key.y });
        let cases = [
            ("no jwk", json!({ "typ": "dpop+jwt", "alg": "ES256" })),
            ("jwk a string", header("ES256", json!("key"))),
            ("a private member d", header("ES256", with_d)),
            ("a symmetric member k", header("ES256", json!({ "kty": "oct", "k": "AAAA" }))),
            ("P-384 under ES256", header("ES256", p384)),
            ("x of 31 bytes", header("ES256", short_x)),
            ("x with padding", header("ES256", padded_x)),
            ("an EC key under RS256", header("RS256", es_jwk(&key))),
        ];
        for (name, hdr) in cases {
            assert_eq!(check(&unsigned(&hdr, &payload()), &jkt), Err(ProofDefect::Jwk), "{name}");
        }
    }

    #[test]
    fn check_5_rsa_bounds() {
        // Synthetic values: check 5 refuses them before any signature, so no key generation is needed.
        let n_bits = |bytes: usize, first: u8| {
            let mut n = vec![0xffu8; bytes];
            n[0] = first;
            URL_SAFE_NO_PAD.encode(n)
        };
        let jwk = |n: String, e: &[u8]| json!({ "kty": "RSA", "n": n, "e": URL_SAFE_NO_PAD.encode(e) });
        let cases = [
            ("n of 2047 bits", jwk(n_bits(256, 0x7f), &[1, 0, 1])),
            ("n of 4097 bits", jwk(n_bits(513, 0x01), &[1, 0, 1])),
            ("n with a leading zero byte", jwk(n_bits(257, 0x00), &[1, 0, 1])),
            ("e even (65536)", jwk(n_bits(256, 0xc0), &[1, 0, 0])),
            ("e of 1", jwk(n_bits(256, 0xc0), &[1])),
            ("e of 2^33 + 1", jwk(n_bits(256, 0xc0), &[2, 0, 0, 0, 1])),
            ("e with a leading zero byte (P12)", jwk(n_bits(256, 0xc0), &[0, 1, 0, 1])),
            ("kty EC under RS256", json!({ "kty": "EC", "n": n_bits(256, 0xc0), "e": "AQAB" })),
        ];
        for (name, key) in cases {
            assert_eq!(check(&unsigned(&header("RS256", key), &payload()), &Jkt::new("x")), Err(ProofDefect::Jwk), "{name}");
        }
        // The bounds themselves pass check 5 (then fail the signature, which is not under test here).
        for (name, key) in [("n of 2048 bits", jwk(n_bits(256, 0x80), &[1, 0, 1])), ("n of 4096 bits, e 3", jwk(n_bits(512, 0x80), &[3]))] {
            assert_eq!(check(&unsigned(&header("RS256", key), &payload()), &Jkt::new("x")), Err(ProofDefect::Signature), "{name}");
        }
    }

    #[test]
    fn check_6_the_signature_must_verify_with_the_header_jwk() {
        let key = es_key();
        let other = es_key();
        // Signed by another key, but the header names `key`.
        let forged = sign(&header("ES256", es_jwk(&key)), &payload(), &other.sign, Algorithm::ES256);
        assert_eq!(check(&forged, &es_jkt(&key)), Err(ProofDefect::Signature));
        // A changed payload under the old signature.
        let good = es_proof(&key, &payload());
        let mut parts: Vec<&str> = good.split('.').collect();
        let changed = b64_json(&json!({ "jti": "jti-2", "htm": "POST", "htu": HTU, "iat": 1_700_000_000i64, "ath": ath_of(TOKEN) }).to_string());
        parts[1] = &changed;
        assert_eq!(check(&parts.join("."), &es_jkt(&key)), Err(ProofDefect::Signature));
    }

    #[test]
    fn check_7_each_claim_must_be_present_and_of_its_type() {
        let key = es_key();
        let base = payload();
        let with = |name: &str, value: Value| {
            let mut p = base.clone();
            p[name] = value;
            p
        };
        let without = |name: &str| {
            let mut p = base.clone();
            p.as_object_mut().unwrap().remove(name);
            p
        };
        let cases = [
            ("no jti", without("jti")),
            ("jti empty", with("jti", json!(""))),
            ("jti of 257 bytes", with("jti", json!("j".repeat(257)))),
            ("jti a number", with("jti", json!(1))),
            ("no htm", without("htm")),
            ("htu a number", with("htu", json!(1))),
            ("no iat", without("iat")),
            ("iat fractional", with("iat", json!(1_700_000_000.5))),
            ("iat a string", with("iat", json!("1700000000"))),
            ("iat above i64", with("iat", json!(u64::MAX))),
            ("no ath", without("ath")),
        ];
        for (name, p) in cases {
            assert_eq!(check(&es_proof(&key, &p), &es_jkt(&key)), Err(ProofDefect::Malformed), "{name}");
        }
        // The edge: a 256-byte jti passes.
        check(&es_proof(&key, &with("jti", json!("j".repeat(256)))), &es_jkt(&key)).expect("a 256-byte jti passes");
    }

    #[test]
    fn check_8_ath_must_hash_this_token() {
        let key = es_key();
        let mut p = payload();
        p["ath"] = json!(ath_of("another.bound.token"));
        assert_eq!(check(&es_proof(&key, &p), &es_jkt(&key)), Err(ProofDefect::Ath));
    }

    #[test]
    fn check_9_the_thumbprint_must_equal_the_token_jkt() {
        let key = es_key();
        let other = es_key();
        assert_eq!(check(&es_proof(&key, &payload()), &es_jkt(&other)), Err(ProofDefect::Thumbprint));
    }

    #[test]
    fn a_nonce_claim_is_accepted_and_ignored() {
        let key = es_key();
        let mut p = payload();
        p["nonce"] = json!("server-nonce");
        check(&es_proof(&key, &p), &es_jkt(&key)).expect("a nonce claim is ignored");
    }

    #[test]
    fn the_rfc_7638_example_key_gives_the_published_thumbprint() {
        // RFC 7638 § 3.1.
        let n = "0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw";
        let key = PublicJwk::Rsa {
            n: n.to_owned(),
            e: "AQAB".to_owned(),
            n_bytes: Vec::new(),
            e_bytes: Vec::new(),
        };
        assert_eq!(thumbprint(&key), "NzbLsXh8uDCcd-6MNwXF4W_7noWXFZAfHkxZsRGC9Xs");
    }

    #[test]
    fn follow_up_claims_reads_jti_and_ath_with_no_signature_check() {
        let key = es_key();
        let good = es_proof(&key, &payload());
        let (signed, _) = good.rsplit_once('.').unwrap();
        let resigned = format!("{signed}.Z2FyYmFnZQ");
        assert_eq!(
            JoseDpopProofChecker.follow_up_claims(&resigned),
            Ok(FollowUpClaims {
                jti: "jti-1".into(),
                ath: ath_of(TOKEN)
            })
        );
        assert_eq!(JoseDpopProofChecker.follow_up_claims(&"a".repeat(MAX_PROOF_BYTES + 1)), Err(ProofDefect::Malformed), "check 1");
        assert_eq!(JoseDpopProofChecker.follow_up_claims("only.two"), Err(ProofDefect::Malformed), "check 2");
        let mut no_ath = payload();
        no_ath.as_object_mut().unwrap().remove("ath");
        assert_eq!(JoseDpopProofChecker.follow_up_claims(&es_proof(&key, &no_ath)), Err(ProofDefect::Malformed), "check 7, ath");
    }

    #[test]
    fn ath_matches_compares_with_the_token_hash() {
        assert!(JoseDpopProofChecker.ath_matches(&ath_of(TOKEN), TOKEN));
        assert!(!JoseDpopProofChecker.ath_matches(&ath_of(TOKEN), "other.token.x"));
    }
}
```

In `src/adapters/oidc/mod.rs`, add `pub mod dpop;` after `pub mod jwks;`, and add to the module doc: "the DPoP proof checker (`JoseDpopProofChecker`, SMA-700) lives in `dpop`."

- [ ] **Step 3: Run the tests to see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo nextest run --locked -p paigasus-iam --lib -E 'test(adapters::oidc::dpop)'
```

Expected: compile errors for `JoseDpopProofChecker`, `PublicJwk`, `thumbprint`, `ath_of`, `MAX_PROOF_BYTES`.

- [ ] **Step 4: Implement the checker above the test module**

```rust
// SPDX-License-Identifier: Apache-2.0

//! `JoseDpopProofChecker` (SMA-700 D2, § 4.4): the stateless checks 1-9 of a DPoP proof
//! (RFC 9449 § 4.3) behind the core `DpopProofChecker` port. The proof is split and decoded by
//! hand, not with `jsonwebtoken::decode_header`: that call fails on `alg: none` before check 4 can
//! name it, and `jsonwebtoken`'s `Jwk` type drops private members that check 5 must see. The key is
//! built from the members that check 5 validated, and `jsonwebtoken::crypto::verify` checks the
//! raw signature, with no `Validation` flags. The request checks (`htm`, `htu`, `iat`, replay) are
//! in `application::dpop`. Nothing here logs.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, DecodingKey};
use paigasus_iam_core::{DpopProofChecker, FollowUpClaims, Jkt, ProofClaims, ProofDefect};
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// Check 1: the largest proof IAM reads (SMA-700 § 4.1).
pub const MAX_PROOF_BYTES: usize = 8192;
/// Check 3 (D16): compared ASCII case-insensitively (RFC 7515 § 4.1.9).
const PROOF_TYPES: [&str; 2] = ["dpop+jwt", "application/dpop+jwt"];
/// Check 5: a public key has none of these members.
const PRIVATE_JWK_MEMBERS: [&str; 8] = ["d", "p", "q", "dp", "dq", "qi", "oth", "k"];
/// Check 7: `jti` is 1 to 256 bytes.
const MAX_JTI_BYTES: usize = 256;
/// Check 5: the RSA modulus size.
const RSA_BITS: std::ops::RangeInclusive<usize> = 2048..=4096;
/// Check 5: the largest RSA public exponent that the `rsa` 0.9 crate accepts.
const MAX_RSA_EXPONENT: u64 = (1 << 33) - 1;

/// The `DpopProofChecker` v1 implementation. Stateless.
#[derive(Debug, Clone, Copy, Default)]
pub struct JoseDpopProofChecker;

/// The three parts of a proof after checks 1 and 2.
struct RawProof<'a> {
    /// `header.payload`, the bytes the signature covers.
    signing_input: &'a str,
    signature: &'a str,
    header: Map<String, Value>,
    payload: Map<String, Value>,
}

/// The public key of a proof, from the `jwk` members that check 5 validated. The string members
/// are strict base64url, so the RFC 7638 canonical JSON needs no escapes.
enum PublicJwk {
    Ec { x: String, y: String },
    Rsa { n: String, e: String, n_bytes: Vec<u8>, e_bytes: Vec<u8> },
}

/// The claims of check 7.
struct WireProofClaims {
    jti: String,
    iat: i64,
    htm: String,
    htu: String,
    ath: String,
}

impl DpopProofChecker for JoseDpopProofChecker {
    fn check(&self, proof: &str, token: &str, jkt: &Jkt) -> Result<ProofClaims, ProofDefect> {
        let raw = parse(proof)?; // checks 1 and 2
        check_typ(&raw.header)?; // check 3
        let alg = check_alg(&raw.header)?; // check 4
        let key = check_jwk(&raw.header, alg)?; // check 5
        verify_signature(&raw, &key, alg)?; // check 6
        let claims = read_claims(&raw.payload)?; // check 7
        if claims.ath != ath_of(token) {
            return Err(ProofDefect::Ath); // check 8
        }
        if thumbprint(&key) != jkt.as_str() {
            return Err(ProofDefect::Thumbprint); // check 9
        }
        Ok(ProofClaims {
            jti: claims.jti,
            iat: claims.iat,
            htm: claims.htm,
            htu: claims.htu,
        })
    }

    fn follow_up_claims(&self, proof: &str) -> Result<FollowUpClaims, ProofDefect> {
        let raw = parse(proof)?;
        Ok(FollowUpClaims {
            jti: read_jti(&raw.payload)?,
            ath: read_string(&raw.payload, "ath")?,
        })
    }

    fn ath_matches(&self, ath: &str, token: &str) -> bool {
        ath == ath_of(token)
    }
}

/// Checks 1 and 2: at most `MAX_PROOF_BYTES`; three non-empty base64url parts with no padding;
/// the header and the payload are JSON objects with unique member names (decision P10).
fn parse(proof: &str) -> Result<RawProof<'_>, ProofDefect> {
    if proof.len() > MAX_PROOF_BYTES {
        return Err(ProofDefect::Malformed);
    }
    let (signing_input, signature) = proof.rsplit_once('.').ok_or(ProofDefect::Malformed)?;
    let (header, payload) = signing_input.split_once('.').ok_or(ProofDefect::Malformed)?;
    // `is_base64url` refuses `.`, so a fourth part fails here, in `payload`.
    if [header, payload, signature].iter().any(|part| part.is_empty() || !part.bytes().all(is_base64url)) {
        return Err(ProofDefect::Malformed);
    }
    Ok(RawProof {
        signing_input,
        signature,
        header: decode_object(header)?,
        payload: decode_object(payload)?,
    })
}

fn is_base64url(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

fn decode_object(part: &str) -> Result<Map<String, Value>, ProofDefect> {
    let bytes = URL_SAFE_NO_PAD.decode(part).map_err(|_| ProofDefect::Malformed)?;
    let UniqueObject(members) = serde_json::from_slice(&bytes).map_err(|_| ProofDefect::Malformed)?;
    Ok(members)
}

/// A JSON object that refuses a repeated top-level member name. A plain `serde_json::Map` keeps
/// the last of two equal names with no error (decision P10; the validator's `StrictPayload` has
/// the same rule for the access token).
struct UniqueObject(Map<String, Value>);

impl<'de> Deserialize<'de> for UniqueObject {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueMembers;

        impl<'de> serde::de::Visitor<'de> for UniqueMembers {
            type Value = Map<String, Value>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object with unique member names")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut members = Map::new();
                while let Some(name) = access.next_key::<String>()? {
                    // The error text names no member: the proof is client material.
                    if members.contains_key(&name) {
                        return Err(serde::de::Error::custom("a member name occurs twice"));
                    }
                    let value = access.next_value::<Value>()?;
                    members.insert(name, value);
                }
                Ok(members)
            }
        }

        deserializer.deserialize_map(UniqueMembers).map(UniqueObject)
    }
}

/// Check 3 (D16).
fn check_typ(header: &Map<String, Value>) -> Result<(), ProofDefect> {
    match header.get("typ").and_then(Value::as_str) {
        Some(typ) if PROOF_TYPES.iter().any(|allowed| typ.eq_ignore_ascii_case(allowed)) => Ok(()),
        _ => Err(ProofDefect::Typ),
    }
}

/// Check 4 (D8): the exact string `ES256` or `RS256`. This refuses `none` and every HMAC alg.
fn check_alg(header: &Map<String, Value>) -> Result<Algorithm, ProofDefect> {
    match header.get("alg").and_then(Value::as_str) {
        Some("ES256") => Ok(Algorithm::ES256),
        Some("RS256") => Ok(Algorithm::RS256),
        _ => Err(ProofDefect::Alg),
    }
}

/// Check 5: a public JWK of the header alg's family, with valid members (D17: `alg`, `use` and
/// `key_ops` are ignored).
fn check_jwk(header: &Map<String, Value>, alg: Algorithm) -> Result<PublicJwk, ProofDefect> {
    let jwk = header.get("jwk").and_then(Value::as_object).ok_or(ProofDefect::Jwk)?;
    if PRIVATE_JWK_MEMBERS.iter().any(|member| jwk.contains_key(*member)) {
        return Err(ProofDefect::Jwk);
    }
    let member = |name: &str| jwk.get(name).and_then(Value::as_str).ok_or(ProofDefect::Jwk);
    match alg {
        Algorithm::ES256 => {
            // `DecodingKey::from_ec_components` ignores `crv`, so this check is necessary.
            if member("kty")? != "EC" || member("crv")? != "P-256" {
                return Err(ProofDefect::Jwk);
            }
            let (x, y) = (member("x")?, member("y")?);
            if strict_b64(x)?.len() != 32 || strict_b64(y)?.len() != 32 {
                return Err(ProofDefect::Jwk);
            }
            Ok(PublicJwk::Ec { x: x.to_owned(), y: y.to_owned() })
        }
        Algorithm::RS256 => {
            if member("kty")? != "RSA" {
                return Err(ProofDefect::Jwk);
            }
            let (n, e) = (member("n")?, member("e")?);
            let (n_bytes, e_bytes) = (strict_b64(n)?, strict_b64(e)?);
            let first = *n_bytes.first().ok_or(ProofDefect::Jwk)?;
            if first == 0 {
                return Err(ProofDefect::Jwk);
            }
            let bits = n_bytes.len() * 8 - first.leading_zeros() as usize;
            if !RSA_BITS.contains(&bits) {
                return Err(ProofDefect::Jwk);
            }
            let exponent = rsa_exponent(&e_bytes).ok_or(ProofDefect::Jwk)?;
            if exponent < 3 || exponent.is_multiple_of(2) || exponent > MAX_RSA_EXPONENT {
                return Err(ProofDefect::Jwk);
            }
            Ok(PublicJwk::Rsa {
                n: n.to_owned(),
                e: e.to_owned(),
                n_bytes,
                e_bytes,
            })
        }
        _ => Err(ProofDefect::Alg),
    }
}

/// Strict base64url: no padding, no other alphabet, canonical trailing bits (`URL_SAFE_NO_PAD`
/// refuses all three).
fn strict_b64(value: &str) -> Result<Vec<u8>, ProofDefect> {
    URL_SAFE_NO_PAD.decode(value).map_err(|_| ProofDefect::Jwk)
}

/// The RSA public exponent as an integer: minimal octets (no leading zero, decision P12), at most
/// five bytes. `None` when the bytes break either rule.
fn rsa_exponent(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() || bytes.len() > 5 || bytes[0] == 0 {
        return None;
    }
    Some(bytes.iter().fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte)))
}

/// Check 6: the signature over `header.payload`, with the key from the validated members.
fn verify_signature(raw: &RawProof<'_>, key: &PublicJwk, alg: Algorithm) -> Result<(), ProofDefect> {
    let decoding_key = match key {
        PublicJwk::Ec { x, y } => DecodingKey::from_ec_components(x, y).map_err(|_| ProofDefect::Signature)?,
        PublicJwk::Rsa { n_bytes, e_bytes, .. } => DecodingKey::from_rsa_raw_components(n_bytes, e_bytes),
    };
    match jsonwebtoken::crypto::verify(raw.signature, raw.signing_input.as_bytes(), &decoding_key, alg) {
        Ok(true) => Ok(()),
        _ => Err(ProofDefect::Signature),
    }
}

/// Check 7. A missing or wrong-typed claim, and a fractional or out-of-range `iat`, are `Malformed`.
fn read_claims(payload: &Map<String, Value>) -> Result<WireProofClaims, ProofDefect> {
    Ok(WireProofClaims {
        jti: read_jti(payload)?,
        iat: payload.get("iat").and_then(Value::as_i64).ok_or(ProofDefect::Malformed)?,
        htm: read_string(payload, "htm")?,
        htu: read_string(payload, "htu")?,
        ath: read_string(payload, "ath")?,
    })
}

fn read_jti(payload: &Map<String, Value>) -> Result<String, ProofDefect> {
    let jti = read_string(payload, "jti")?;
    if jti.is_empty() || jti.len() > MAX_JTI_BYTES {
        return Err(ProofDefect::Malformed);
    }
    Ok(jti)
}

fn read_string(payload: &Map<String, Value>, name: &str) -> Result<String, ProofDefect> {
    payload.get(name).and_then(Value::as_str).map(str::to_owned).ok_or(ProofDefect::Malformed)
}

/// Check 8: base64url, with no padding, of SHA-256 over the token's ASCII bytes.
fn ath_of(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

/// Check 9: the RFC 7638 thumbprint, built by hand from the validated members, in the required
/// member order, with no whitespace. SHA-256, then base64url with no padding.
fn thumbprint(key: &PublicJwk) -> String {
    let canonical = match key {
        PublicJwk::Ec { x, y } => format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#),
        PublicJwk::Rsa { n, e, .. } => format!(r#"{{"e":"{e}","kty":"RSA","n":"{n}"}}"#),
    };
    URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes()))
}
```

- [ ] **Step 5: Run the tests to see them pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo fmt
cargo nextest run --locked -p paigasus-iam --lib -E 'test(adapters::oidc::dpop)'
cargo clippy --locked -p paigasus-iam --all-targets -- -D warnings
```

Expected: every test in `adapters::oidc::dpop::tests` passes; clippy is clean. If `EncodingKey::from_rsa_pem` refuses the fixture, the file is not PKCS#1: re-run Step 1 (the `-traditional` flag writes `BEGIN RSA PRIVATE KEY`). If `Sha256::digest(..)` does not satisfy `Engine::encode`'s `AsRef<[u8]>` bound, pass `Sha256::digest(..).as_slice()`.

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git add rs/crates/services/paigasus-iam/src/adapters/oidc/dpop.rs rs/crates/services/paigasus-iam/src/adapters/oidc/mod.rs rs/crates/services/paigasus-iam/src/adapters/oidc/testdata
git commit -m "feat(rs): JOSE DPoP proof checker, checks 1 to 9 (SMA-700)

A hand parse of the three parts with unique member names, the typ and alg
allow-lists, a public-key-only jwk with the EC and RSA bounds, the raw
signature check, the claim types, ath and the RFC 7638 thumbprint. The
RS256 test key is a committed fixture.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: `InMemoryReplayStore` and the R8 memory measurement

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/dpop_replay.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/mod.rs` (add `pub mod dpop_replay;` after `pub mod clock;`)
- Create: `rs/crates/services/paigasus-iam/tests/dpop_replay_memory.rs`

**Interfaces:**
- Consumes: `ReplayStore`, `NewProof`, `ProofKey`, `RecordOutcome`, `RedeemOutcome` (Task 2).
- Produces: `paigasus_iam::adapters::dpop_replay::InMemoryReplayStore` with `pub fn new(capacity: usize, per_key_quota: usize, per_subject_quota: usize) -> Self`, implementing `ReplayStore`.

- [ ] **Step 1: Write the store tests first**

Create `src/adapters/dpop_replay.rs` with this test module at the end (Step 3 adds the code above it):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A proof with one-byte ids, so each test names its keys, `jkt`s and subjects plainly.
    fn proof(key: u8, jkt: u8, subject: u8, expires_at: i64, follow_up_deadline: i64) -> NewProof {
        NewProof {
            key: ProofKey([key; 16]),
            subject: [subject; 16],
            jkt: [jkt; 16],
            expires_at,
            follow_up_deadline,
            follow_up_digest: [key; 32],
        }
    }

    fn roomy() -> InMemoryReplayStore {
        InMemoryReplayStore::new(100, 100, 100)
    }

    #[test]
    fn a_new_proof_is_fresh_and_the_same_key_again_is_a_replay() {
        let store = roomy();
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 101), RecordOutcome::Replayed);
        assert_eq!(store.record(proof(2, 1, 1, 160, 160), 101), RecordOutcome::Fresh, "another key is not a replay");
    }

    #[test]
    fn an_entry_is_live_at_its_deadline_and_gone_after_the_later_deadline() {
        let store = roomy();
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 160), RecordOutcome::Replayed, "live at now == expires_at");
        assert_eq!(store.record(proof(1, 1, 1, 220, 220), 161), RecordOutcome::Fresh, "removed after the later deadline");
        // The later of the two deadlines decides: here the ticket deadline (190) outlives expires_at (160).
        let store = roomy();
        assert_eq!(store.record(proof(2, 1, 1, 160, 190), 160), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(2, 1, 1, 160, 190), 190), RecordOutcome::Replayed, "kept until the ticket deadline");
        assert_eq!(store.record(proof(2, 1, 1, 260, 260), 191), RecordOutcome::Fresh);
    }

    #[test]
    fn the_follow_up_has_30_s_when_recorded_at_expires_at() {
        // § 4.5: follow_up_deadline = max(expires_at, now + 30). Recorded at now == expires_at == 160.
        let store = roomy();
        assert_eq!(store.record(proof(1, 1, 1, 160, 190), 160), RecordOutcome::Fresh);
        assert_eq!(store.redeem_follow_up(ProofKey([1; 16]), [1; 32], 190), RedeemOutcome::Redeemed);
        let store = roomy();
        assert_eq!(store.record(proof(1, 1, 1, 160, 190), 160), RecordOutcome::Fresh);
        assert_eq!(store.redeem_follow_up(ProofKey([1; 16]), [1; 32], 191), RedeemOutcome::Refused, "after the deadline");
    }

    #[test]
    fn the_key_quota_reports_the_time_until_its_oldest_entry_goes() {
        let store = InMemoryReplayStore::new(100, 2, 100);
        assert_eq!(store.record(proof(1, 7, 1, 150, 150), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(2, 7, 2, 140, 140), 100), RecordOutcome::Fresh);
        // The oldest entry of jkt 7 goes at the first call with now > 140, so 141 - 100 = 41.
        assert_eq!(store.record(proof(3, 7, 3, 160, 160), 100), RecordOutcome::QuotaExceeded { retry_after_secs: 41 });
        assert_eq!(store.record(proof(3, 8, 3, 160, 160), 100), RecordOutcome::Fresh, "another jkt is not limited");
    }

    #[test]
    fn the_subject_quota_reports_the_time_until_its_oldest_entry_goes() {
        let store = InMemoryReplayStore::new(100, 100, 2);
        assert_eq!(store.record(proof(1, 1, 9, 130, 130), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(2, 2, 9, 170, 170), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(3, 3, 9, 160, 160), 100), RecordOutcome::QuotaExceeded { retry_after_secs: 31 });
    }

    #[test]
    fn a_full_capacity_is_reported_with_the_entry_count() {
        let store = InMemoryReplayStore::new(2, 100, 100);
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(2, 2, 2, 160, 160), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(3, 3, 3, 160, 160), 100), RecordOutcome::CapacityFull { entries: 2 });
    }

    #[test]
    fn a_quota_wins_over_a_full_capacity() {
        // Decision P4: the client over its own quota gets a 429, not an outage answer.
        let store = InMemoryReplayStore::new(1, 1, 100);
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(2, 1, 2, 160, 160), 100), RecordOutcome::QuotaExceeded { retry_after_secs: 61 });
    }

    #[test]
    fn expired_heads_are_removed_before_a_limit_is_reported() {
        let store = InMemoryReplayStore::new(1, 1, 1);
        assert_eq!(store.record(proof(1, 1, 1, 110, 110), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(2, 1, 1, 170, 170), 111), RecordOutcome::Fresh, "the entry that went at 111 frees the key, the subject and the capacity");
    }

    #[test]
    fn counts_go_to_zero_and_are_removed() {
        let store = roomy();
        assert_eq!(store.record(proof(1, 1, 1, 110, 110), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(2, 1, 2, 120, 120), 100), RecordOutcome::Fresh);
        assert_eq!(store.record(proof(3, 3, 3, 500, 500), 121), RecordOutcome::Fresh, "purges keys 1 and 2");
        let state = store.lock();
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.by_removal.len(), 1, "no empty index bucket is kept");
        assert!(!state.keys.contains_key(&[1; 16]), "a jkt count at zero is removed");
        assert!(!state.subjects.contains_key(&[1; 16]) && !state.subjects.contains_key(&[2; 16]), "subject counts at zero are removed");
        assert_eq!(state.keys.len(), 1);
        assert_eq!(state.subjects.len(), 1);
    }

    #[test]
    fn a_follow_up_is_redeemed_once_and_only_with_the_same_digest() {
        let store = roomy();
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 100), RecordOutcome::Fresh);
        assert_eq!(store.redeem_follow_up(ProofKey([1; 16]), [9; 32], 101), RedeemOutcome::Refused, "another digest");
        assert_eq!(store.redeem_follow_up(ProofKey([1; 16]), [1; 32], 101), RedeemOutcome::Redeemed, "a digest mismatch did not use the ticket");
        assert_eq!(store.redeem_follow_up(ProofKey([1; 16]), [1; 32], 102), RedeemOutcome::Refused, "a ticket is used once");
        assert_eq!(store.redeem_follow_up(ProofKey([2; 16]), [2; 32], 102), RedeemOutcome::Refused, "no ticket for an unknown key");
        // A redeemed entry still blocks a replay of the proof.
        assert_eq!(store.record(proof(1, 1, 1, 160, 160), 103), RecordOutcome::Replayed);
    }
}
```

- [ ] **Step 2: Run them to see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo nextest run --locked -p paigasus-iam --lib -E 'test(dpop_replay)'
```

Expected: compile error `cannot find type InMemoryReplayStore`.

- [ ] **Step 3: Implement the store above the test module**

```rust
// SPDX-License-Identifier: Apache-2.0

//! The in-memory DPoP replay store (SMA-700 § 4.5, D3). `AppState` holds ONE instance behind one
//! `Arc`, shared by `Introspect` and the `IsAuthorized` follow-up. It is correct only with one IAM
//! replica; the chart pins IAM to one (R2). A restart clears it (R1).
//!
//! Every call first removes the expired heads of the removal index, so no call scans the whole
//! map. An entry is removed at the first call with `now > max(expires_at, follow_up_deadline)`.
//! Time comes only from the `now` argument (the `Clock` port). One `Mutex` protects the state;
//! it is never held across an `.await` (the port is synchronous).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard, PoisonError};

use paigasus_iam_core::{NewProof, ProofKey, RecordOutcome, RedeemOutcome, ReplayStore};

/// One recorded proof. The `subject` and `jkt` hashes are kept to release the counts.
struct Entry {
    subject: [u8; 16],
    jkt: [u8; 16],
    remove_at: i64,
    follow_up_deadline: i64,
    follow_up_digest: [u8; 32],
    redeemed: bool,
}

/// The live entries of one `jkt` hash or one subject hash: the count, and the removal seconds as
/// a multiset. The first key is the oldest removal, which a quota refusal reports.
#[derive(Default)]
struct Owner {
    count: usize,
    removals: BTreeMap<i64, u32>,
}

impl Owner {
    fn add(&mut self, remove_at: i64) {
        self.count += 1;
        *self.removals.entry(remove_at).or_insert(0) += 1;
    }

    /// Removes one entry. `true` when the owner has no entry left.
    fn release(&mut self, remove_at: i64) -> bool {
        self.count = self.count.saturating_sub(1);
        if let Some(n) = self.removals.get_mut(&remove_at) {
            *n -= 1;
            if *n == 0 {
                self.removals.remove(&remove_at);
            }
        }
        self.count == 0
    }
}

#[derive(Default)]
struct State {
    entries: HashMap<ProofKey, Entry>,
    /// The keys by removal second (the expiry index).
    by_removal: BTreeMap<i64, Vec<ProofKey>>,
    keys: HashMap<[u8; 16], Owner>,
    subjects: HashMap<[u8; 16], Owner>,
}

impl State {
    /// Removes every entry whose removal second is before `now`. Reads only the expired heads.
    fn purge(&mut self, now: i64) {
        while let Some(head) = self.by_removal.first_entry() {
            if *head.key() >= now {
                break;
            }
            for key in head.remove() {
                if let Some(entry) = self.entries.remove(&key) {
                    release(&mut self.keys, entry.jkt, entry.remove_at);
                    release(&mut self.subjects, entry.subject, entry.remove_at);
                }
            }
        }
    }
}

fn release(owners: &mut HashMap<[u8; 16], Owner>, id: [u8; 16], remove_at: i64) {
    if let Some(owner) = owners.get_mut(&id)
        && owner.release(remove_at)
    {
        owners.remove(&id);
    }
}

/// `Some(retry_after_secs)` when `id` already has `quota` live entries. The oldest of them goes at
/// the first call with `now > remove_at`, so the wait is `remove_at + 1 - now`, at least 1.
fn quota_hit(owners: &HashMap<[u8; 16], Owner>, id: [u8; 16], quota: usize, now: i64) -> Option<u32> {
    let owner = owners.get(&id)?;
    if owner.count < quota {
        return None;
    }
    let oldest = owner.removals.keys().next().copied().unwrap_or(now);
    let wait = oldest.saturating_add(1).saturating_sub(now).max(1);
    Some(u32::try_from(wait).unwrap_or(u32::MAX))
}

/// The `ReplayStore` v1 implementation (SMA-700 § 4.5).
pub struct InMemoryReplayStore {
    state: Mutex<State>,
    capacity: usize,
    per_key_quota: usize,
    per_subject_quota: usize,
}

impl InMemoryReplayStore {
    /// `IamConfig::validate` has refused a zero and a quota above the capacity.
    #[must_use]
    pub fn new(capacity: usize, per_key_quota: usize, per_subject_quota: usize) -> Self {
        InMemoryReplayStore {
            state: Mutex::new(State::default()),
            capacity,
            per_key_quota,
            per_subject_quota,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ReplayStore for InMemoryReplayStore {
    fn record(&self, entry: NewProof, now: i64) -> RecordOutcome {
        let mut state = self.lock();
        state.purge(now);
        if state.entries.contains_key(&entry.key) {
            return RecordOutcome::Replayed;
        }
        // Decision P4: a quota before the global capacity, the key quota before the subject quota.
        if let Some(retry_after_secs) = quota_hit(&state.keys, entry.jkt, self.per_key_quota, now) {
            return RecordOutcome::QuotaExceeded { retry_after_secs };
        }
        if let Some(retry_after_secs) = quota_hit(&state.subjects, entry.subject, self.per_subject_quota, now) {
            return RecordOutcome::QuotaExceeded { retry_after_secs };
        }
        if state.entries.len() >= self.capacity {
            return RecordOutcome::CapacityFull { entries: state.entries.len() };
        }
        let remove_at = entry.expires_at.max(entry.follow_up_deadline);
        state.keys.entry(entry.jkt).or_default().add(remove_at);
        state.subjects.entry(entry.subject).or_default().add(remove_at);
        state.by_removal.entry(remove_at).or_default().push(entry.key);
        state.entries.insert(
            entry.key,
            Entry {
                subject: entry.subject,
                jkt: entry.jkt,
                remove_at,
                follow_up_deadline: entry.follow_up_deadline,
                follow_up_digest: entry.follow_up_digest,
                redeemed: false,
            },
        );
        RecordOutcome::Fresh
    }

    fn redeem_follow_up(&self, key: ProofKey, proof_digest: [u8; 32], now: i64) -> RedeemOutcome {
        let mut state = self.lock();
        state.purge(now);
        match state.entries.get_mut(&key) {
            Some(entry) if !entry.redeemed && now <= entry.follow_up_deadline && entry.follow_up_digest == proof_digest => {
                entry.redeemed = true;
                RedeemOutcome::Redeemed
            }
            _ => RedeemOutcome::Refused,
        }
    }
}
```

Add `pub mod dpop_replay;` to `src/adapters/mod.rs` after `pub mod clock;`.

- [ ] **Step 4: Run them to see them pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo fmt
cargo nextest run --locked -p paigasus-iam --lib -E 'test(dpop_replay)'
```

Expected: all ten tests pass.

- [ ] **Step 5: Write the R8 measurement (decision P11)**

Create `rs/crates/services/paigasus-iam/tests/dpop_replay_memory.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! SMA-700 R8: the heap bytes per entry of `InMemoryReplayStore` at the default capacity
//! (200 000). A measurement, not a gate: `#[ignore]`, run by hand with
//! `cargo nextest run -p paigasus-iam --test dpop_replay_memory --run-ignored only --no-capture`.
//! Its own binary, because the counting allocator below applies to the whole binary.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use paigasus_iam::adapters::dpop_replay::InMemoryReplayStore;
use paigasus_iam_core::{NewProof, ProofKey, RecordOutcome, ReplayStore};

/// Counts the live heap bytes of this test binary.
struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(new_size as isize - layout.size() as isize, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn id16(n: u64) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&n.to_le_bytes());
    out
}

#[test]
#[ignore = "SMA-700 R8 measurement; run by hand with --run-ignored only"]
fn bytes_per_entry_at_the_default_capacity() {
    const N: usize = 200_000;
    let store = InMemoryReplayStore::new(N, N, N);
    let before = LIVE.load(Ordering::SeqCst);
    for i in 0..N as u64 {
        // 1000 keys, 500 subjects, removal seconds spread over 120 s: the default window shape.
        let remove_at = 1_000 + (i % 120) as i64;
        let entry = NewProof {
            key: ProofKey(id16(i)),
            subject: id16(1_000_000 + i % 500),
            jkt: id16(2_000_000 + i % 1_000),
            expires_at: remove_at,
            follow_up_deadline: remove_at,
            follow_up_digest: [0u8; 32],
        };
        assert_eq!(store.record(entry, 1_000), RecordOutcome::Fresh);
    }
    let bytes = LIVE.load(Ordering::SeqCst) - before;
    let per_entry = bytes / N as isize;
    println!("SMA-700 R8: {N} entries use {bytes} heap bytes, {per_entry} bytes per entry, {} MiB", bytes / (1024 * 1024));
    assert!(per_entry > 0);
}
```

- [ ] **Step 6: Run the measurement once and record the number**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo nextest run --locked -p paigasus-iam --test dpop_replay_memory --run-ignored only --no-capture 2>&1 | grep 'SMA-700 R8'
cargo nextest run --locked -p paigasus-iam --test dpop_replay_memory
```

Expected: the first command prints one line `SMA-700 R8: 200000 entries use B heap bytes, P bytes per entry, M MiB`. The second runs nothing and passes (the test is ignored by default). Write `P` and `M` into the task report for the coordinator; Task 15 puts them into the runbook and the spec. If `M` is more than 100, STOP and report: the default `replay_capacity` then needs a decision by Sven (R8).

- [ ] **Step 7: Clippy and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo clippy --locked -p paigasus-iam --all-targets -- -D warnings
cd .. && git add rs/crates/services/paigasus-iam/src/adapters/dpop_replay.rs rs/crates/services/paigasus-iam/src/adapters/mod.rs rs/crates/services/paigasus-iam/tests/dpop_replay_memory.rs
git commit -m "feat(rs): in-memory DPoP replay store with quotas and an expiry index (SMA-700)

One mutex, removal of expired heads only, per-key and per-subject quotas
with retry_after_secs, the global capacity, and the one-time follow-up
ticket. An ignored test measures the heap bytes per entry (R8).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

If clippy reports `cast_possible_wrap` or a similar pedantic lint in the measurement test, it is not on by default in this workspace; fix only what `-D warnings` reports.

---

### Task 6: IAM `[authn.dpop]` configuration

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs` (`AuthnConfig` `:113-157`; a new struct after `IssuerConfig` `:176-190`; `validate()` before its final `Ok(())` near `:1440`; tests)
- Modify: `rs/crates/services/paigasus-iam/iam.toml.example` (after line 46, before `# REQUIRED — no default. At least one issuer:`)
- Modify: every `AuthnConfig { … }` literal: `src/service_info.rs:174-187`, `src/adapters/http/mod.rs:1147-1160` (test helper `authn_cfg`), `tests/support/mod.rs:469-490`, `tests/keycloak_e2e.rs:293-314`, `tests/zitadel_e2e.rs:733` onward

**Interfaces:**
- Consumes: nothing new.
- Produces:

```rust
// paigasus_iam::config
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct DpopConfig {
    pub enabled: bool,                    // default false
    pub forwarded_base_urls: Vec<String>, // default []
    pub iat_window_secs: u64,             // default 60, 1..=300
    pub replay_capacity: usize,           // default 200_000
    pub per_key_quota: usize,             // default 1_000
    pub per_subject_quota: usize,         // default 2_000
}
impl Default for DpopConfig { … }
pub struct AuthnConfig { /* existing */ #[serde(default)] pub dpop: DpopConfig }
```

Env names: `IAM_AUTHN__DPOP__ENABLED`, `IAM_AUTHN__DPOP__FORWARDED_BASE_URLS` (inline `["…","…"]`), `IAM_AUTHN__DPOP__IAT_WINDOW_SECS`, `IAM_AUTHN__DPOP__REPLAY_CAPACITY`, `IAM_AUTHN__DPOP__PER_KEY_QUOTA`, `IAM_AUTHN__DPOP__PER_SUBJECT_QUOTA`.

- [ ] **Step 1: Write the failing config tests**

In the `config.rs` test module, after `bootstrap_admins_env_without_quotes_or_with_indexed_keys_does_not_load`, add:

```rust
    // ---- SMA-700: `[authn.dpop]` ---------------------------------------------------------------

    fn dpop_on(urls: &[&str]) -> DpopConfig {
        DpopConfig {
            enabled: true,
            forwarded_base_urls: urls.iter().map(|url| (*url).to_string()).collect(),
            ..DpopConfig::default()
        }
    }

    #[test]
    fn dpop_defaults_are_off_and_land_with_no_dpop_table() {
        let cfg = load_minimal_config();
        assert_eq!(
            cfg.authn.dpop,
            DpopConfig {
                enabled: false,
                forwarded_base_urls: Vec::new(),
                iat_window_secs: 60,
                replay_capacity: 200_000,
                per_key_quota: 1_000,
                per_subject_quota: 2_000,
            }
        );
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn every_dpop_key_reads_from_the_environment() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.set_env("IAM_API_KEYS__PEPPER", valid_pepper_b64());
            jail.create_file("iam.toml", minimal_issuer_toml())?;
            jail.set_env("IAM_AUTHN__DPOP__ENABLED", "true");
            jail.set_env("IAM_AUTHN__DPOP__FORWARDED_BASE_URLS", r#"["https://gw.example.test"]"#);
            jail.set_env("IAM_AUTHN__DPOP__IAT_WINDOW_SECS", "30");
            jail.set_env("IAM_AUTHN__DPOP__REPLAY_CAPACITY", "5000");
            jail.set_env("IAM_AUTHN__DPOP__PER_KEY_QUOTA", "50");
            jail.set_env("IAM_AUTHN__DPOP__PER_SUBJECT_QUOTA", "100");
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert_eq!(
                cfg.authn.dpop,
                DpopConfig {
                    enabled: true,
                    forwarded_base_urls: vec!["https://gw.example.test".to_string()],
                    iat_window_secs: 30,
                    replay_capacity: 5_000,
                    per_key_quota: 50,
                    per_subject_quota: 100,
                }
            );
            assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
            Ok(())
        });
    }

    #[test]
    fn dpop_env_in_the_chart_form_parses() {
        // The exact strings that charts/paigasus renders for zones.iam.backend.dpop (Task 14,
        // `tests/env.sh` row D2): each URL quoted with %q, joined by a comma, in brackets.
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.set_env("IAM_API_KEYS__PEPPER", valid_pepper_b64());
            jail.set_env("IAM_AUTHN__ISSUERS", r#"[{issuer="https://idp.example.test/realms/paigasus",audiences=["paigasus-console"]}]"#);
            jail.set_env("IAM_AUTHN__DPOP__ENABLED", "true");
            jail.set_env("IAM_AUTHN__DPOP__FORWARDED_BASE_URLS", r#"["https://gw.example.test","https://edge.example.test/api"]"#);
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.authn.dpop.enabled);
            assert_eq!(cfg.authn.dpop.forwarded_base_urls, vec!["https://gw.example.test".to_string(), "https://edge.example.test/api".to_string()]);
            assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
            Ok(())
        });
    }

    #[test]
    fn validate_refuses_each_bad_dpop_value_and_names_the_key() {
        let cases: Vec<(&str, DpopConfig, &str)> = vec![
            ("on with no URL", dpop_on(&[]), "authn.dpop.enabled is true and authn.dpop.forwarded_base_urls is empty"),
            ("not a URL", dpop_on(&["gw.example.test"]), "authn.dpop.forwarded_base_urls entry \"gw.example.test\" is not an absolute URL"),
            ("a query", dpop_on(&["https://gw.example.test/?a=1"]), "must have no query, fragment or user info"),
            ("an empty query", dpop_on(&["https://gw.example.test/?"]), "must have no query, fragment or user info"),
            ("a fragment", dpop_on(&["https://gw.example.test/#x"]), "must have no query, fragment or user info"),
            ("user info", dpop_on(&["https://user@gw.example.test"]), "must have no query, fragment or user info"),
            ("a password", dpop_on(&["https://user:pw@gw.example.test"]), "must have no query, fragment or user info"),
            ("http, not loopback", dpop_on(&["http://gw.example.test"]), "must use https, or http on a loopback host"),
            ("http on a name that starts with 127", dpop_on(&["http://127.evil.example"]), "must use https, or http on a loopback host"),
            ("ftp", dpop_on(&["ftp://gw.example.test"]), "must use https, or http on a loopback host"),
            ("padding", dpop_on(&[" https://gw.example.test"]), "has leading or trailing whitespace"),
            ("off, but a bad entry (P8)", DpopConfig { enabled: false, ..dpop_on(&["http://gw.example.test"]) }, "must use https, or http on a loopback host"),
            ("window 0", DpopConfig { iat_window_secs: 0, ..dpop_on(&["https://gw.example.test"]) }, "authn.dpop.iat_window_secs must be between 1 and 300"),
            ("window 301", DpopConfig { iat_window_secs: 301, ..dpop_on(&["https://gw.example.test"]) }, "authn.dpop.iat_window_secs must be between 1 and 300"),
            ("capacity 0", DpopConfig { replay_capacity: 0, per_key_quota: 0, per_subject_quota: 0, ..dpop_on(&["https://gw.example.test"]) }, "authn.dpop.replay_capacity must be at least 1"),
            ("key quota 0", DpopConfig { per_key_quota: 0, ..dpop_on(&["https://gw.example.test"]) }, "authn.dpop.per_key_quota must be at least 1"),
            ("subject quota 0", DpopConfig { per_subject_quota: 0, ..dpop_on(&["https://gw.example.test"]) }, "authn.dpop.per_subject_quota must be at least 1"),
            ("key quota above capacity", DpopConfig { replay_capacity: 10, per_key_quota: 11, per_subject_quota: 10, ..dpop_on(&["https://gw.example.test"]) }, "authn.dpop.per_key_quota (11) must not exceed authn.dpop.replay_capacity (10)"),
            ("subject quota above capacity", DpopConfig { replay_capacity: 10, per_key_quota: 10, per_subject_quota: 11, ..dpop_on(&["https://gw.example.test"]) }, "authn.dpop.per_subject_quota (11) must not exceed authn.dpop.replay_capacity (10)"),
        ];
        for (name, dpop, want) in cases {
            let mut cfg = load_minimal_config();
            cfg.authn.dpop = dpop;
            let err = cfg.validate().expect_err(name);
            assert!(err.contains(want), "{name}: want {want:?} in {err}");
        }
    }

    #[test]
    fn validate_accepts_the_loopback_forms() {
        // Review Focus 5: a local gateway on IPv6 loopback, and the other loopback forms.
        let mut cfg = load_minimal_config();
        cfg.authn.dpop = dpop_on(&["http://localhost:8088", "http://LOCALHOST", "http://127.0.0.1:8088", "http://127.9.8.7", "http://[::1]:8088", "https://gw.example.test/api/"]);
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }
```

- [ ] **Step 2: Run them to see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo nextest run --locked -p paigasus-iam --lib -E 'test(dpop)'
```

Expected: compile error `cannot find struct DpopConfig`.

- [ ] **Step 3: Implement `DpopConfig`**

In `AuthnConfig`, after `pub extra_ca_bundle_path: Option<String>,` (before `jwks_cache`), add:

```rust
    /// The DPoP proof check of the gateway path (SMA-700). Off by default; an absent
    /// `[authn.dpop]` table is valid config.
    #[serde(default)]
    pub dpop: DpopConfig,
```

After `fn default_jit_provisioning()`, add:

```rust
/// `[authn.dpop]` (SMA-700 § 4.9). IAM checks the DPoP proof that the gateway forwards in
/// `Introspect`, and accepts the one `IsAuthorized` follow-up. Every field has a default, so a
/// partial table, or a single `IAM_AUTHN__DPOP__*` env var, is valid. The `MigrationConfig`
/// pattern, with a container-level `#[serde(default)]`.
///
/// An entry lives up to `2 × iat_window_secs` (a proof with `iat = now + window` expires at
/// `now + 2 × window`), so with the defaults a key can make about `1000 / 120 ≈ 8` proofs a second,
/// and a subject about 16, with no quota hit.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct DpopConfig {
    pub enabled: bool,
    /// The public base URLs at which clients reach the gateway, each with any path prefix that a
    /// proxy removes (D5, D6). IAM checks `htu` against these, never against a request header.
    pub forwarded_base_urls: Vec<String>,
    /// The allowed `|now - iat|` of a proof, in seconds. 1 to 300. Its own setting: it does not add
    /// `authn.leeway_secs` (D20).
    pub iat_window_secs: u64,
    /// The global entry cap of the replay store. A full store answers `Unavailable` (D10).
    pub replay_capacity: usize,
    /// The live entries allowed for one `jkt`. A full quota answers `dpop-quota-exceeded` (429).
    pub per_key_quota: usize,
    /// The live entries allowed for one `(issuer, subject)`.
    pub per_subject_quota: usize,
}

impl Default for DpopConfig {
    fn default() -> Self {
        DpopConfig {
            enabled: false,
            forwarded_base_urls: Vec::new(),
            iat_window_secs: 60,
            replay_capacity: 200_000,
            per_key_quota: 1_000,
            per_subject_quota: 2_000,
        }
    }
}

/// The loopback hosts on which `http` is allowed for a `forwarded_base_urls` entry.
fn is_loopback_host(host: Option<url::Host<&str>>) -> bool {
    match host {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// One `forwarded_base_urls` entry, checked as `validate()` requires (SMA-700 § 4.9).
fn check_forwarded_base_url(raw: &str) -> Result<(), String> {
    if raw.trim() != raw {
        return Err(format!("authn.dpop.forwarded_base_urls entry {raw:?} has leading or trailing whitespace"));
    }
    let url = url::Url::parse(raw).map_err(|e| format!("authn.dpop.forwarded_base_urls entry {raw:?} is not an absolute URL: {e}"))?;
    if url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() || url.password().is_some() {
        return Err(format!("authn.dpop.forwarded_base_urls entry {raw:?} must have no query, fragment or user info"));
    }
    let allowed = match url.scheme() {
        "https" => url.host().is_some(),
        "http" => is_loopback_host(url.host()),
        _ => false,
    };
    if !allowed {
        return Err(format!(
            "authn.dpop.forwarded_base_urls entry {raw:?} must use https, or http on a loopback host (localhost, 127.0.0.0/8, ::1)"
        ));
    }
    Ok(())
}
```

In `validate()`, before the final `Ok(())`, add:

```rust
        // --- SMA-700: `[authn.dpop]` ------------------------------------------------------------
        // A refused boot leaves no IAM pod (one replica, maxSurge 0), so the chart copies the
        // forwarded_base_urls rules (`paigasus.validateIamDpop`). Entries are checked also when
        // DPoP is off (decision P8).
        let dpop = &self.authn.dpop;
        if dpop.enabled && dpop.forwarded_base_urls.is_empty() {
            return Err("authn.dpop.enabled is true and authn.dpop.forwarded_base_urls is empty: list the public URLs at which clients reach the gateway".to_string());
        }
        for raw in &dpop.forwarded_base_urls {
            check_forwarded_base_url(raw)?;
        }
        if !(1..=300).contains(&dpop.iat_window_secs) {
            return Err(format!("authn.dpop.iat_window_secs must be between 1 and 300 (got {})", dpop.iat_window_secs));
        }
        for (name, value) in [
            ("replay_capacity", dpop.replay_capacity),
            ("per_key_quota", dpop.per_key_quota),
            ("per_subject_quota", dpop.per_subject_quota),
        ] {
            if value == 0 {
                return Err(format!("authn.dpop.{name} must be at least 1"));
            }
        }
        for (name, value) in [("per_key_quota", dpop.per_key_quota), ("per_subject_quota", dpop.per_subject_quota)] {
            if value > dpop.replay_capacity {
                return Err(format!("authn.dpop.{name} ({value}) must not exceed authn.dpop.replay_capacity ({})", dpop.replay_capacity));
            }
        }
```

Extend the doc comment of `validate()` with one sentence: `Also (SMA-700): the `[authn.dpop]` rules — a non-empty URL list when enabled, each URL https (or http on a loopback host) with no query, fragment or user info, a window of 1 to 300 s, and quotas from 1 to the capacity.`

- [ ] **Step 4: Add `dpop` to every `AuthnConfig` literal**

In each literal, add one line `dpop: DpopConfig::default(),` after `extra_ca_bundle_path: …,`, and add `DpopConfig` to that file's `use paigasus_iam::config::{…}` (or `use crate::config::{…}`) import:

- `src/service_info.rs:174-187` (`iam_config_with_empty_authn`)
- `src/adapters/http/mod.rs:1147-1160` (`authn_cfg`)
- `tests/support/mod.rs:469-490` (`test_config_with`)
- `tests/keycloak_e2e.rs:293-314` (`keycloak_config`; Task 10 changes this one to DPoP on)
- `tests/zitadel_e2e.rs` (the `AuthnConfig` literal in the config helper that starts at line 724)

Check none is missed:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
grep -rn "AuthnConfig {" --include='*.rs' crates | grep -v "pub struct"
```

Expected: five hits, the five files above.

- [ ] **Step 5: `iam.toml.example`**

After line 46 (the end of the `[authn.jwks_cache]` TRUST NOTE), insert:

```toml

# --- DPoP proof check on the gateway path (SMA-700) ---
#
# Off by default. With enabled = true, IAM checks the DPoP proof (RFC 9449) that the gateway
# forwards in Introspect, and accepts the one IsAuthorized follow-up of the gateway. IAM's own
# API keeps refusing the DPoP scheme. The replay store is in memory: run ONE IAM replica.
# [authn.dpop]
# enabled = false                       # default shown
# forwarded_base_urls = []              # REQUIRED when enabled: the public URLs at which clients
#                                       # reach the gateway, each with any path prefix that a proxy
#                                       # removes, e.g. ["https://gw.example.com", "https://edge.example.com/api"].
#                                       # https only, or http on localhost/127.0.0.0/8/::1. No query,
#                                       # fragment or user info.
# iat_window_secs = 60                  # allowed |now - iat| of a proof, 1-300 — default shown
# replay_capacity = 200000              # global replay-store entries; full -> 503 — default shown
# per_key_quota = 1000                  # live entries per proof key; full -> 429 — default shown
# per_subject_quota = 2000              # live entries per user — default shown
#
# An entry lives up to 2 × iat_window_secs, so with the defaults one key can make about 8 proofs a
# second and one user about 16. Env: IAM_AUTHN__DPOP__ENABLED, IAM_AUTHN__DPOP__FORWARDED_BASE_URLS
# (inline form: ["https://gw.example.com"]), IAM_AUTHN__DPOP__IAT_WINDOW_SECS, …
```

- [ ] **Step 6: Run the tests to see them pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo fmt
cargo nextest run --locked -p paigasus-iam --lib -E 'test(config::) | test(service_info)'
cargo build --locked -p paigasus-iam --all-targets
cargo clippy --locked -p paigasus-iam --all-targets -- -D warnings
```

Expected: all pass. If `url::Url::parse("http://127.evil.example")` gives `Host::Domain`, the "name that starts with 127" case is refused as intended.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git add rs/crates/services/paigasus-iam/src/config.rs rs/crates/services/paigasus-iam/iam.toml.example rs/crates/services/paigasus-iam/src/service_info.rs \
  rs/crates/services/paigasus-iam/src/adapters/http/mod.rs rs/crates/services/paigasus-iam/tests/support/mod.rs rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs rs/crates/services/paigasus-iam/tests/zitadel_e2e.rs
git commit -m "feat(rs): IAM authn.dpop configuration and its boot rules (SMA-700)

Off by default. The forwarded base URLs must be https or loopback http
with no query, fragment or user info; the window is 1 to 300 s; the
quotas are 1 to the capacity. A test parses the chart's env form.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: `DpopProofVerifier` — checks 10-13 and the follow-up redeem

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/application/dpop.rs`
- Modify: `rs/crates/services/paigasus-iam/src/application/mod.rs` (add `pub mod dpop;` after `pub mod dead_letters;`)

**Interfaces:**
- Consumes: `DpopProofChecker`, `ReplayStore`, `ProofKey`, `NewProof`, `RecordOutcome`, `RedeemOutcome`, `Jkt`, `ProofDefect`, `ValidatedClaims`, `Clock` (Task 2); `LogRateLimiter` (`application::log_rate_limit`).
- Produces (`paigasus_iam::application::dpop`):

```rust
pub const MAX_PROOF_BYTES: usize = 8192;
pub const MAX_METHOD_BYTES: usize = 16;
pub const MAX_PATH_BYTES: usize = 2048;
pub struct DpopRequest { pub proof: String, pub method: String, pub path: String }   // Clone, PartialEq, Eq; Debug prints no field
pub struct DpopProofVerifier { … }
impl DpopProofVerifier {
    pub fn new(checker: Arc<dyn DpopProofChecker>, store: Arc<dyn ReplayStore>, clock: Arc<dyn Clock>, forwarded_base_urls: &[String], iat_window_secs: u64) -> Result<Self, String>;
    /// § 4.8 request checks: sizes, an empty proof, the forwarded path. No clock, no token.
    pub fn check_request(&self, request: &DpopRequest) -> Result<(), AuthnError>;
    /// Checks 1-13 for an Introspect, after the token passed `authenticate(.., Dpop)`.
    pub fn verify(&self, claims: &ValidatedClaims, token: &str, request: &DpopRequest) -> Result<(), AuthnError>;
    /// The IsAuthorized follow-up: `follow_up_claims`, `ath`, then the one-time ticket.
    pub fn redeem(&self, claims: &ValidatedClaims, token: &str, proof: &str) -> Result<(), AuthnError>;
}
```

- [ ] **Step 1: Write the tests first**

Create `src/application/dpop.rs` with this test module at its end:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::dpop_replay::InMemoryReplayStore;
    use crate::application::fakes::FixedClock;
    use chrono::{TimeZone, Utc};
    use paigasus_iam_core::{FollowUpClaims, Issuer, ProofClaims};
    use paigasus_logging::test_support::capture_logs;
    use std::sync::Mutex;

    const NOW: i64 = 1_700_000_000;
    const BASE: &str = "https://gw.example.test";
    const URL: &str = "https://gw.example.test/v1/chat/completions";
    const PATH: &str = "/v1/chat/completions";
    const ISSUER: &str = "https://idp.example.test";

    /// A `DpopProofChecker` that returns scripted answers, so each verifier test changes one
    /// field of the claims (checks 1-9 have their own tests in `adapters::oidc::dpop`).
    struct ScriptedChecker {
        check: Mutex<Result<ProofClaims, ProofDefect>>,
        follow_up: Result<FollowUpClaims, ProofDefect>,
        ath_ok: bool,
    }

    impl DpopProofChecker for ScriptedChecker {
        fn check(&self, _proof: &str, _token: &str, _jkt: &Jkt) -> Result<ProofClaims, ProofDefect> {
            self.check.lock().unwrap().clone()
        }
        fn follow_up_claims(&self, _proof: &str) -> Result<FollowUpClaims, ProofDefect> {
            self.follow_up.clone()
        }
        fn ath_matches(&self, _ath: &str, _token: &str) -> bool {
            self.ath_ok
        }
    }

    fn proof_claims(htm: &str, htu: &str, iat: i64) -> ProofClaims {
        ProofClaims {
            jti: "jti-1".into(),
            iat,
            htm: htm.into(),
            htu: htu.into(),
        }
    }

    fn checker(check: Result<ProofClaims, ProofDefect>) -> ScriptedChecker {
        ScriptedChecker {
            check: Mutex::new(check),
            follow_up: Ok(FollowUpClaims {
                jti: "jti-1".into(),
                ath: "ath".into(),
            }),
            ath_ok: true,
        }
    }

    fn verifier_with(checker: ScriptedChecker, bases: &[&str], store: Arc<InMemoryReplayStore>) -> (DpopProofVerifier, FixedClock) {
        let clock = FixedClock::default();
        clock.set(Utc.timestamp_opt(NOW, 0).unwrap());
        let bases: Vec<String> = bases.iter().map(|b| (*b).to_string()).collect();
        let verifier = DpopProofVerifier::new(Arc::new(checker), store, Arc::new(clock.clone()), &bases, 60).expect("test bases parse");
        (verifier, clock)
    }

    fn verifier(check: Result<ProofClaims, ProofDefect>) -> (DpopProofVerifier, FixedClock) {
        verifier_with(checker(check), &[BASE], Arc::new(InMemoryReplayStore::new(100, 100, 100)))
    }

    fn bound(sub: &str, jkt: &str) -> ValidatedClaims {
        ValidatedClaims {
            issuer: Issuer::parse(ISSUER).unwrap(),
            subject: sub.into(),
            audiences: vec!["aud".into()],
            expires_at: Utc.timestamp_opt(NOW + 3600, 0).unwrap(),
            email: None,
            name: None,
            locale: None,
            zoneinfo: None,
            key_binding: Some(Jkt::new(jkt)),
        }
    }

    fn request(path: &str) -> DpopRequest {
        DpopRequest {
            proof: "p.r.oof".into(),
            method: "POST".into(),
            path: path.into(),
        }
    }

    fn defect(result: Result<(), AuthnError>) -> ProofDefect {
        match result {
            Err(AuthnError::InvalidDpopProof(defect)) => defect,
            other => panic!("want InvalidDpopProof, got {other:?}"),
        }
    }

    #[test]
    fn a_valid_proof_for_the_request_passes() {
        let (v, _) = verifier(Ok(proof_claims("POST", URL, NOW)));
        v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH)).expect("passes");
    }

    #[test]
    fn checks_1_to_9_pass_through_their_defect() {
        let (v, _) = verifier(Err(ProofDefect::Thumbprint));
        assert_eq!(defect(v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH))), ProofDefect::Thumbprint);
    }

    #[test]
    fn check_10_htm_is_exact() {
        // § 5.1: `post` against `POST` fails.
        let (v, _) = verifier(Ok(proof_claims("post", URL, NOW)));
        assert_eq!(defect(v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH))), ProofDefect::Htm);
    }

    #[test]
    fn check_11_htu_rules() {
        // § 5.1 `htu` cases. Each (base list, htu, forwarded path, passes).
        let cases: [(&[&str], &str, &str, bool); 11] = [
            (&[BASE], "https://gw.example.test:443/v1/chat/completions", PATH, true),
            (&[BASE], "https://GW.Example.Test/v1/chat/completions", PATH, true),
            (&[BASE], "https://gw.example.test/v1/chat/completions?x=1#f", PATH, true),
            (&["https://edge.example.test/api"], "https://edge.example.test/api/v1/chat/completions", PATH, true),
            (&["https://edge.example.test/api"], "https://edge.example.test/v1/chat/completions", PATH, false),
            (&[BASE], "https://gw.example.test/v1/chat/%63ompletions", PATH, false),
            (&[BASE], "https://gw.example.test/v1/chat/completions/", PATH, false),
            (&[BASE], "https://other.example.test/v1/chat/completions", PATH, false),
            (&[BASE], "http://gw.example.test/v1/chat/completions", PATH, false),
            (&[BASE], "not a url", PATH, false),
            (&["https://one.example.test", BASE], URL, PATH, true),
        ];
        for (bases, htu, path, passes) in cases {
            let store = Arc::new(InMemoryReplayStore::new(100, 100, 100));
            let (v, _) = verifier_with(checker(Ok(proof_claims("POST", htu, NOW))), bases, store);
            let result = v.verify(&bound("alice", "jkt-a"), "tok", &request(path));
            if passes {
                result.unwrap_or_else(|e| panic!("{bases:?} {htu}: want a pass, got {e:?}"));
            } else {
                assert_eq!(defect(result), ProofDefect::Htu, "{bases:?} {htu}");
            }
        }
    }

    #[test]
    fn a_base_url_with_a_trailing_slash_still_matches() {
        // Review Focus 2.
        for (base, htu) in [("https://gw.example.test/", URL), ("https://edge.example.test/api/", "https://edge.example.test/api/v1/chat/completions")] {
            let (v, _) = verifier_with(checker(Ok(proof_claims("POST", htu, NOW))), &[base], Arc::new(InMemoryReplayStore::new(100, 100, 100)));
            v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH)).unwrap_or_else(|e| panic!("{base}: {e:?}"));
        }
    }

    #[test]
    fn an_ipv6_loopback_base_url_matches() {
        // Review Focus 5.
        let htu = "http://[::1]:8088/v1/chat/completions";
        let (v, _) = verifier_with(checker(Ok(proof_claims("POST", htu, NOW))), &["http://[::1]:8088"], Arc::new(InMemoryReplayStore::new(100, 100, 100)));
        v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH)).expect("passes");
    }

    #[test]
    fn check_12_iat_window_edges() {
        for (iat, passes) in [(NOW - 60, true), (NOW + 60, true), (NOW - 61, false), (NOW + 61, false), (i64::MAX, false), (i64::MIN, false)] {
            let (v, _) = verifier(Ok(proof_claims("POST", URL, iat)));
            let result = v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH));
            if passes {
                result.unwrap_or_else(|e| panic!("iat {iat}: {e:?}"));
            } else {
                assert_eq!(defect(result), ProofDefect::Iat, "iat {iat}");
            }
        }
    }

    #[test]
    fn check_13_a_second_use_is_a_replay_and_a_failed_check_uses_no_jti() {
        let store = Arc::new(InMemoryReplayStore::new(100, 100, 100));
        // A proof that fails check 10 records nothing.
        let (bad, _) = verifier_with(checker(Ok(proof_claims("GET", URL, NOW))), &[BASE], store.clone());
        assert_eq!(defect(bad.verify(&bound("alice", "jkt-a"), "tok", &request(PATH))), ProofDefect::Htm);
        let (v, _) = verifier_with(checker(Ok(proof_claims("POST", URL, NOW))), &[BASE], store);
        v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH)).expect("the first use passes");
        assert_eq!(defect(v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH))), ProofDefect::Replayed);
    }

    #[test]
    fn the_same_jti_under_two_keys_is_not_a_replay() {
        // Review Focus 4: the replay key includes the jkt.
        let (v, _) = verifier(Ok(proof_claims("POST", URL, NOW)));
        v.verify(&bound("alice", "jkt-a"), "tok-a", &request(PATH)).expect("key a");
        v.verify(&bound("bob", "jkt-b"), "tok-b", &request(PATH)).expect("key b, same jti");
    }

    #[test]
    fn a_quota_hit_is_dpop_quota_exceeded_and_a_full_store_is_unavailable() {
        let alice = bound("alice", "jkt-a");
        // One store with a key quota of 1, shared by two verifiers whose proofs have two jtis.
        let shared = Arc::new(InMemoryReplayStore::new(100, 1, 100));
        let (first, _) = verifier_with(checker(Ok(proof_claims("POST", URL, NOW))), &[BASE], shared.clone());
        first.verify(&alice, "tok", &request(PATH)).expect("the first proof of jkt-a");
        let second_jti = ScriptedChecker {
            check: Mutex::new(Ok(ProofClaims {
                jti: "jti-2".into(),
                ..proof_claims("POST", URL, NOW)
            })),
            ..checker(Ok(proof_claims("POST", URL, NOW)))
        };
        let (second, _) = verifier_with(second_jti, &[BASE], shared);
        // iat NOW, window 60: the oldest entry goes at NOW + 61, so retry after 61 s.
        assert!(matches!(second.verify(&alice, "tok", &request(PATH)), Err(AuthnError::DpopQuotaExceeded { retry_after_secs: 61 })));

        let (full, _) = verifier_with(checker(Ok(proof_claims("POST", URL, NOW))), &[BASE], Arc::new(InMemoryReplayStore::new(1, 1, 1)));
        full.verify(&alice, "tok", &request(PATH)).expect("fills the store");
        assert!(matches!(full.verify(&bound("bob", "jkt-b"), "tok", &request(PATH)), Err(AuthnError::Unavailable)));
    }

    #[test]
    fn the_forwarded_path_must_not_move_the_origin() {
        // § 5.1 forwarded path cases: each gives Malformed, before any proof check.
        let (v, _) = verifier(Err(ProofDefect::Signature));
        for path in [".evil.example/x", "@evil.example/x", ":8443/x", "x", "/v1?x=1", "/v1#f", "/v1\\x", "/v1\u{0}x", "/v1\nx", ""] {
            assert_eq!(defect(v.check_request(&request(path))), ProofDefect::Malformed, "{path:?}");
        }
        v.check_request(&request(PATH)).expect("a plain path passes");
        v.check_request(&request("//evil.example/x")).expect("a double slash stays on the base host; the host check after the parse holds");
    }

    #[test]
    fn the_request_sizes_and_an_empty_proof() {
        let (v, _) = verifier(Err(ProofDefect::Signature));
        let big_proof = DpopRequest { proof: "a".repeat(MAX_PROOF_BYTES + 1), ..request(PATH) };
        let big_method = DpopRequest { method: "M".repeat(MAX_METHOD_BYTES + 1), ..request(PATH) };
        let big_path = DpopRequest { path: format!("/{}", "a".repeat(MAX_PATH_BYTES)), ..request(PATH) };
        let empty = DpopRequest { proof: String::new(), ..request(PATH) };
        assert_eq!(defect(v.check_request(&big_proof)), ProofDefect::Malformed);
        assert_eq!(defect(v.check_request(&big_method)), ProofDefect::Malformed);
        assert_eq!(defect(v.check_request(&big_path)), ProofDefect::Malformed);
        assert_eq!(defect(v.check_request(&empty)), ProofDefect::Missing);
        let edge = DpopRequest { proof: "a".repeat(MAX_PROOF_BYTES), method: "M".repeat(MAX_METHOD_BYTES), path: format!("/{}", "a".repeat(MAX_PATH_BYTES - 1)) };
        v.check_request(&edge).expect("the sizes themselves pass");
    }

    #[test]
    fn the_follow_up_redeems_once_with_the_same_proof_bytes() {
        let (v, _) = verifier(Ok(proof_claims("POST", URL, NOW)));
        let alice = bound("alice", "jkt-a");
        let introspected = request(PATH);
        assert_eq!(defect(v.redeem(&alice, "tok", &introspected.proof)), ProofDefect::FollowUp, "no ticket before Introspect");
        v.verify(&alice, "tok", &introspected).expect("Introspect");
        assert_eq!(defect(v.redeem(&alice, "tok", "other.proof.bytes")), ProofDefect::FollowUp, "other bytes");
        v.redeem(&alice, "tok", &introspected.proof).expect("the follow-up, still redeemable after the wrong digest");
        assert_eq!(defect(v.redeem(&alice, "tok", &introspected.proof)), ProofDefect::FollowUp, "once only");
    }

    #[test]
    fn the_follow_up_checks_ath_and_the_proof_shape() {
        let store = Arc::new(InMemoryReplayStore::new(100, 100, 100));
        let (v, _) = verifier_with(ScriptedChecker { ath_ok: false, ..checker(Ok(proof_claims("POST", URL, NOW))) }, &[BASE], store);
        assert_eq!(defect(v.redeem(&bound("alice", "jkt-a"), "tok", "p.r.oof")), ProofDefect::Ath);
        let (v, _) = verifier_with(
            ScriptedChecker { follow_up: Err(ProofDefect::Malformed), ..checker(Ok(proof_claims("POST", URL, NOW))) },
            &[BASE],
            Arc::new(InMemoryReplayStore::new(100, 100, 100)),
        );
        assert_eq!(defect(v.redeem(&bound("alice", "jkt-a"), "tok", "p.r.oof")), ProofDefect::Malformed);
        assert_eq!(defect(v.redeem(&bound("alice", "jkt-a"), "tok", "")), ProofDefect::Missing);
    }

    #[test]
    fn the_follow_up_ticket_expires() {
        let (v, clock) = verifier(Ok(proof_claims("POST", URL, NOW)));
        let alice = bound("alice", "jkt-a");
        v.verify(&alice, "tok", &request(PATH)).expect("Introspect at NOW");
        // follow_up_deadline = max(NOW + 60, NOW + 30) = NOW + 60.
        clock.set(Utc.timestamp_opt(NOW + 61, 0).unwrap());
        assert_eq!(defect(v.redeem(&alice, "tok", &request(PATH).proof)), ProofDefect::FollowUp);
    }

    #[test]
    fn a_refusal_logs_the_issuer_and_the_static_defect_only() {
        let (logs, _guard) = capture_logs();
        let (v, _) = verifier(Ok(proof_claims("POST", "https://other.example.test/secret-path", NOW)));
        let _ = v.verify(&bound("alice-subject", "jkt-secret"), "token-secret", &DpopRequest { proof: "proof-secret".into(), ..request("/v1/secret-path") });
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|l| l.contains("refused a DPoP proof")).collect();
        assert_eq!(lines.len(), 1, "{text}");
        assert!(lines[0].contains("INFO") && lines[0].contains(ISSUER) && lines[0].contains("htu"), "{}", lines[0]);
        for secret in ["alice-subject", "jkt-secret", "token-secret", "proof-secret", "secret-path", "jti-1", "other.example.test"] {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }

    #[test]
    fn a_full_store_writes_a_warn_line_with_the_entry_count() {
        let (logs, _guard) = capture_logs();
        let (v, _) = verifier_with(checker(Ok(proof_claims("POST", URL, NOW))), &[BASE], Arc::new(InMemoryReplayStore::new(1, 1, 1)));
        v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH)).expect("fills the store");
        let _ = v.verify(&bound("bob", "jkt-b"), "tok", &request(PATH));
        let text = logs.text();
        let line = text.lines().find(|l| l.contains("the DPoP replay store is full")).unwrap_or_else(|| panic!("{text}"));
        assert!(line.contains("WARN") && line.contains("entries=1"), "{line}");
    }

    #[test]
    fn new_refuses_a_base_url_it_cannot_parse() {
        let bases = vec!["not a url".to_string()];
        let err = DpopProofVerifier::new(
            Arc::new(checker(Err(ProofDefect::Signature))),
            Arc::new(InMemoryReplayStore::new(1, 1, 1)),
            Arc::new(FixedClock::default()),
            &bases,
            60,
        )
        .err()
        .expect("an unparseable base URL is a wiring defect");
        assert!(err.contains("forwarded_base_urls"), "{err}");
    }

    #[test]
    fn dpop_request_debug_prints_no_field() {
        let printed = format!("{:?}", DpopRequest { proof: "proof-secret".into(), method: "POST".into(), path: "/secret".into() });
        assert_eq!(printed, "DpopRequest { .. }");
    }
}
```

Add `pub mod dpop;` to `src/application/mod.rs` after `pub mod dead_letters;`.

- [ ] **Step 2: Run them to see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo nextest run --locked -p paigasus-iam --lib -E 'test(application::dpop)'
```

Expected: compile error `cannot find type DpopProofVerifier`.

- [ ] **Step 3: Implement the verifier above the test module**

```rust
// SPDX-License-Identifier: Apache-2.0

//! `DpopProofVerifier` (SMA-700 D2, § 4.4 checks 10-13, § 4.6, § 4.8): the request checks of a
//! DPoP proof that the gateway forwards in `Introspect`, the `iat` window, the replay store, and
//! the one-time `IsAuthorized` follow-up. Checks 1-9 are behind the `DpopProofChecker` port; this
//! module has no JOSE code. The verifier owns the configured base-URL list: no adapter can pass
//! another one.
//!
//! Logs: one rate-limited `info` line for each refusal, "refused a DPoP proof", with the issuer and
//! the static defect name. A request refusal before the token is verified (§ 4.8, decision P5)
//! logs the issuer `-`. A quota hit writes one rate-limited `info` line, and a full store one
//! rate-limited `warn` line with the entry count. No line carries the proof, the token, the `jti`,
//! the `jkt`, the path or a URL.

use std::sync::Arc;
use std::time::Instant;

use paigasus_iam_core::{
    AuthnError, Clock, DpopProofChecker, Jkt, NewProof, ProofDefect, ProofKey, RecordOutcome, RedeemOutcome, ReplayStore, TokenDefect, ValidatedClaims,
};
use url::Url;

use crate::application::log_rate_limit::{LOG_RATE_LIMIT_INTERVAL, LogRateLimiter};

/// The largest proof IAM reads (SMA-700 § 4.1). Equal to the checker's own check-1 bound.
pub const MAX_PROOF_BYTES: usize = 8192;
/// The largest forwarded HTTP method (§ 4.1).
pub const MAX_METHOD_BYTES: usize = 16;
/// The largest forwarded path (§ 4.1).
pub const MAX_PATH_BYTES: usize = 2048;
/// The follow-up ticket lives at least this long after `record` (§ 4.5).
const MIN_FOLLOW_UP_SECS: i64 = 30;
/// The issuer field of a refusal that happens before the token is verified (decision P5).
const UNVERIFIED_ISSUER: &str = "-";

/// The DPoP context of one `Introspect` (SMA-700 § 4.1). `Debug` prints no field: the proof is
/// a credential and the path is request data (§ 4.8).
#[derive(Clone, PartialEq, Eq)]
pub struct DpopRequest {
    pub proof: String,
    pub method: String,
    pub path: String,
}

impl std::fmt::Debug for DpopRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DpopRequest").finish_non_exhaustive()
    }
}

/// One configured `forwarded_base_urls` entry, parsed at boot (§ 4.6): scheme, host, port (a
/// default port is `None`) and a path prefix with no trailing `/` (empty for the root).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ForwardedBase {
    scheme: String,
    host: String,
    port: Option<u16>,
    prefix: String,
}

impl ForwardedBase {
    fn parse(raw: &str) -> Result<Self, String> {
        let url = Url::parse(raw).map_err(|e| format!("authn.dpop.forwarded_base_urls entry {raw:?} does not parse: {e}"))?;
        let host = url.host_str().ok_or_else(|| format!("authn.dpop.forwarded_base_urls entry {raw:?} has no host"))?.to_owned();
        Ok(ForwardedBase {
            scheme: url.scheme().to_owned(),
            host,
            port: url.port(),
            prefix: url.path().trim_end_matches('/').to_owned(),
        })
    }

    /// The URL that the client signed for `path` through this base, or `None` when the parse
    /// moves the scheme, the host or the port away from the base (§ 4.6).
    fn expected(&self, path: &str) -> Option<Url> {
        let port = self.port.map(|p| format!(":{p}")).unwrap_or_default();
        let url = Url::parse(&format!("{}://{}{}{}{}", self.scheme, self.host, port, self.prefix, path)).ok()?;
        (url.scheme() == self.scheme && url.host_str() == Some(self.host.as_str()) && url.port() == self.port).then_some(url)
    }
}

/// § 4.6: after the same `url` parser, the scheme, the host, the port (a default port removed) and
/// `path()` are equal. The query and the fragment of `htu` are ignored. `path()` keeps the
/// percent-encoding, so a path that differs only by it does not match (a deliberate deviation from
/// RFC 3986 § 6.2.2).
fn same_target(htu: &Url, expected: &Url) -> bool {
    htu.scheme() == expected.scheme() && htu.host_str() == expected.host_str() && htu.port() == expected.port() && htu.path() == expected.path()
}

/// § 4.8: the forwarded path starts with `/` and has no `?`, `#`, `\` or control character.
fn path_is_well_formed(path: &str) -> bool {
    path.starts_with('/') && !path.chars().any(|c| matches!(c, '?' | '#' | '\\') || c.is_control())
}

/// The two replay-store events with their own log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum StoreEvent {
    Quota,
    Capacity,
}

fn first_16(hash: &blake3::Hash) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&hash.as_bytes()[..16]);
    out
}

/// § 4.5: the first 16 bytes of `blake3(jkt || 0x00 || jti)`.
fn proof_key(jkt: &Jkt, jti: &str) -> ProofKey {
    let mut hasher = blake3::Hasher::new();
    hasher.update(jkt.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(jti.as_bytes());
    ProofKey(first_16(&hasher.finalize()))
}

/// § 4.5: the first 16 bytes of `blake3(issuer || 0x00 || sub)`.
fn subject_hash(claims: &ValidatedClaims) -> [u8; 16] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(claims.issuer.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(claims.subject.as_bytes());
    first_16(&hasher.finalize())
}

fn jkt_hash(jkt: &Jkt) -> [u8; 16] {
    first_16(&blake3::hash(jkt.as_str().as_bytes()))
}

/// § 4.5: `blake3` of the whole proof string as received, with no trim.
fn proof_digest(proof: &str) -> [u8; 32] {
    *blake3::hash(proof.as_bytes()).as_bytes()
}

/// The request checks, the window, the replay store and the follow-up (SMA-700 § 4.4 checks
/// 10-13, § 4.8). One instance per process behind an `Arc`; it shares the one replay store.
pub struct DpopProofVerifier {
    checker: Arc<dyn DpopProofChecker>,
    store: Arc<dyn ReplayStore>,
    clock: Arc<dyn Clock>,
    bases: Vec<ForwardedBase>,
    iat_window_secs: i64,
    refusal_log: LogRateLimiter<ProofDefect>,
    store_log: LogRateLimiter<StoreEvent>,
}

impl DpopProofVerifier {
    /// `IamConfig::validate` has already checked the list and the window, so an `Err` here is a
    /// wiring defect.
    pub fn new(checker: Arc<dyn DpopProofChecker>, store: Arc<dyn ReplayStore>, clock: Arc<dyn Clock>, forwarded_base_urls: &[String], iat_window_secs: u64) -> Result<Self, String> {
        let bases = forwarded_base_urls.iter().map(|raw| ForwardedBase::parse(raw)).collect::<Result<Vec<_>, _>>()?;
        Ok(DpopProofVerifier {
            checker,
            store,
            clock,
            bases,
            iat_window_secs: i64::try_from(iat_window_secs).map_err(|_| "authn.dpop.iat_window_secs is too large".to_string())?,
            refusal_log: LogRateLimiter::new(LOG_RATE_LIMIT_INTERVAL),
            store_log: LogRateLimiter::new(LOG_RATE_LIMIT_INTERVAL),
        })
    }

    /// § 4.8, before the token is verified (decision P5): the sizes, an empty proof, the
    /// forwarded path.
    pub fn check_request(&self, request: &DpopRequest) -> Result<(), AuthnError> {
        let defect = if request.proof.len() > MAX_PROOF_BYTES || request.method.len() > MAX_METHOD_BYTES || request.path.len() > MAX_PATH_BYTES {
            Some(ProofDefect::Malformed)
        } else if request.proof.is_empty() {
            Some(ProofDefect::Missing)
        } else if !path_is_well_formed(&request.path) {
            Some(ProofDefect::Malformed)
        } else {
            None
        };
        match defect {
            Some(defect) => Err(self.refuse(UNVERIFIED_ISSUER, defect)),
            None => Ok(()),
        }
    }

    /// Checks 1-13 for one `Introspect`. `claims` came from `authenticate(.., TokenScheme::Dpop)`.
    /// The replay record is last, so a proof that fails another check uses no `jti` (§ 4.4).
    pub fn verify(&self, claims: &ValidatedClaims, token: &str, request: &DpopRequest) -> Result<(), AuthnError> {
        let issuer = claims.issuer.as_str();
        let jkt = claims.key_binding.as_ref().ok_or(AuthnError::InvalidToken(TokenDefect::NotKeyBound))?;
        let proof = self.checker.check(&request.proof, token, jkt).map_err(|defect| self.refuse(issuer, defect))?;
        if proof.htm != request.method {
            return Err(self.refuse(issuer, ProofDefect::Htm)); // check 10
        }
        if !self.htu_matches(&proof.htu, &request.path) {
            return Err(self.refuse(issuer, ProofDefect::Htu)); // check 11
        }
        let now = self.now();
        let Some(expires_at) = self.iat_window_end(proof.iat, now) else {
            return Err(self.refuse(issuer, ProofDefect::Iat)); // check 12
        };
        let entry = NewProof {
            key: proof_key(jkt, &proof.jti),
            subject: subject_hash(claims),
            jkt: jkt_hash(jkt),
            expires_at,
            follow_up_deadline: expires_at.max(now.saturating_add(MIN_FOLLOW_UP_SECS)),
            follow_up_digest: proof_digest(&request.proof),
        };
        match self.store.record(entry, now) {
            RecordOutcome::Fresh => Ok(()), // check 13
            RecordOutcome::Replayed => Err(self.refuse(issuer, ProofDefect::Replayed)),
            RecordOutcome::QuotaExceeded { retry_after_secs } => {
                if let Some(suppressed) = self.store_log.admit_at(issuer, StoreEvent::Quota, Instant::now()) {
                    tracing::info!(issuer, retry_after_secs, suppressed, "refused a DPoP proof: a key or subject quota of the replay store is full");
                }
                Err(AuthnError::DpopQuotaExceeded { retry_after_secs })
            }
            RecordOutcome::CapacityFull { entries } => {
                if let Some(suppressed) = self.store_log.admit_at(issuer, StoreEvent::Capacity, Instant::now()) {
                    tracing::warn!(issuer, entries, suppressed, "the DPoP replay store is full; raise authn.dpop.replay_capacity");
                }
                Err(AuthnError::Unavailable)
            }
        }
    }

    /// The `IsAuthorized` follow-up (§ 4.8): `jti` and `ath` with no signature check, the `ath` of
    /// this token, then the one-time ticket for the digest of these exact bytes.
    pub fn redeem(&self, claims: &ValidatedClaims, token: &str, proof: &str) -> Result<(), AuthnError> {
        let issuer = claims.issuer.as_str();
        let jkt = claims.key_binding.as_ref().ok_or(AuthnError::InvalidToken(TokenDefect::NotKeyBound))?;
        if proof.is_empty() {
            return Err(self.refuse(issuer, ProofDefect::Missing));
        }
        let follow_up = self.checker.follow_up_claims(proof).map_err(|defect| self.refuse(issuer, defect))?;
        if !self.checker.ath_matches(&follow_up.ath, token) {
            return Err(self.refuse(issuer, ProofDefect::Ath)); // decision P6
        }
        match self.store.redeem_follow_up(proof_key(jkt, &follow_up.jti), proof_digest(proof), self.now()) {
            RedeemOutcome::Redeemed => Ok(()),
            RedeemOutcome::Refused => Err(self.refuse(issuer, ProofDefect::FollowUp)),
        }
    }

    fn now(&self) -> i64 {
        self.clock.now().timestamp()
    }

    /// Check 11: the proof matches when it matches one configured base.
    fn htu_matches(&self, htu: &str, path: &str) -> bool {
        let Ok(htu) = Url::parse(htu) else {
            return false;
        };
        self.bases.iter().filter_map(|base| base.expected(path)).any(|expected| same_target(&htu, &expected))
    }

    /// Check 12: `|now - iat| <= iat_window_secs` with checked arithmetic. `Some(iat + window)`
    /// (the entry's `expires_at`) on a pass.
    fn iat_window_end(&self, iat: i64, now: i64) -> Option<i64> {
        let skew = now.checked_sub(iat)?.checked_abs()?;
        (skew <= self.iat_window_secs).then(|| iat.saturating_add(self.iat_window_secs))
    }

    fn refuse(&self, issuer: &str, defect: ProofDefect) -> AuthnError {
        if let Some(suppressed) = self.refusal_log.admit_at(issuer, defect, Instant::now()) {
            tracing::info!(issuer, defect = defect.as_str(), suppressed, "refused a DPoP proof");
        }
        AuthnError::InvalidDpopProof(defect)
    }
}
```

- [ ] **Step 4: Run them to see them pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo fmt
cargo nextest run --locked -p paigasus-iam --lib -E 'test(application::dpop)'
cargo clippy --locked -p paigasus-iam --all-targets -- -D warnings
```

Expected: all pass, clippy clean. If `url` parses `https://gw.example.test/v1/chat/%63ompletions` to a path without the `%63`, STOP and report: § 4.6 relies on `Url::path()` keeping the percent-encoding.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git add rs/crates/services/paigasus-iam/src/application/dpop.rs rs/crates/services/paigasus-iam/src/application/mod.rs
git commit -m "feat(rs): DPoP proof verifier, checks 10 to 13 and the follow-up redeem (SMA-700)

The forwarded path check, htu against the configured base URLs with a
host check after the parse, the iat window with checked arithmetic, the
replay record last, and the one-time follow-up ticket. Refusals log the
issuer and a static defect name only.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: `AuthenticateToken::resolve_dpop`, the `Introspect` wiring, and the one shared store

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` (struct `:135-155`, `new` `:167-180`, `resolve` `:188-230`, new methods after `introspect` `:267-270`, tests)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs:56-66` (`introspect`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:796-805` (`AppState::new`)

**Interfaces:**
- Consumes: `DpopProofVerifier`, `DpopRequest` (Task 7); `JoseDpopProofChecker` (Task 4); `InMemoryReplayStore` (Task 5); `DpopConfig` (Task 6); `IntrospectRequest.dpop` (Task 1).
- Produces (`paigasus_iam::application::authenticate_token`):

```rust
pub enum DpopInput { Introspect(DpopRequest), FollowUp(String) }   // no Debug: it holds a proof
impl AuthenticateToken<…> {
    pub fn with_dpop(self, verifier: Arc<DpopProofVerifier>) -> Self;
    pub fn dpop_enabled(&self) -> bool;
    /// D18 order: request checks, authenticate(.., Dpop), proof check or redeem, THEN identity lookup.
    pub async fn resolve_dpop(&self, token: &str, input: DpopInput, provisioning: Provisioning) -> Result<AuthnPrincipal, AuthnError>;
    /// `resolve_dpop(.., Introspect, Disabled)` plus `context_for`.
    pub async fn introspect_dpop(&self, token: &str, request: DpopRequest) -> Result<PrincipalContext, AuthnError>;
}
```

DPoP off (`dpop_enabled() == false`): `resolve_dpop` answers `InvalidToken(Malformed)` (D11).

- [ ] **Step 1: Write the failing use-case tests**

In the `authenticate_token.rs` test module, add these imports beside the others:

```rust
    use crate::adapters::dpop_replay::InMemoryReplayStore;
    use crate::application::dpop::{DpopProofVerifier, DpopRequest};
    use paigasus_iam_core::{DpopProofChecker, FollowUpClaims, Jkt, ProofClaims, ProofDefect};
    use std::sync::atomic::AtomicUsize;
```

and these helpers and tests at the end of the module:

```rust
    // ---- SMA-700: resolve_dpop (D18) -------------------------------------------------------

    /// Checks 1-9 scripted: `Ok` gives a proof for `POST https://gw.example.test/v1/chat/completions`
    /// at the epoch (`FixedClock::default()`), so checks 10-13 pass.
    struct ScriptedChecker(Result<ProofClaims, ProofDefect>);

    impl DpopProofChecker for ScriptedChecker {
        fn check(&self, _proof: &str, _token: &str, _jkt: &Jkt) -> Result<ProofClaims, ProofDefect> {
            self.0.clone()
        }
        fn follow_up_claims(&self, _proof: &str) -> Result<FollowUpClaims, ProofDefect> {
            Err(ProofDefect::Malformed)
        }
        fn ath_matches(&self, _ath: &str, _token: &str) -> bool {
            false
        }
    }

    fn good_proof() -> Result<ProofClaims, ProofDefect> {
        Ok(ProofClaims {
            jti: "jti-1".into(),
            iat: 0,
            htm: "POST".into(),
            htu: "https://gw.example.test/v1/chat/completions".into(),
        })
    }

    fn verifier(check: Result<ProofClaims, ProofDefect>) -> Arc<DpopProofVerifier> {
        let bases = vec!["https://gw.example.test".to_string()];
        Arc::new(DpopProofVerifier::new(Arc::new(ScriptedChecker(check)), Arc::new(InMemoryReplayStore::new(10, 10, 10)), Arc::new(FixedClock::default()), &bases, 60).expect("verifier"))
    }

    fn dpop_request() -> DpopRequest {
        DpopRequest {
            proof: "p.r.oof".into(),
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
        }
    }

    fn bound_claims(subject: &str) -> ValidatedClaims {
        ValidatedClaims {
            key_binding: Some(Jkt::new("jkt-1")),
            ..claims("https://idp.example.com", subject, Some("x@example.com"), None)
        }
    }

    /// Counts calls to the identity port, so a test proves the proof check came first (D18).
    struct CountingIdentities {
        calls: Arc<AtomicUsize>,
        inner: InMemoryIdentities,
    }

    #[async_trait]
    impl ExternalIdentityRepository for CountingIdentities {
        async fn find_by_issuer_subject(&self, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inner.find_by_issuer_subject(issuer, subject).await
        }
        async fn provision(&self, principal: &Principal, user: &User, identity: &ExternalIdentity) -> Result<(), RepositoryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inner.provision(principal, user, identity).await
        }
    }

    fn dpop_use_case(
        claims: ValidatedClaims,
        calls: Arc<AtomicUsize>,
        store: AuthnStore,
        check: Result<ProofClaims, ProofDefect>,
    ) -> AuthenticateToken<FakeAuthenticator, CountingIdentities, InMemoryPrincipals, InMemoryMemberships, SeqIds, FixedClock> {
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        AuthenticateToken::new(
            FakeAuthenticator::ok(claims),
            CountingIdentities { calls, inner: InMemoryIdentities(store.clone()) },
            InMemoryPrincipals(store),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer, true)]),
        )
        .with_dpop(verifier(check))
    }

    #[tokio::test]
    async fn no_identity_lookup_runs_before_the_proof_check() {
        // D18 / AC 4, both directions: a bad proof makes NO identity call; a good proof makes one.
        let calls = Arc::new(AtomicUsize::new(0));
        let uc = dpop_use_case(bound_claims("sub-d1"), calls.clone(), AuthnStore::default(), Err(ProofDefect::Signature));
        let err = uc.resolve_dpop("token", DpopInput::Introspect(dpop_request()), Provisioning::Disabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidDpopProof(ProofDefect::Signature)), "got {err:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "the identity port must not run before the proof check");

        let calls = Arc::new(AtomicUsize::new(0));
        let uc = dpop_use_case(bound_claims("sub-d1"), calls.clone(), AuthnStore::default(), good_proof());
        let err = uc.resolve_dpop("token", DpopInput::Introspect(dpop_request()), Provisioning::Disabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "after a good proof, the lookup runs once");
    }

    #[tokio::test]
    async fn an_unprovisioned_identity_with_a_bad_proof_is_an_invalid_proof() {
        // § 5.1: InvalidDpopProof, not IdentityNotProvisioned — else a stolen token passes the
        // gateway's service-info, which accepts identity-not-provisioned (challenge 2).
        let uc = dpop_use_case(bound_claims("sub-unknown"), Arc::new(AtomicUsize::new(0)), AuthnStore::default(), Err(ProofDefect::Htu));
        let err = uc.introspect_dpop("token", dpop_request()).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidDpopProof(ProofDefect::Htu)), "got {err:?}");
    }

    #[tokio::test]
    async fn a_bad_request_is_refused_before_the_token_is_verified() {
        // Decision P5: the request checks run before `authenticate`.
        let uc = AuthenticateToken::new(
            PanicIfCalledAuthenticator,
            PanicIfCalledIdentities,
            PanicIfCalledPrincipals,
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[]),
        )
        .with_dpop(verifier(good_proof()));
        let request = DpopRequest { path: "x".into(), ..dpop_request() };
        let err = uc.resolve_dpop("token", DpopInput::Introspect(request), Provisioning::Disabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidDpopProof(ProofDefect::Malformed)), "got {err:?}");
    }

    #[tokio::test]
    async fn with_dpop_off_a_dpop_context_is_a_malformed_token() {
        // D11: the answer is the one a malformed token gets today.
        let uc = AuthenticateToken::new(
            PanicIfCalledAuthenticator,
            PanicIfCalledIdentities,
            PanicIfCalledPrincipals,
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[]),
        );
        assert!(!uc.dpop_enabled());
        let err = uc.resolve_dpop("token", DpopInput::Introspect(dpop_request()), Provisioning::Disabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Malformed)), "got {err:?}");
    }

    #[tokio::test]
    async fn a_provisioned_identity_with_a_good_proof_resolves() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let pid = seeded_principal(&store, &issuer, "sub-ok");
        let uc = dpop_use_case(bound_claims("sub-ok"), Arc::new(AtomicUsize::new(0)), store, good_proof());
        let ctx = uc.introspect_dpop("token", dpop_request()).await.expect("resolves");
        assert_eq!(ctx.principal.principal_id, pid);
    }
```

(`seeded_principal` exists at `:1131`; check its signature with `grep -n "fn seeded_principal" -A3 src/application/authenticate_token.rs` and pass what it takes. `Ordering` is already imported in that module through `std::sync::atomic::{AtomicBool, Ordering}`.)

- [ ] **Step 2: Run them to see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo nextest run --locked -p paigasus-iam --lib -E 'test(authenticate_token)'
```

Expected: compile errors `no method named with_dpop`, `cannot find type DpopInput`.

- [ ] **Step 3: Implement**

Imports at the top of `authenticate_token.rs`: add `TokenDefect` (and keep `TokenScheme` from Task 2) to the `use paigasus_iam_core::{…}` block, and add:

```rust
use crate::application::dpop::{DpopProofVerifier, DpopRequest};
```

Add a field to `AuthenticateToken` (after `not_provisioned_log`):

```rust
    /// SMA-700: the DPoP proof verifier, or `None` when `authn.dpop.enabled` is false. An `Arc`,
    /// so every `AppState` clone shares the one replay store behind it (D3).
    dpop: Option<Arc<DpopProofVerifier>>,
```

In `new`, add `dpop: None,` after `not_provisioned_log: …,`.

Above the `AuthenticateToken` struct, add:

```rust
/// What the DPoP scheme brings besides the token (SMA-700 § 4.7). No `Debug`: both arms hold a proof.
pub enum DpopInput {
    /// `Introspect` with a `dpop` context from the gateway.
    Introspect(DpopRequest),
    /// The `IsAuthorized` follow-up: the `dpop` metadata value, as received.
    FollowUp(String),
}
```

Split `resolve` into the token step and the rest. Replace its body with:

```rust
    pub async fn resolve(&self, token: &str, provisioning: Provisioning) -> Result<AuthnPrincipal, AuthnError> {
        let claims = self.authenticator.authenticate(token, TokenScheme::Bearer).await?;
        self.resolve_claims(claims, provisioning).await
    }

    /// Everything `resolve` does after the token is verified: the identity lookup, JIT
    /// provisioning, the principal read and its status. Shared by `resolve` and `resolve_dpop`.
    async fn resolve_claims(&self, claims: ValidatedClaims, provisioning: Provisioning) -> Result<AuthnPrincipal, AuthnError> {
        let principal_id = match self.identities.find_by_issuer_subject(&claims.issuer, &claims.subject).await.map_err(backend)? {
```

followed by the unchanged remainder of the old body (from `Some(identity) => identity.principal_id,` to the final `Ok(AuthnPrincipal { … })`), closing the new function.

After `introspect`, add:

```rust
    /// SMA-700: attaches the DPoP proof verifier. `AppState::new` calls this when
    /// `authn.dpop.enabled` is true.
    #[must_use]
    pub fn with_dpop(mut self, verifier: Arc<DpopProofVerifier>) -> Self {
        self.dpop = Some(verifier);
        self
    }

    /// Whether DPoP is on: `AuthEnforce` accepts the follow-up scheme only then.
    #[must_use]
    pub fn dpop_enabled(&self) -> bool {
        self.dpop.is_some()
    }

    /// The DPoP scheme (SMA-700 § 4.7). The order is fixed (D18): the request checks of § 4.8,
    /// `authenticate(.., Dpop)`, the proof check (`Introspect`) or the ticket redeem (the
    /// follow-up), and only then the identity lookup and anything after it. No identity lookup,
    /// provisioning, seeding or API-key branch runs before the proof check. With DPoP off, the
    /// answer is the one a malformed token gets (D11).
    pub async fn resolve_dpop(&self, token: &str, input: DpopInput, provisioning: Provisioning) -> Result<AuthnPrincipal, AuthnError> {
        let Some(verifier) = &self.dpop else {
            return Err(AuthnError::InvalidToken(TokenDefect::Malformed));
        };
        if let DpopInput::Introspect(request) = &input {
            verifier.check_request(request)?;
        }
        let claims = self.authenticator.authenticate(token, TokenScheme::Dpop).await?;
        match &input {
            DpopInput::Introspect(request) => verifier.verify(&claims, token, request)?,
            DpopInput::FollowUp(proof) => verifier.redeem(&claims, token, proof)?,
        }
        self.resolve_claims(claims, provisioning).await
    }

    /// `Introspect` with a `dpop` context (SMA-700 § 4.8): `resolve_dpop` with
    /// `Provisioning::Disabled` (D10 holds), plus `context_for`.
    pub async fn introspect_dpop(&self, token: &str, request: DpopRequest) -> Result<PrincipalContext, AuthnError> {
        let principal = self.resolve_dpop(token, DpopInput::Introspect(request), Provisioning::Disabled).await?;
        self.context_for(principal).await
    }
```

`adapters/grpc/authn.rs`, `introspect` (`:56-66`), replace the inner block with:

```rust
        let result: Result<Response<IntrospectResponse>, Status> = async {
            let request = request.into_inner();
            // SMA-700 § 4.8: no `dpop` context is the Bearer scheme, as before (D7). A context
            // is the DPoP scheme; with DPoP off it is refused as a malformed token (D11).
            let resolved = match request.dpop {
                None => self.state.authn.introspect(&request.token).await,
                Some(dpop) => {
                    let context = DpopRequest {
                        proof: dpop.proof,
                        method: dpop.method,
                        path: dpop.path,
                    };
                    self.state.authn.introspect_dpop(&request.token, context).await
                }
            };
            let ctx = resolved.map_err(|e| convert::authn_status(&e))?;
            Ok(Response::new(convert::to_introspect_response(&ctx)))
        }
        .await;
```

and add `use crate::application::dpop::DpopRequest;` to its imports. Extend the `introspect` doc: "SMA-700: a `dpop` context runs the DPoP scheme (`introspect_dpop`)."

`adapters/http/mod.rs`, `AppState::new`: replace `let authn = AuthenticateToken::new(…);` (`:796-805`) with:

```rust
        let authn = AuthenticateToken::new(
            authenticator,
            PgExternalIdentityRepository::new(db.clone()),
            PgPrincipalRepository::new(db.clone()),
            PgMembershipRepository::new(db.clone()),
            role_grant_store.clone(),
            KernelIdGenerator,
            SystemClock,
            JitPolicy::from_issuers(&jit_flags),
        );
        // SMA-700: ONE replay store for the whole process, shared by every `AppState` clone through
        // the verifier's `Arc` (D3). Correct only with one IAM replica (R2).
        let authn = if authn_cfg.dpop.enabled {
            let dpop = &authn_cfg.dpop;
            let verifier = DpopProofVerifier::new(
                Arc::new(JoseDpopProofChecker),
                Arc::new(InMemoryReplayStore::new(dpop.replay_capacity, dpop.per_key_quota, dpop.per_subject_quota)),
                Arc::new(SystemClock),
                &dpop.forwarded_base_urls,
                dpop.iat_window_secs,
            )
            .map_err(|e| AuthnError::Backend(e.into()))?;
            tracing::info!(
                forwarded_base_urls = dpop.forwarded_base_urls.len(),
                iat_window_secs = dpop.iat_window_secs,
                replay_capacity = dpop.replay_capacity,
                "DPoP is on: Introspect checks a forwarded DPoP proof, and IsAuthorized accepts the one follow-up"
            );
            authn.with_dpop(Arc::new(verifier))
        } else {
            authn
        };
```

with the imports:

```rust
use crate::adapters::dpop_replay::InMemoryReplayStore;
use crate::adapters::oidc::dpop::JoseDpopProofChecker;
use crate::application::dpop::DpopProofVerifier;
```

- [ ] **Step 4: Run them to see them pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo fmt
cargo nextest run --locked -p paigasus-iam --lib
cargo clippy --locked -p paigasus-iam --all-targets -- -D warnings
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test grpc_authn --test http_authn
```

Expected: all pass; the existing `grpc_authn` and `http_authn` suites are unchanged (DPoP off).

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git add rs/crates/services/paigasus-iam/src/application/authenticate_token.rs rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
git commit -m "feat(rs): resolve_dpop with the proof check before the identity lookup (SMA-700)

Introspect runs the DPoP scheme for a dpop context and refuses it as a
malformed token while DPoP is off. AppState wires one verifier with one
in-memory replay store when authn.dpop.enabled is true.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: The `IsAuthorized` follow-up in `AuthEnforce`, the self-query rule, and the header limit

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/auth.rs` (new items after `bearer_from_headers` `:33-44`; tests)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs` (`AuthEnforce::call` `:198-244`; a path constant; tests)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/authz.rs:96-124` (`is_authorized`) and a new helper with tests
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs` (`router` `:130-142`; a new pub fn)
- Modify: `rs/crates/services/paigasus-iam/src/main.rs:233-236`

**Interfaces:**
- Consumes: `AuthenticateToken::{dpop_enabled, resolve_dpop}`, `DpopInput::FollowUp` (Task 8).
- Produces:

```rust
// paigasus_iam::adapters::auth
pub fn dpop_token_from_headers(headers: &HeaderMap) -> Option<String>;   // `DPoP <token>`, scheme case-insensitive
pub enum DpopProofHeader { One(String), Missing, Invalid }               // Debug prints no proof
pub fn dpop_proof_from_headers(headers: &HeaderMap) -> DpopProofHeader;  // exactly one `dpop` entry, visible ASCII, not empty
#[derive(Debug, Clone, Copy)] pub struct DpopFollowUp;                   // request extension
// paigasus_iam::adapters::grpc::authn
pub static IS_AUTHORIZED_PATH: LazyLock<String>;                          // "/paigasus.iam.v1.AuthorizationService/IsAuthorized"
// paigasus_iam::adapters::grpc
pub fn grpc_max_header_list_size(max_token_bytes: usize) -> u32;          // max_token_bytes + 8192 + 4096
```

- [ ] **Step 1: Write the failing unit tests**

`adapters/auth.rs` test module, add:

```rust
    fn with_dpop(authorization: Option<&str>, proofs: &[&[u8]]) -> HeaderMap {
        let mut headers = headers(authorization);
        for proof in proofs {
            headers.append("dpop", HeaderValue::from_bytes(proof).unwrap());
        }
        headers
    }

    #[test]
    fn dpop_token_needs_the_dpop_scheme_in_any_case() {
        assert_eq!(dpop_token_from_headers(&headers(Some("DPoP abc"))).as_deref(), Some("abc"));
        assert_eq!(dpop_token_from_headers(&headers(Some("dpop abc"))).as_deref(), Some("abc"));
        assert_eq!(dpop_token_from_headers(&headers(Some("Bearer abc"))), None);
        assert_eq!(dpop_token_from_headers(&headers(Some("DPoP "))), None);
        assert_eq!(dpop_token_from_headers(&headers(Some("DPoPabc"))), None);
        assert_eq!(dpop_token_from_headers(&headers(None)), None);
        // The shared Bearer parser keeps refusing DPoP, so IAM's HTTP routes do too (§ 4.8).
        assert_eq!(bearer_from_headers(&headers(Some("DPoP abc"))), None);
    }

    #[test]
    fn the_proof_header_must_be_exactly_one_visible_ascii_value() {
        assert!(matches!(dpop_proof_from_headers(&with_dpop(None, &[b"a.b.c"])), DpopProofHeader::One(p) if p == "a.b.c"));
        assert!(matches!(dpop_proof_from_headers(&with_dpop(None, &[])), DpopProofHeader::Missing));
        assert!(matches!(dpop_proof_from_headers(&with_dpop(None, &[b"a.b.c", b"d.e.f"])), DpopProofHeader::Invalid));
        assert!(matches!(dpop_proof_from_headers(&with_dpop(None, &[b"a.\xffb.c"])), DpopProofHeader::Invalid));
        assert!(matches!(dpop_proof_from_headers(&with_dpop(None, &[b""])), DpopProofHeader::Invalid));
        assert_eq!(format!("{:?}", DpopProofHeader::One("secret".into())), "One(..)");
    }
```

`adapters/grpc/authn.rs` test module, add:

```rust
    /// SMA-700 § 4.8: tonic generates only the service name, so the path is built once and pinned
    /// here. A rename of the service or the RPC reds this test, not the follow-up in production.
    #[test]
    fn the_is_authorized_path_is_pinned() {
        assert_eq!(IS_AUTHORIZED_PATH.as_str(), "/paigasus.iam.v1.AuthorizationService/IsAuthorized");
    }
```

`adapters/grpc/authz.rs`: add a test module at the end of the file (it has none today; check with `grep -n "mod tests" src/adapters/grpc/authz.rs`, and if one exists, add the tests to it):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn prn(id: u128) -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(id)).unwrap()
    }

    #[test]
    fn a_follow_up_must_be_a_self_query() {
        // SMA-700 § 4.8 / § 5.1: a self-query passes, another principal_prn is refused.
        assert!(follow_up_self_query(true, &prn(1), &prn(1)).is_ok());
        let status = follow_up_self_query(true, &prn(1), &prn(2)).unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
        let details = tonic_types::StatusExt::get_error_details(&status);
        assert_eq!(details.error_info().expect("ErrorInfo").reason, "invalid-dpop-proof");
        // No follow-up: the exposure rule of `decide_gated` decides, as before.
        assert!(follow_up_self_query(false, &prn(1), &prn(2)).is_ok());
    }
}
```

`adapters/grpc/mod.rs` test module, add:

```rust
    #[test]
    fn the_header_list_fits_a_maximal_token_and_proof() {
        // SMA-700 § 4.8: the follow-up carries the token AND the proof in metadata.
        assert_eq!(super::grpc_max_header_list_size(16_384), 16_384 + 8_192 + 4_096);
        assert_eq!(super::grpc_max_header_list_size(usize::MAX), u32::MAX);
    }
```

- [ ] **Step 2: Run them to see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo nextest run --locked -p paigasus-iam --lib -E 'test(adapters::auth) | test(adapters::grpc)'
```

Expected: compile errors for the four new names.

- [ ] **Step 3: Implement the header helpers in `adapters/auth.rs`**

After `bearer_from_headers`, add:

```rust
/// The `DPoP` scheme's token (SMA-700 § 4.8): `Authorization: DPoP <token>`, scheme
/// ASCII-case-insensitive, token trimmed and not empty. Only `AuthEnforce` reads it, and only for
/// the `IsAuthorized` follow-up while DPoP is on. `bearer_from_headers` keeps refusing the scheme,
/// so every other gRPC route and every HTTP route refuse it as before.
pub fn dpop_token_from_headers(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("DPoP") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// The `dpop` request header (gRPC metadata) of the follow-up. `Debug` prints no proof.
pub enum DpopProofHeader {
    One(String),
    Missing,
    /// Two or more entries, a value that is not visible ASCII, or an empty value.
    Invalid,
}

impl std::fmt::Debug for DpopProofHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DpopProofHeader::One(_) => "One(..)",
            DpopProofHeader::Missing => "Missing",
            DpopProofHeader::Invalid => "Invalid",
        })
    }
}

/// The `dpop` header: exactly one entry (§ 4.8).
pub fn dpop_proof_from_headers(headers: &HeaderMap) -> DpopProofHeader {
    let mut values = headers.get_all("dpop").iter();
    let Some(first) = values.next() else {
        return DpopProofHeader::Missing;
    };
    if values.next().is_some() {
        return DpopProofHeader::Invalid;
    }
    match first.to_str() {
        Ok(proof) if !proof.is_empty() => DpopProofHeader::One(proof.to_string()),
        _ => DpopProofHeader::Invalid,
    }
}

/// Marks a request that `AuthEnforce` admitted as the DPoP `IsAuthorized` follow-up (SMA-700
/// § 4.8). `grpc::authz::is_authorized` then requires a self-query.
#[derive(Debug, Clone, Copy)]
pub struct DpopFollowUp;
```

- [ ] **Step 4: Implement the follow-up in `AuthEnforce`**

In `adapters/grpc/authn.rs`, add imports:

```rust
use std::sync::LazyLock;

use paigasus_iam_core::ProofDefect;
use paigasus_proto::paigasus::iam::v1::authorization_service_server;

use crate::adapters::auth::{DpopFollowUp, DpopProofHeader, dpop_proof_from_headers, dpop_token_from_headers};
use crate::application::authenticate_token::DpopInput;
```

(merge them with the existing `use` lines; `ProofDefect` joins `paigasus_iam_core::{AuthnError, AuthnPrincipal, Credential, TokenDefect}`).

After `is_exempt`, add:

```rust
/// The one gRPC path that accepts the DPoP follow-up (SMA-700 § 4.8). tonic generates only the
/// service name, so the method is appended here; `the_is_authorized_path_is_pinned` pins it.
pub static IS_AUTHORIZED_PATH: LazyLock<String> = LazyLock::new(|| format!("/{}/IsAuthorized", authorization_service_server::SERVICE_NAME));
```

In `call`, replace the `let Some(token) = bearer_from_headers(req.headers()) else { … };` statement with:

```rust
            let Some(token) = bearer_from_headers(req.headers()) else {
                // SMA-700 § 4.8: the DPoP scheme only for the IsAuthorized follow-up while DPoP is
                // on. It never reaches the API-key branch or the bootstrap seeder, and it resolves
                // with Provisioning::Disabled: Introspect required the identity on the chat path.
                if state.authn.dpop_enabled()
                    && req.uri().path() == IS_AUTHORIZED_PATH.as_str()
                    && let Some(token) = dpop_token_from_headers(req.headers())
                {
                    let proof = match dpop_proof_from_headers(req.headers()) {
                        DpopProofHeader::One(proof) => proof,
                        DpopProofHeader::Missing => return Ok(reject(&AuthnError::InvalidDpopProof(ProofDefect::Missing))),
                        DpopProofHeader::Invalid => return Ok(reject(&AuthnError::InvalidDpopProof(ProofDefect::Malformed))),
                    };
                    return match state.authn.resolve_dpop(&token, DpopInput::FollowUp(proof), Provisioning::Disabled).await {
                        Ok(principal) => {
                            req.extensions_mut().insert(AuthContext {
                                principal_id: principal.principal_id,
                                kind: principal.kind,
                                status: principal.status,
                                credential: principal.credential,
                            });
                            req.extensions_mut().insert(DpopFollowUp);
                            inner.call(req).await
                        }
                        Err(err) => Ok(reject(&err)),
                    };
                }
                // A missing or malformed `authorization` header is treated exactly like a rejected
                // token (D12): both are `Unauthenticated`. The DPoP scheme on any other path, or
                // with DPoP off, lands here too (§ 4.8, D11).
                return Ok(reject(&AuthnError::InvalidToken(TokenDefect::Malformed)));
            };
```

Extend the doc of `AuthEnforce`: "SMA-700: with DPoP on, `IsAuthorized` alone also accepts the one-time follow-up (`authorization: DPoP <token>` plus `dpop: <proof>`), resolved with `resolve_dpop(.., FollowUp, Disabled)` and marked with a `DpopFollowUp` extension."

- [ ] **Step 5: The self-query rule in `grpc/authz.rs`**

Add the imports `use paigasus_iam_core::{AuthnError, ProofDefect};` (merge with the existing `paigasus_iam_core` line) and `use crate::adapters::auth::DpopFollowUp;`. Before `#[tonic::async_trait] impl AuthorizationService for AuthzGrpc`, add:

```rust
/// SMA-700 § 4.8: a DPoP follow-up answers only a self-query. The gateway always self-queries,
/// so this costs nothing; it binds the one-time ticket to the question.
fn follow_up_self_query(is_follow_up: bool, actor: &Prn, principal: &Prn) -> Result<(), Status> {
    if is_follow_up && principal != actor {
        return Err(convert::authn_status(&AuthnError::InvalidDpopProof(ProofDefect::FollowUp)));
    }
    Ok(())
}
```

In `is_authorized`, change the first two statements of the inner block to:

```rust
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let is_follow_up = request.extensions().get::<DpopFollowUp>().is_some();
            let req = request.into_inner();
```

and after `let principal = parse_prn(&req.principal_prn).map_err(convert::status_to_grpc)?;` add:

```rust
            follow_up_self_query(is_follow_up, &actor, &principal)?;
```

- [ ] **Step 6: The header list size**

In `adapters/grpc/mod.rs`, after `routes`, add:

```rust
/// The HTTP/2 header-list limit of IAM's gRPC server (SMA-700 § 4.8): the DPoP follow-up carries
/// the token (`max_token_bytes`) and the proof (8192 bytes) in metadata, plus 4096 bytes for the
/// other headers. The hyper default (16 KiB) is too small for that.
#[must_use]
pub fn grpc_max_header_list_size(max_token_bytes: usize) -> u32 {
    u32::try_from(max_token_bytes.saturating_add(8192).saturating_add(4096)).unwrap_or(u32::MAX)
}
```

In `router` (the test router), add after `.timeout(timeout)`:

```rust
        .http2_max_header_list_size(grpc_max_header_list_size(state.grpc_max_token_bytes))
```

and add to `AppState` (in `adapters/http/mod.rs`, after `introspect_body_limit`):

```rust
    /// `authn.max_token_bytes`, for the gRPC header-list limit of the test router (SMA-700);
    /// `main.rs` reads the config directly.
    pub grpc_max_token_bytes: usize,
```

set in `AppState::new` as `grpc_max_token_bytes: cfg.authn.max_token_bytes,`.

In `main.rs`, the gRPC `Server::builder()` (`:233-236`) becomes:

```rust
            tonic::transport::Server::builder()
                .timeout(request_timeout)
                // SMA-700 § 4.8: room for the DPoP follow-up's token and proof metadata.
                .http2_max_header_list_size(grpc::grpc_max_header_list_size(max_token_bytes))
                .layer(paigasus_observability::CorrelationLayer)
```

with `let max_token_bytes = config.authn.max_token_bytes;` added before the block that spawns it (the closure moves `routes`, `grpc_incoming` and `rx`; a `usize` copy is enough). Check `grpc` is the imported module name in `main.rs` (`grep -n "use paigasus_iam::adapters::grpc\|grpc::health_service" src/main.rs`).

- [ ] **Step 7: Run everything**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo fmt
cargo nextest run --locked -p paigasus-iam --lib
cargo clippy --locked -p paigasus-iam --all-targets -- -D warnings
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test grpc_authn --test grpc_whoami --test grpc_authz
```

Expected: all pass (DPoP is off in every existing suite, so their answers do not change).

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git add rs/crates/services/paigasus-iam/src
git commit -m "feat(rs): the DPoP IsAuthorized follow-up in AuthEnforce (SMA-700)

With DPoP on, IsAuthorized alone accepts authorization: DPoP plus one
dpop metadata entry, redeems the ticket with no seeding and no API-key
branch, and answers only a self-query. The gRPC server takes a header
list that fits the token and the proof.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: IAM integration tests over real gRPC and HTTP, and the Keycloak extension

All suites here are Docker-backed. Run them with `PAIGASUS_REQUIRE_DOCKER=1`.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs` (`MockIdp::bearer` `:181-194`; new helpers after `test_config_with` `:498`; imports `:26-58`)
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_authn.rs` (imports `:16-30`; new tests at the end)
- Modify: `rs/crates/services/paigasus-iam/tests/http_authn.rs` (a new test after `present_but_malformed_authorization_keeps_error_challenge` `:218`)
- Modify: `rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs` (`keycloak_config` `:287-327`; `dpop_proof` `:356-366`; the token request `:180`; new assertions after the second introspect `:281`)

**Interfaces:**
- Consumes: everything from Tasks 1-9.
- Produces (test support only, `#[allow(dead_code)]` like the other helpers):

```rust
impl MockIdp {
    pub fn bearer_with(&self, sub: &str, email: Option<&str>, aud: &str, exp_offset_secs: i64, extra: Value) -> String;
    pub fn bound_bearer(&self, sub: &str, email: Option<&str>, aud: &str, exp_offset_secs: i64, jkt: &str) -> String;
}
pub const GATEWAY_BASE: &str = "https://gw.example.test";
pub const CHAT_URL: &str = "https://gw.example.test/v1/chat/completions";
pub const CHAT_PATH: &str = "/v1/chat/completions";
pub fn test_config_dpop(idp: &MockIdp) -> IamConfig;
pub struct DpopKey;   impl DpopKey { pub fn generate() -> Self; pub fn jkt(&self) -> String; pub fn proof(&self, htm: &str, htu: &str, token: &str, jti: &str, extra: Value) -> String; }
pub fn dpop_introspect(token: &str, proof: &str) -> IntrospectRequest;
pub fn dpop_follow_up<T>(message: T, token: &str, proof: &str) -> tonic::Request<T>;
pub fn reason_of(status: &tonic::Status) -> String;
```

- [ ] **Step 1: The support helpers**

In `tests/support/mod.rs`, add imports:

```rust
use paigasus_iam::config::DpopConfig;
use paigasus_proto::paigasus::iam::v1::{DpopContext, IntrospectRequest};
use sha2::{Digest, Sha256};
```

(`DpopConfig` joins the existing `use paigasus_iam::config::{…}` line.) Replace `MockIdp::bearer` (`:181-194`) with:

```rust
    pub fn bearer(&self, sub: &str, email: Option<&str>, aud: &str, exp_offset_secs: i64) -> String {
        self.bearer_with(sub, email, aud, exp_offset_secs, serde_json::json!({}))
    }

    /// [`bearer`](Self::bearer) plus the members of `extra` (an object) in the payload — SMA-700
    /// uses it for `typ`, `cnf` and a size pad.
    pub fn bearer_with(&self, sub: &str, email: Option<&str>, aud: &str, exp_offset_secs: i64, extra: Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.kid.clone());
        let mut claims = serde_json::json!({
            "iss": self.issuer,
            "sub": sub,
            "aud": aud,
            "exp": chrono::Utc::now().timestamp() + exp_offset_secs,
        });
        if let Some(email) = email {
            claims["email"] = Value::String(email.to_string());
        }
        if let (Some(claims), Value::Object(extra)) = (claims.as_object_mut(), extra) {
            claims.extend(extra);
        }
        jsonwebtoken::encode(&header, &claims, &self.sign).expect("signing a test token")
    }

    /// A DPoP-bound token (SMA-700), in the Keycloak shape: `typ: DPoP` and `cnf.jkt`.
    pub fn bound_bearer(&self, sub: &str, email: Option<&str>, aud: &str, exp_offset_secs: i64, jkt: &str) -> String {
        self.bearer_with(sub, email, aud, exp_offset_secs, serde_json::json!({ "typ": "DPoP", "cnf": { "jkt": jkt } }))
    }
```

After `test_config_with`, add:

```rust
/// The public gateway origin that the DPoP tests configure in `forwarded_base_urls` (SMA-700).
#[allow(dead_code)]
pub const GATEWAY_BASE: &str = "https://gw.example.test";
/// The `htu` of a chat request through [`GATEWAY_BASE`].
#[allow(dead_code)]
pub const CHAT_URL: &str = "https://gw.example.test/v1/chat/completions";
/// The path that the gateway forwards for [`CHAT_URL`].
#[allow(dead_code)]
pub const CHAT_PATH: &str = "/v1/chat/completions";

/// [`test_config`] with `[authn.dpop]` on for [`GATEWAY_BASE`] (SMA-700), the other DPoP values
/// at their defaults.
#[allow(dead_code)]
pub fn test_config_dpop(idp: &MockIdp) -> IamConfig {
    let mut cfg = test_config(idp);
    cfg.authn.dpop = DpopConfig {
        enabled: true,
        forwarded_base_urls: vec![GATEWAY_BASE.to_string()],
        ..DpopConfig::default()
    };
    cfg
}

/// A client's DPoP key (SMA-700): an ES256 key, its public JWK members and its RFC 7638
/// thumbprint. The proof shape follows `keycloak_e2e.rs::dpop_proof`, plus `ath`.
#[allow(dead_code)]
pub struct DpopKey {
    sign: EncodingKey,
    x: String,
    y: String,
}

#[allow(dead_code)]
impl DpopKey {
    pub fn generate() -> Self {
        let secret = p256::SecretKey::generate();
        let pem = secret.to_pkcs8_pem(LineEnding::LF).expect("valid pkcs8 pem");
        let point = secret.public_key().to_sec1_point(false);
        DpopKey {
            sign: EncodingKey::from_ec_pem(pem.as_bytes()).expect("valid ec pem"),
            x: URL_SAFE_NO_PAD.encode(point.x().expect("uncompressed point has x")),
            y: URL_SAFE_NO_PAD.encode(point.y().expect("uncompressed point has y")),
        }
    }

    /// The RFC 7638 thumbprint: the `cnf.jkt` of a token bound to this key.
    pub fn jkt(&self) -> String {
        let canonical = format!(r#"{{"crv":"P-256","kty":"EC","x":"{}","y":"{}"}}"#, self.x, self.y);
        URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes()))
    }

    /// A signed proof for one request to `htu`, bound to `token` by `ath`, with `iat` now. The
    /// members of `extra` (an object) join the payload, for a size pad.
    pub fn proof(&self, htm: &str, htu: &str, token: &str, jti: &str, extra: Value) -> String {
        let header = serde_json::json!({ "typ": "dpop+jwt", "alg": "ES256", "jwk": { "kty": "EC", "crv": "P-256", "x": self.x, "y": self.y } });
        let mut payload = serde_json::json!({
            "jti": jti,
            "htm": htm,
            "htu": htu,
            "iat": chrono::Utc::now().timestamp(),
            "ath": URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes())),
        });
        if let (Some(payload), Value::Object(extra)) = (payload.as_object_mut(), extra) {
            payload.extend(extra);
        }
        let message = format!("{}.{}", URL_SAFE_NO_PAD.encode(header.to_string()), URL_SAFE_NO_PAD.encode(payload.to_string()));
        let signature = jsonwebtoken::crypto::sign(message.as_bytes(), &self.sign, Algorithm::ES256).expect("sign the DPoP proof");
        format!("{message}.{signature}")
    }
}

/// An `Introspect` request as the gateway sends it for `POST /v1/chat/completions` (SMA-700).
#[allow(dead_code)]
pub fn dpop_introspect(token: &str, proof: &str) -> IntrospectRequest {
    IntrospectRequest {
        token: token.to_string(),
        dpop: Some(DpopContext {
            proof: proof.to_string(),
            method: "POST".to_string(),
            path: CHAT_PATH.to_string(),
        }),
    }
}

/// The gateway's `IsAuthorized` follow-up metadata (SMA-700 § 4.8): `authorization: DPoP <token>`
/// and `dpop: <proof>`.
#[allow(dead_code)]
pub fn dpop_follow_up<T>(message: T, token: &str, proof: &str) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    request.metadata_mut().insert("authorization", format!("DPoP {token}").parse().expect("ascii metadata"));
    request.metadata_mut().insert("dpop", proof.parse().expect("ascii metadata"));
    request
}

/// The `ErrorInfo` reason of an IAM status.
#[allow(dead_code)]
pub fn reason_of(status: &tonic::Status) -> String {
    let details = tonic_types::StatusExt::get_error_details(status);
    details.error_info().expect("every IAM status carries ErrorInfo").reason.clone()
}
```

- [ ] **Step 2: The gRPC sequence tests**

In `tests/grpc_authn.rs`, extend the imports:

```rust
use paigasus_proto::paigasus::iam::v1::authorization_service_client::AuthorizationServiceClient;
use paigasus_proto::paigasus::iam::v1::{AttachMembershipRequest, CreateOrganizationRequest, IntrospectRequest, IsAuthorizedRequest, WhoAmIRequest};
use sea_orm::DatabaseConnection;
use serde_json::json;
```

and add at the end of the file:

```rust
// ---- SMA-700: the DPoP gateway path ------------------------------------------------------------

/// IAM with DPoP on, on an ephemeral port. The `AppState` clone lets a test provision first.
async fn dpop_server(db: DatabaseConnection) -> (SocketAddr, JoinHandle<()>, support::MockIdp, AppState) {
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config_dpop(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state.clone()).await;
    (addr, server, idp, state)
}

/// The gateway's self-query: the caller's own principal, InvokeModel, at Root.
fn self_query(principal_prn: &str) -> IsAuthorizedRequest {
    IsAuthorizedRequest {
        principal_prn: principal_prn.to_string(),
        action: "InvokeModel".to_string(),
        resource_prn: root_prn().canonical(),
        context: Default::default(),
    }
}

/// Provisions `sub` with a plain bearer (the precondition of § 2) and returns its PRN.
async fn provisioned(state: &AppState, idp: &support::MockIdp, sub: &str) -> String {
    support::provision(state, &idp.bearer(sub, Some(&format!("{sub}@example.com")), "paigasus", 3600)).await
}

fn bound(idp: &support::MockIdp, sub: &str, key: &support::DpopKey) -> String {
    idp.bound_bearer(sub, Some(&format!("{sub}@example.com")), "paigasus", 3600, &key.jkt())
}

#[tokio::test]
async fn a_dpop_gateway_sequence_passes_once_and_never_again() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (addr, server, idp, state) = dpop_server(db).await;
    let principal_prn = provisioned(&state, &idp, "dpop-alice").await;
    let key = support::DpopKey::generate();
    let token = bound(&idp, "dpop-alice", &key);
    let proof = key.proof("POST", support::CHAT_URL, &token, "jti-sequence", json!({}));
    let ch = channel(addr).await;
    let mut authn = AuthnServiceClient::new(ch.clone());
    let mut authz = AuthorizationServiceClient::new(ch);

    // AC 1: Introspect with the context, then the follow-up. Both pass.
    let ctx = authn.introspect(support::dpop_introspect(&token, &proof)).await.expect("Introspect with a valid proof").into_inner();
    assert_eq!(ctx.principal_prn, principal_prn);
    authz.is_authorized(support::dpop_follow_up(self_query(&principal_prn), &token, &proof)).await.expect("the follow-up passes");

    // AC 3: the same Introspect again is a replay, and the follow-up works once.
    let replay = authn.introspect(support::dpop_introspect(&token, &proof)).await.unwrap_err();
    assert_eq!(replay.code(), Code::Unauthenticated, "{replay:?}");
    assert_eq!(support::reason_of(&replay), "invalid-dpop-proof");
    let twice = authz.is_authorized(support::dpop_follow_up(self_query(&principal_prn), &token, &proof)).await.unwrap_err();
    assert_eq!(support::reason_of(&twice), "invalid-dpop-proof");

    server.abort();
}

#[tokio::test]
async fn a_follow_up_with_another_token_of_the_same_key_is_refused() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (addr, server, idp, state) = dpop_server(db).await;
    let principal_prn = provisioned(&state, &idp, "dpop-ath").await;
    let key = support::DpopKey::generate();
    let token_a = idp.bound_bearer("dpop-ath", Some("dpop-ath@example.com"), "paigasus", 3600, &key.jkt());
    let token_b = idp.bound_bearer("dpop-ath", Some("dpop-ath@example.com"), "paigasus", 3599, &key.jkt());
    assert_ne!(token_a, token_b);
    let proof = key.proof("POST", support::CHAT_URL, &token_a, "jti-ath", json!({}));
    let ch = channel(addr).await;
    AuthnServiceClient::new(ch.clone()).introspect(support::dpop_introspect(&token_a, &proof)).await.expect("Introspect");
    let err = AuthorizationServiceClient::new(ch).is_authorized(support::dpop_follow_up(self_query(&principal_prn), &token_b, &proof)).await.unwrap_err();
    assert_eq!(support::reason_of(&err), "invalid-dpop-proof", "the ath of the proof names token_a");
    server.abort();
}

#[tokio::test]
async fn a_follow_up_for_another_principal_is_refused() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (addr, server, idp, state) = dpop_server(db).await;
    provisioned(&state, &idp, "dpop-self").await;
    let other_prn = provisioned(&state, &idp, "dpop-other").await;
    let key = support::DpopKey::generate();
    let token = bound(&idp, "dpop-self", &key);
    let proof = key.proof("POST", support::CHAT_URL, &token, "jti-other", json!({}));
    let ch = channel(addr).await;
    AuthnServiceClient::new(ch.clone()).introspect(support::dpop_introspect(&token, &proof)).await.expect("Introspect");
    let err = AuthorizationServiceClient::new(ch).is_authorized(support::dpop_follow_up(self_query(&other_prn), &token, &proof)).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");
    assert_eq!(support::reason_of(&err), "invalid-dpop-proof", "the follow-up answers a self-query only");
    server.abort();
}

#[tokio::test]
async fn a_follow_up_on_another_rpc_is_an_invalid_token() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (addr, server, idp, state) = dpop_server(db).await;
    provisioned(&state, &idp, "dpop-whoami").await;
    let key = support::DpopKey::generate();
    let token = bound(&idp, "dpop-whoami", &key);
    let proof = key.proof("POST", support::CHAT_URL, &token, "jti-whoami", json!({}));
    let mut authn = AuthnServiceClient::new(channel(addr).await);
    // A live ticket exists, so the refusal is the path rule, not a missing ticket.
    authn.introspect(support::dpop_introspect(&token, &proof)).await.expect("Introspect");
    let err = authn.who_am_i(support::dpop_follow_up(WhoAmIRequest {}, &token, &proof)).await.unwrap_err();
    assert_eq!(support::reason_of(&err), "invalid-token", "the DPoP scheme is accepted on IsAuthorized only");
    server.abort();
}

#[tokio::test]
async fn a_follow_up_with_no_introspect_is_refused() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (addr, server, idp, state) = dpop_server(db).await;
    let principal_prn = provisioned(&state, &idp, "dpop-noticket").await;
    let key = support::DpopKey::generate();
    let token = bound(&idp, "dpop-noticket", &key);
    let proof = key.proof("POST", support::CHAT_URL, &token, "jti-noticket", json!({}));
    let err = AuthorizationServiceClient::new(channel(addr).await)
        .is_authorized(support::dpop_follow_up(self_query(&principal_prn), &token, &proof))
        .await
        .unwrap_err();
    assert_eq!(support::reason_of(&err), "invalid-dpop-proof");
    server.abort();
}

#[tokio::test]
async fn an_unprovisioned_identity_with_a_bad_proof_is_an_invalid_proof() {
    // AC 4 / D18 over the wire: the proof check runs before the identity lookup.
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (addr, server, idp, _state) = dpop_server(db).await;
    let key = support::DpopKey::generate();
    let token = bound(&idp, "dpop-nobody", &key);
    let mut authn = AuthnServiceClient::new(channel(addr).await);
    let bad = key.proof("POST", "https://elsewhere.example.test/v1/chat/completions", &token, "jti-nobody-1", json!({}));
    let err = authn.introspect(support::dpop_introspect(&token, &bad)).await.unwrap_err();
    assert_eq!(support::reason_of(&err), "invalid-dpop-proof", "not identity-not-provisioned");
    // The control: with a good proof, the same identity reaches the lookup.
    let good = key.proof("POST", support::CHAT_URL, &token, "jti-nobody-2", json!({}));
    let err = authn.introspect(support::dpop_introspect(&token, &good)).await.unwrap_err();
    assert_eq!(support::reason_of(&err), "identity-not-provisioned");
    server.abort();
}

#[tokio::test]
async fn a_proof_for_a_base_url_that_is_not_configured_is_refused() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (addr, server, idp, state) = dpop_server(db).await;
    provisioned(&state, &idp, "dpop-htu").await;
    let key = support::DpopKey::generate();
    let token = bound(&idp, "dpop-htu", &key);
    let proof = key.proof("POST", "https://gw.example.test:8443/v1/chat/completions", &token, "jti-htu", json!({}));
    let err = AuthnServiceClient::new(channel(addr).await).introspect(support::dpop_introspect(&token, &proof)).await.unwrap_err();
    assert_eq!(support::reason_of(&err), "invalid-dpop-proof");
    server.abort();
}

#[tokio::test]
async fn with_dpop_off_a_dpop_context_is_an_invalid_token() {
    // D11: the same answer as for a malformed token.
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    provisioned(&state, &idp, "dpop-off").await;
    let (addr, server) = spawn_server(state).await;
    let key = support::DpopKey::generate();
    let token = bound(&idp, "dpop-off", &key);
    let proof = key.proof("POST", support::CHAT_URL, &token, "jti-off", json!({}));
    let err = AuthnServiceClient::new(channel(addr).await).introspect(support::dpop_introspect(&token, &proof)).await.unwrap_err();
    assert_eq!((err.code(), support::reason_of(&err).as_str()), (Code::Unauthenticated, "invalid-token"));
    server.abort();
}

#[tokio::test]
async fn a_bound_token_with_no_dpop_context_is_an_invalid_token() {
    // D7: with DPoP on, the Bearer scheme still refuses a bound token.
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (addr, server, idp, state) = dpop_server(db).await;
    provisioned(&state, &idp, "dpop-bearer").await;
    let token = bound(&idp, "dpop-bearer", &support::DpopKey::generate());
    let err = AuthnServiceClient::new(channel(addr).await).introspect(IntrospectRequest { token, dpop: None }).await.unwrap_err();
    assert_eq!(support::reason_of(&err), "invalid-token");
    server.abort();
}

#[tokio::test]
async fn a_large_token_and_a_large_proof_pass_the_transport() {
    // § 4.8: the follow-up metadata holds a token near max_token_bytes and a proof near 8192 bytes.
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (addr, server, idp, state) = dpop_server(db).await;
    let principal_prn = provisioned(&state, &idp, "dpop-big").await;
    let key = support::DpopKey::generate();
    let token = idp.bearer_with(
        "dpop-big",
        Some("dpop-big@example.com"),
        "paigasus",
        3600,
        json!({ "typ": "DPoP", "cnf": { "jkt": key.jkt() }, "pad": "x".repeat(11_500) }),
    );
    assert!((15_000..=16_384).contains(&token.len()), "token is {} bytes", token.len());
    let proof = key.proof("POST", support::CHAT_URL, &token, "jti-big", json!({ "pad": "y".repeat(5_500) }));
    assert!((7_500..=8_192).contains(&proof.len()), "proof is {} bytes", proof.len());
    let ch = channel(addr).await;
    AuthnServiceClient::new(ch.clone()).introspect(support::dpop_introspect(&token, &proof)).await.expect("Introspect");
    AuthorizationServiceClient::new(ch)
        .is_authorized(support::dpop_follow_up(self_query(&principal_prn), &token, &proof))
        .await
        .expect("the follow-up metadata fits the header list");
    server.abort();
}
```

- [ ] **Step 3: The HTTP refusal test**

In `tests/http_authn.rs`, after `present_but_malformed_authorization_keeps_error_challenge`, add:

```rust
#[tokio::test]
async fn the_dpop_scheme_is_still_refused_on_http_with_dpop_on() {
    // SMA-700 § 4.8: IAM's own HTTP API keeps refusing the DPoP scheme; only the gRPC
    // IsAuthorized follow-up accepts it.
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = start_mock_idp().await;
    let (app, state) = support::app_with_config(db, &support::test_config_dpop(&idp)).await;
    support::provision_platform_admin(&state, &idp.bearer("http-dpop", Some("http-dpop@example.com"), "paigasus", 3600)).await;
    let token = idp.bound_bearer("http-dpop", Some("http-dpop@example.com"), "paigasus", 3600, &support::DpopKey::generate().jkt());
    let response = send_raw_parts(&app, "GET", "/v1/organizations", Some(&format!("DPoP {token}")), None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let challenge = response.headers().get("www-authenticate").expect("WWW-Authenticate header").to_str().unwrap();
    assert_eq!(challenge, "Bearer error=\"invalid_token\"");
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "invalid-token");
}
```

- [ ] **Step 4: The Keycloak extension**

In `tests/keycloak_e2e.rs`:

1. Imports: add `DpopConfig` to the `use paigasus_iam::config::{…}` line, and add `use paigasus_iam::application::dpop::DpopRequest;`.
2. `keycloak_config`: after `extra_ca_bundle_path: None,` set DPoP on (Task 6 added `dpop: DpopConfig::default(),` there):

```rust
            // SMA-700: DPoP on. The Bearer assertions below do not change (D7).
            dpop: DpopConfig {
                enabled: true,
                forwarded_base_urls: vec!["https://gw.example.test".to_string()],
                ..DpopConfig::default()
            },
```

3. `dpop_proof` gets an `ath` input. Replace it with:

```rust
/// A DPoP proof (RFC 9449 § 4.2) for one request: header `typ: dpop+jwt`, `alg: ES256` and the
/// public `jwk`; payload `jti`, `htm`, `htu`, `iat`, and `ath` when `token` is given (a proof that
/// a resource server checks, SMA-700). The token endpoint gets no `ath`.
fn dpop_proof(key: &EncodingKey, x: &str, y: &str, htm: &str, htu: &str, token: Option<&str>) -> String {
    let jwk: Jwk = serde_json::from_value(json!({ "kty": "EC", "crv": "P-256", "x": x, "y": y })).expect("public EC jwk");
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some("dpop+jwt".to_string());
    header.jwk = Some(jwk);
    let jti = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 16]>());
    let mut claims = json!({ "jti": jti, "htm": htm, "htu": htu, "iat": chrono::Utc::now().timestamp() });
    if let Some(token) = token {
        claims["ath"] = json!(URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(token.as_bytes())));
    }
    jsonwebtoken::encode(&header, &claims, key).expect("sign the DPoP proof")
}
```

and change the token-endpoint call at `:180` to `dpop_proof(&dpop_key, &dpop_x, &dpop_y, "POST", &token_url, None)`.

4. After the last assertion of the test (`assert_eq!(second["subject"], subject);`), add:

```rust
    // SMA-700 AC 1 with a real Keycloak token: alice is provisioned now (above). The same bound
    // token and a NEW proof for the gateway URL pass Introspect with a DPoP context. The Bearer
    // refusals above still hold with DPoP on (D7).
    let proof = dpop_proof(&dpop_key, &dpop_x, &dpop_y, "POST", "https://gw.example.test/v1/chat/completions", Some(&dpop_token));
    let request = DpopRequest {
        proof,
        method: "POST".to_string(),
        path: "/v1/chat/completions".to_string(),
    };
    let ctx = state.authn.introspect_dpop(&dpop_token, request.clone()).await.expect("a Keycloak DPoP-bound token with a valid proof passes Introspect");
    assert_eq!(ctx.principal.principal_id.canonical(), principal_prn, "the bound token resolves to alice");
    let err = state.authn.introspect_dpop(&dpop_token, request).await.expect_err("the same proof twice is a replay");
    assert!(matches!(err, AuthnError::InvalidDpopProof(ProofDefect::Replayed)), "got {err:?}");
```

and add `ProofDefect` to `use paigasus_iam_core::{AuthnError, TokenDefect};`.

- [ ] **Step 5: Run the suites**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo fmt
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test grpc_authn --test grpc_whoami --test http_authn --test docker_preflight
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test keycloak_e2e
cargo clippy --locked -p paigasus-iam --all-targets -- -D warnings
```

Expected: all pass. `keycloak_e2e` takes up to 4 minutes (the container start). If `a_large_token_and_a_large_proof_pass_the_transport` reports a token or proof size outside its range, adjust the two pad lengths only (11 500 and 5 500), never the ranges.

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git add rs/crates/services/paigasus-iam/tests
git commit -m "test(rs): DPoP gateway path over real gRPC, HTTP and Keycloak (SMA-700)

The Introspect and follow-up sequence once and never again, ath across
two tokens of one key, the self-query rule, the path rule, the order
before the identity lookup, an unconfigured base URL, DPoP off, the
Bearer scheme with a bound token, and the header-list size.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Gateway configuration, `AuthState`, and the `credentials()` parser

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/config.rs` (`GatewayConfig` `:22-45`; a new struct after `LimitsBackend` `:200`; tests)
- Modify: `rs/crates/services/paigasus-gateway/src/service_info.rs:95-114` (test literal)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs` (`AppState` `:45-61`; `router` `:87-118`; `state_with_iam` `:250-258`; the `pub use` line `:20`)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs` (`require_iam_auth` `:72`; `require_authenticated` `:250`; `bearer` `:402-413`; test builders `:640-642`, `:693-697`; new parser tests)
- Modify: `rs/crates/services/paigasus-gateway/src/main.rs:93-99`
- Modify: the `AppState { … }` literals in `tests/chat_proxy.rs:164`, `tests/service_info.rs:156`, `tests/metrics.rs:103`, `:156`, `:229`, `tests/support/limits.rs:208`
- Modify: `rs/crates/services/paigasus-gateway/gateway.toml.example` (after the `[metrics]` section, line 75)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:

```rust
// paigasus_gateway::config
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct GatewayDpopConfig { pub enabled: bool }
pub struct GatewayConfig { /* existing */ #[serde(default)] pub dpop: GatewayDpopConfig }
// paigasus_gateway::adapters::http
pub struct AppState { /* existing */ pub dpop_enabled: bool }
pub use auth::AuthState;
// paigasus_gateway::adapters::http::auth
#[derive(Clone)] pub struct AuthState { pub iam: Arc<dyn Iam>, pub dpop_enabled: bool }
pub const DPOP_HEADER: &str = "dpop";
#[derive(Clone, PartialEq, Eq)] pub enum ProofHeader { One(String), Missing, Invalid }   // Debug prints no proof
#[derive(Clone, PartialEq, Eq)] pub enum Credentials { Bearer(String), Dpop { token: String, proof: ProofHeader } } // Debug prints no secret
pub fn credentials(headers: &HeaderMap, dpop_enabled: bool) -> Option<Credentials>;
```

`require_iam_auth` and `require_authenticated` take `State(auth): State<AuthState>`. In this task they still use only the Bearer arm (`Credentials::Dpop` falls into the missing-bearer branch); Task 13 adds the DPoP flow.

- [ ] **Step 1: Write the failing config and parser tests**

`config.rs` tests, after `every_limits_key_reads_from_the_environment`:

```rust
    #[test]
    fn dpop_is_off_by_default_and_reads_from_the_environment() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(!cfg.dpop.enabled, "D4: off by default");
            jail.set_env("GATEWAY_DPOP__ENABLED", "true");
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.dpop.enabled);
            assert!(cfg.validate().is_ok());
            Ok(())
        });
    }

    #[test]
    fn a_misspelt_dpop_key_fails_extraction() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &format!("{}\n[dpop]\nenable = true\n", valid_toml()))?;
            assert!(GatewayConfig::figment().extract::<GatewayConfig>().is_err(), "a typo must not leave DPoP silently off");
            Ok(())
        });
    }
```

`auth.rs` tests, add:

```rust
    // ---- SMA-700: the credentials parser -----------------------------------------------------

    fn headers_of(authorization: Option<&str>, proofs: &[&[u8]]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = authorization {
            headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        }
        for proof in proofs {
            headers.append(DPOP_HEADER, HeaderValue::from_bytes(proof).unwrap());
        }
        headers
    }

    #[test]
    fn the_parser_reads_both_schemes_in_any_case() {
        assert_eq!(credentials(&headers_of(Some("Bearer t"), &[]), true), Some(Credentials::Bearer("t".into())));
        assert_eq!(credentials(&headers_of(Some("bearer t"), &[]), false), Some(Credentials::Bearer("t".into())));
        for scheme in ["DPoP", "dpop", "DPOP"] {
            assert_eq!(
                credentials(&headers_of(Some(&format!("{scheme} t")), &[b"a.b.c"]), true),
                Some(Credentials::Dpop { token: "t".into(), proof: ProofHeader::One("a.b.c".into()) }),
                "{scheme}"
            );
        }
    }

    #[test]
    fn the_proof_header_is_one_visible_ascii_value() {
        let dpop = |proofs: &[&[u8]]| credentials(&headers_of(Some("DPoP t"), proofs), true);
        assert_eq!(dpop(&[]), Some(Credentials::Dpop { token: "t".into(), proof: ProofHeader::Missing }));
        assert_eq!(dpop(&[b"a.b.c", b"d.e.f"]), Some(Credentials::Dpop { token: "t".into(), proof: ProofHeader::Invalid }), "two headers");
        assert_eq!(dpop(&[b"a.\xffb.c"]), Some(Credentials::Dpop { token: "t".into(), proof: ProofHeader::Invalid }), "not visible ASCII");
        assert_eq!(dpop(&[b""]), Some(Credentials::Dpop { token: "t".into(), proof: ProofHeader::Invalid }), "empty");
    }

    #[test]
    fn the_bearer_scheme_ignores_a_dpop_header_and_dpop_off_ignores_the_scheme() {
        assert_eq!(credentials(&headers_of(Some("Bearer t"), &[b"a.b.c"]), true), Some(Credentials::Bearer("t".into())));
        assert_eq!(credentials(&headers_of(Some("DPoP t"), &[b"a.b.c"]), false), None, "D11: DPoP off is as today");
        assert_eq!(credentials(&headers_of(Some("DPoP "), &[b"a.b.c"]), true), None, "an empty token");
        assert_eq!(credentials(&headers_of(Some("Basic t"), &[]), true), None);
        assert_eq!(credentials(&headers_of(None, &[b"a.b.c"]), true), None);
    }

    #[test]
    fn credentials_debug_prints_no_secret() {
        let printed = format!("{:?} {:?}", Credentials::Bearer("tok-secret".into()), Credentials::Dpop { token: "tok-secret".into(), proof: ProofHeader::One("proof-secret".into()) });
        assert!(!printed.contains("secret"), "{printed}");
    }
```

- [ ] **Step 2: Run them to see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo nextest run --locked -p paigasus-gateway --lib -E 'test(dpop) | test(credentials) | test(the_parser) | test(the_proof_header) | test(the_bearer_scheme)'
```

Expected: compile errors for `dpop`, `credentials`, `Credentials`, `ProofHeader`, `DPOP_HEADER`.

- [ ] **Step 3: Implement the config**

In `GatewayConfig`, after `limits`:

```rust
    /// SMA-700: the DPoP scheme on the protected routes. Off by default (D4). The chart does not
    /// deploy the gateway, so the operator sets `GATEWAY_DPOP__ENABLED`. Turn DPoP on in IAM first.
    #[serde(default)]
    pub dpop: GatewayDpopConfig,
```

After `LimitsBackend`:

```rust
/// `[dpop]` (SMA-700 § 4.10). With `enabled`, the gateway accepts `Authorization: DPoP <token>`
/// with one `DPoP` proof header, forwards the proof to IAM, and sends a `WWW-Authenticate: DPoP`
/// challenge on a 401. A misspelt key fails extraction.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct GatewayDpopConfig {
    pub enabled: bool,
}
```

`service_info.rs` test literal (`:112-113`): after `limits: None,` add `dpop: crate::config::GatewayDpopConfig::default(),`.

`gateway.toml.example`, after line 75:

```toml

# --- DPoP (SMA-700): proof-of-possession tokens on the protected routes ---
#
# Off by default. With enabled = true, a client may send `Authorization: DPoP <token>` with one
# `DPoP` proof header (RFC 9449). The gateway forwards the proof, the method and the path to IAM,
# which checks it against its authn.dpop.forwarded_base_urls. Turn DPoP on in IAM first.
# A 401 then carries `WWW-Authenticate: DPoP algs="ES256 RS256"`. The API-key leg does not run for
# the DPoP scheme. A client must make a NEW proof for each attempt: a retry that sends the same
# proof again is refused as a replay.
# [dpop]
# enabled = false                       # default shown; env GATEWAY_DPOP__ENABLED
```

- [ ] **Step 4: Implement `AuthState` and the parser in `auth.rs`**

Above `require_iam_auth`:

```rust
/// The auth middlewares' state (SMA-700 § 4.10): the IAM port and the DPoP switch. Independent of
/// the handler's `AppState`, as before.
#[derive(Clone)]
pub struct AuthState {
    pub iam: Arc<dyn Iam>,
    pub dpop_enabled: bool,
}

/// The DPoP proof request header (RFC 9449 § 4.1), in lower case as `HeaderMap` stores it.
pub const DPOP_HEADER: &str = "dpop";

/// The `DPoP` header of a DPoP request. `Debug` prints no proof.
#[derive(Clone, PartialEq, Eq)]
pub enum ProofHeader {
    One(String),
    Missing,
    /// Two or more `DPoP` headers, a value that is not visible ASCII, or an empty value.
    Invalid,
}

impl std::fmt::Debug for ProofHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ProofHeader::One(_) => "One(..)",
            ProofHeader::Missing => "Missing",
            ProofHeader::Invalid => "Invalid",
        })
    }
}

/// The credential of a request. `Debug` prints no secret.
#[derive(Clone, PartialEq, Eq)]
pub enum Credentials {
    Bearer(String),
    Dpop { token: String, proof: ProofHeader },
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credentials::Bearer(_) => f.write_str("Bearer(..)"),
            Credentials::Dpop { proof, .. } => write!(f, "Dpop {{ token: .., proof: {proof:?} }}"),
        }
    }
}

/// Parse the `Authorization` header (SMA-700 § 4.10, decision P7). Matches IAM's own parser
/// (`adapters/auth.rs`): split on the first space, an ASCII-case-insensitive scheme, the token
/// trimmed and not empty. `Bearer` ignores a `DPoP` header. The `DPoP` scheme counts only when
/// gateway DPoP is on; else it is `None`, as today (D11).
pub fn credentials(headers: &HeaderMap, dpop_enabled: bool) -> Option<Credentials> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    if scheme.eq_ignore_ascii_case("Bearer") {
        return Some(Credentials::Bearer(token.to_owned()));
    }
    if dpop_enabled && scheme.eq_ignore_ascii_case("DPoP") {
        return Some(Credentials::Dpop {
            token: token.to_owned(),
            proof: proof_header(headers),
        });
    }
    None
}

fn proof_header(headers: &HeaderMap) -> ProofHeader {
    let mut values = headers.get_all(DPOP_HEADER).iter();
    let Some(first) = values.next() else {
        return ProofHeader::Missing;
    };
    if values.next().is_some() {
        return ProofHeader::Invalid;
    }
    match first.to_str() {
        Ok(proof) if !proof.is_empty() => ProofHeader::One(proof.to_owned()),
        _ => ProofHeader::Invalid,
    }
}
```

Delete the old `fn bearer(headers: &HeaderMap) -> Option<String>` (`:398-413`); its callers change below.

`require_iam_auth` (`:72-75`) becomes:

```rust
pub async fn require_iam_auth(State(auth): State<AuthState>, mut req: Request, next: Next) -> Response {
    let iam = auth.iam;
    // 1. Bearer — the ONLY accepted credential source (no cookies, no query params). The DPoP
    //    scheme is added in SMA-700 Task 13.
    let Some(Credentials::Bearer(token)) = credentials(req.headers(), auth.dpop_enabled) else {
        return GatewayError::MissingBearer.into_response();
    };
```

(the rest of the body is unchanged; it reads `iam` as before). `require_authenticated` (`:250-253`) the same way:

```rust
pub async fn require_authenticated(State(auth): State<AuthState>, req: Request, next: Next) -> Response {
    let iam = auth.iam;
    let Some(Credentials::Bearer(token)) = credentials(req.headers(), auth.dpop_enabled) else {
        return GatewayError::MissingBearer.into_response();
    };
```

Fix the doc sentence of `require_iam_auth`: `Wired via from_fn_with_state(AuthState { iam, dpop_enabled }, require_iam_auth)`.

The test builders (`:640-642`, `:693-697`):

```rust
    fn build_app(fake: FakeIam) -> Router {
        build_app_with(fake, false)
    }

    fn build_app_with(fake: FakeIam, dpop_enabled: bool) -> Router {
        let state = AuthState { iam: Arc::new(fake), dpop_enabled };
        Router::new().route("/x", get(probe)).layer(from_fn_with_state(state, require_iam_auth))
    }
```

```rust
    fn build_discovery_app(fake: FakeIam) -> Router {
        build_discovery_app_with(fake, false)
    }

    fn build_discovery_app_with(fake: FakeIam, dpop_enabled: bool) -> Router {
        let state = AuthState { iam: Arc::new(fake), dpop_enabled };
        Router::new().route("/x", get(discovery_probe)).layer(from_fn_with_state(state, require_authenticated))
    }
```

The test `self_query_uses_caller_key_and_introspected_principal` (`:963`) builds its own router; change that line to `let app = build_app(fake);` (the recorder is captured before, on line 961).

- [ ] **Step 5: Wire `AppState`, the router, `main.rs` and the test literals**

`adapters/http/mod.rs`: add to `AppState` after `limits`:

```rust
    /// SMA-700: `GatewayConfig::dpop.enabled`. The auth middlewares read it through `AuthState`.
    pub dpop_enabled: bool,
```

Change `pub use auth::{require_authenticated, require_iam_auth};` to `pub use auth::{AuthState, require_authenticated, require_iam_auth};`. In `router`:

```rust
    // The auth middlewares' state is the IAM port and the DPoP switch (`AuthState`), captured here
    // BEFORE the final `with_state`, and independent of the handler's `AppState`.
    let auth_state = AuthState {
        iam: state.iam.clone(),
        dpop_enabled: state.dpop_enabled,
    };
    let auth = axum::middleware::from_fn_with_state(auth_state.clone(), require_iam_auth);
```

and `let discovery_auth = axum::middleware::from_fn_with_state(auth_state, require_authenticated);`. In `state_with_iam` add `dpop_enabled: false,`.

`main.rs`, the `AppState` literal gets `dpop_enabled: config.dpop.enabled,`, and after it:

```rust
    if config.dpop.enabled {
        tracing::info!("DPoP is on: the protected routes accept Authorization: DPoP with one DPoP proof header; IAM must have authn.dpop on");
    }
```

In each test `AppState { … }` literal (`tests/chat_proxy.rs:164`, `tests/service_info.rs:156`, `tests/metrics.rs:103`, `:156`, `:229`, `tests/support/limits.rs:208`) add `dpop_enabled: false,` after `limits…,`. Check none is missed:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs/crates/services/paigasus-gateway
grep -rn "AppState {" src tests | grep -v "pub struct"
```

Expected: eight hits (`main.rs`, `mod.rs` `state_with_iam`, and the six test literals), each with `dpop_enabled`.

- [ ] **Step 6: Run the gateway**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo fmt
cargo nextest run --locked -p paigasus-gateway --lib --test chat_proxy --test service_info --test metrics --test limits_http
cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings
```

Expected: all pass; every existing auth test answers as before.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): gateway dpop switch, AuthState and the credentials parser (SMA-700)

GATEWAY_DPOP__ENABLED, off by default. The auth middlewares take the IAM
port and the switch. The parser reads the Bearer and the DPoP schemes and
one visible-ASCII DPoP header; with DPoP off the DPoP scheme is ignored.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: The gateway `Iam` trait carries the DPoP context and the caller credential

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs` (trait `:52-75`; impl `:103-128`; `self_authorize_request` `:159-169`; tests `:247-299`)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/iam/mod.rs:8`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs` (the two `introspect_token` calls `:102`, `:284`; `authorize_self` `:135`, `:165`, `:179-181`; `FakeIam` `:519-605`; assertions `:969`, `:1255`)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs` (`UnusedIam` `:188-198`, `ProbeIam` `:219-236`)
- Modify: `rs/crates/services/paigasus-gateway/tests/{metrics.rs,service_info.rs,chat_proxy.rs,support/limits.rs}` (every `impl Iam`)

**Interfaces:**
- Consumes: `DpopContext` (Task 1).
- Produces (`paigasus_gateway::adapters::iam`, re-exported from `client`):

```rust
pub use paigasus_proto::paigasus::iam::v1::DpopContext;
#[derive(Clone, PartialEq, Eq)] pub enum CallerCredential { ApiKey(String), Bearer(String), Dpop { token: String, proof: String } } // Debug prints no secret
#[async_trait] pub trait Iam: Send + Sync {
    async fn introspect_api_key(&self, token: &str) -> Result<IntrospectApiKeyResponse, IamError>;
    async fn is_authorized_self(&self, caller: &CallerCredential, principal_prn: &str, action: &str, resource_prn: &str) -> Result<bool, IamError>;
    async fn introspect_token(&self, token: &str, dpop: Option<DpopContext>) -> Result<IntrospectResponse, IamError>;
}
```

`ApiKey` and `Bearer` send `authorization: Bearer <secret>`; `Dpop` sends `authorization: DPoP <token>` and `dpop: <proof>`. This task changes no behavior: every production caller passes `None` and `ApiKey`/`Bearer`.

- [ ] **Step 1: Write the failing client tests**

In `client.rs` tests, change the call in `self_authorize_request_sets_bearer_and_body` to:

```rust
            self_authorize_request(&CallerCredential::ApiKey(caller_key.to_owned()), "prn:paigasus:iam:default:sa/gw-caller", "InvokeModel", "prn:paigasus:iam:default:scope/team-a").expect("valid metadata")
```

and add after it:

```rust
    #[test]
    fn a_bearer_caller_sends_the_bearer_scheme_and_no_proof() {
        let req = self_authorize_request(&CallerCredential::Bearer("user-token".into()), "p", "InvokeModel", "r").expect("valid metadata");
        assert_eq!(req.metadata().get("authorization").unwrap().to_str().unwrap(), "Bearer user-token");
        assert!(req.metadata().get("dpop").is_none());
    }

    #[test]
    fn a_dpop_caller_sends_the_dpop_scheme_and_one_proof() {
        // SMA-700 § 4.10: the IsAuthorized follow-up metadata.
        let caller = CallerCredential::Dpop {
            token: "bound-token".into(),
            proof: "a.b.c".into(),
        };
        let req = self_authorize_request(&caller, "p", "InvokeModel", "r").expect("valid metadata");
        assert_eq!(req.metadata().get("authorization").unwrap().to_str().unwrap(), "DPoP bound-token");
        assert_eq!(req.metadata().get_all("dpop").iter().map(|v| v.to_str().unwrap().to_owned()).collect::<Vec<_>>(), vec!["a.b.c".to_owned()]);
    }

    #[test]
    fn a_token_introspect_carries_the_dpop_context_and_no_authorization() {
        let context = DpopContext {
            proof: "a.b.c".into(),
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
        };
        let req = token_introspect_request("tok", Some(context.clone()));
        assert!(req.metadata().get("authorization").is_none(), "Introspect is bearer-exempt");
        assert_eq!(req.get_ref().token, "tok");
        assert_eq!(req.get_ref().dpop, Some(context));
        assert_eq!(token_introspect_request("tok", None).get_ref().dpop, None);
    }

    #[test]
    fn caller_credential_debug_prints_no_secret() {
        let printed = format!(
            "{:?} {:?} {:?}",
            CallerCredential::ApiKey("key-secret".into()),
            CallerCredential::Bearer("tok-secret".into()),
            CallerCredential::Dpop { token: "tok-secret".into(), proof: "proof-secret".into() }
        );
        assert!(!printed.contains("secret"), "{printed}");
    }
```

- [ ] **Step 2: Run them to see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo nextest run --locked -p paigasus-gateway --lib -E 'test(adapters::iam)'
```

Expected: compile errors for `CallerCredential`, `DpopContext`, `token_introspect_request`.

- [ ] **Step 3: Implement in `client.rs`**

Imports: change line 19 to `use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyRequest, IntrospectApiKeyResponse, IntrospectRequest, IntrospectResponse, IsAuthorizedRequest};` (unchanged) and add after it:

```rust
pub use paigasus_proto::paigasus::iam::v1::DpopContext;
```

Before the `Iam` trait, add:

```rust
/// The credential that the self-query presents to IAM (SMA-700 § 4.10). `ApiKey` and `Bearer`
/// both send `authorization: Bearer <secret>`; `Dpop` sends `authorization: DPoP <token>` and the
/// one `dpop: <proof>` that IAM's `IsAuthorized` follow-up needs. `Debug` prints no secret.
#[derive(Clone, PartialEq, Eq)]
pub enum CallerCredential {
    ApiKey(String),
    Bearer(String),
    Dpop { token: String, proof: String },
}

impl std::fmt::Debug for CallerCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CallerCredential::ApiKey(_) => "ApiKey(..)",
            CallerCredential::Bearer(_) => "Bearer(..)",
            CallerCredential::Dpop { .. } => "Dpop { .. }",
        })
    }
}
```

Change the two trait methods:

```rust
    /// The self-query `IsAuthorized` (D9). The caller's own credential rides as the
    /// `authorization` metadata AND `principal_prn` is that same caller's PRN — so IAM sees a
    /// principal asking about *itself*. For a DPoP caller (SMA-700) the credential is the DPoP
    /// follow-up: the same token and the same proof that `introspect_token` carried.
    async fn is_authorized_self(&self, caller: &CallerCredential, principal_prn: &str, action: &str, resource_prn: &str) -> Result<bool, IamError>;
```

```rust
    /// Introspect a caller-presented OIDC token (IAM's `AuthnService.Introspect`). **Bearer-
    /// EXEMPT**: the token is the request body. `dpop` is the client's DPoP context (SMA-700);
    /// `None` is the Bearer scheme.
    async fn introspect_token(&self, token: &str, dpop: Option<DpopContext>) -> Result<IntrospectResponse, IamError>;
```

Impl:

```rust
    async fn is_authorized_self(&self, caller: &CallerCredential, principal_prn: &str, action: &str, resource_prn: &str) -> Result<bool, IamError> {
        let req = self_authorize_request(caller, principal_prn, action, resource_prn)?;
        let resp = self.authz.clone().is_authorized(req).await?;
        Ok(resp.into_inner().allowed)
    }

    async fn introspect_token(&self, token: &str, dpop: Option<DpopContext>) -> Result<IntrospectResponse, IamError> {
        let resp = self.authn.clone().introspect(token_introspect_request(token, dpop)).await?;
        Ok(resp.into_inner())
    }
```

After `introspect_request`, add:

```rust
/// Build the `Introspect` request: the token and the optional DPoP context in the body, **no**
/// `authorization` metadata (bearer-exempt). The third `with_correlation` site, extracted so a
/// unit test reaches it (it was inline until SMA-700).
fn token_introspect_request(token: &str, dpop: Option<DpopContext>) -> Request<IntrospectRequest> {
    with_correlation(Request::new(IntrospectRequest { token: token.to_owned(), dpop }))
}
```

Replace `self_authorize_request`:

```rust
fn self_authorize_request(caller: &CallerCredential, principal_prn: &str, action: &str, resource_prn: &str) -> Result<Request<IsAuthorizedRequest>, IamError> {
    let mut req = with_correlation(Request::new(IsAuthorizedRequest {
        principal_prn: principal_prn.to_owned(),
        action: action.to_owned(),
        resource_prn: resource_prn.to_owned(),
        context: Default::default(),
    }));
    let (authorization, proof) = match caller {
        CallerCredential::ApiKey(secret) | CallerCredential::Bearer(secret) => (format!("Bearer {secret}"), None),
        CallerCredential::Dpop { token, proof } => (format!("DPoP {token}"), Some(proof)),
    };
    // The error text of `try_from` names no value, so no credential reaches the message.
    let authorization = MetadataValue::try_from(authorization).map_err(|e| IamError::Connect(format!("the caller credential is not a valid `authorization` metadata value: {e}")))?;
    req.metadata_mut().insert("authorization", authorization);
    if let Some(proof) = proof {
        let proof = MetadataValue::try_from(proof.as_str()).map_err(|e| IamError::Connect(format!("the DPoP proof is not a valid `dpop` metadata value: {e}")))?;
        req.metadata_mut().insert("dpop", proof);
    }
    Ok(req)
}
```

Update the doc above it (`caller_key` → `caller`, and "SMA-700: a DPoP caller adds one `dpop` entry").

`iam/mod.rs:8`: `pub use client::{CallerCredential, DpopContext, Iam, IamClient, IamError};`

- [ ] **Step 4: Change every caller and every implementer**

`auth.rs` production code: add `use crate::adapters::iam::CallerCredential;` to the imports (Task 13 adds `DpopContext`; importing it now would be an unused import). Change:

- `:102` `iam.introspect_token(&token).await` → `iam.introspect_token(&token, None).await`
- `:284` `iam.introspect_token(&token).await` → `iam.introspect_token(&token, None).await`
- `:135` `authorize_self(iam.as_ref(), &token, &principal_prn, &org_prn, None)` → `authorize_self(iam.as_ref(), &CallerCredential::Bearer(token.clone()), &principal_prn, &org_prn, None)`
- `:165` `authorize_self(iam, token, &resp.principal_prn, …)` → `authorize_self(iam, &CallerCredential::ApiKey(token.to_owned()), &resp.principal_prn, …)`
- `:179` `async fn authorize_self(iam: &dyn Iam, token: &str, …)` → `async fn authorize_self(iam: &dyn Iam, caller: &CallerCredential, …)`, and `:181` `iam.is_authorized_self(token, …)` → `iam.is_authorized_self(caller, …)`.

`auth.rs` tests — `RecordedAuthz` (`:521-526`) becomes:

```rust
    #[derive(Debug, Clone)]
    struct RecordedAuthz {
        caller: CallerCredential,
        principal_prn: String,
        action: String,
        resource_prn: String,
    }
```

`FakeIam::is_authorized_self` (`:573-579`):

```rust
        async fn is_authorized_self(&self, caller: &CallerCredential, principal_prn: &str, action: &str, resource_prn: &str) -> Result<bool, IamError> {
            *self.recorded.lock().unwrap() = Some(RecordedAuthz {
                caller: caller.clone(),
                principal_prn: principal_prn.to_owned(),
                action: action.to_owned(),
                resource_prn: resource_prn.to_owned(),
            });
```

`FakeIam::introspect_token` (`:591`): `async fn introspect_token(&self, _token: &str, _dpop: Option<DpopContext>) -> Result<IntrospectResponse, IamError> {` and add `use crate::adapters::iam::DpopContext;` to the test module imports.

`:969`: `assert_eq!(rec.caller, CallerCredential::ApiKey(CALLER_KEY.to_owned()), "authz must present the caller's OWN key");`
`:1255`: `assert_eq!(rec.caller, CallerCredential::Bearer(USER_TOKEN.to_owned()));`

`mod.rs` `UnusedIam` and `ProbeIam`: the two signatures become

```rust
        async fn is_authorized_self(&self, _caller: &CallerCredential, _principal_prn: &str, _action: &str, _resource_prn: &str) -> Result<bool, IamError> {
```

```rust
        async fn introspect_token(&self, _token: &str, _dpop: Option<DpopContext>) -> Result<paigasus_proto::paigasus::iam::v1::IntrospectResponse, IamError> {
```

with `use crate::adapters::iam::{CallerCredential, DpopContext};` in that test module.

Integration tests — in `tests/metrics.rs` (three impls), `tests/service_info.rs`, `tests/chat_proxy.rs`, `tests/support/limits.rs`, change each `is_authorized_self` signature to `(&self, _caller: &CallerCredential, _principal_prn: &str, _action: &str, _resource_prn: &str)` and each `introspect_token` signature to `(&self, _token: &str, _dpop: Option<DpopContext>)`, and change each `use paigasus_gateway::adapters::iam::{Iam, IamError};` to `use paigasus_gateway::adapters::iam::{CallerCredential, DpopContext, Iam, IamError};`. Check:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs/crates/services/paigasus-gateway
grep -rn "_caller_key\|introspect_token(&self, _token: &str)" src tests
```

Expected: no output.

- [ ] **Step 5: Run the gateway**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo fmt
cargo nextest run --locked -p paigasus-gateway --lib --test chat_proxy --test service_info --test metrics --test limits_http
cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): the gateway Iam port carries the DPoP context and the caller credential (SMA-700)

introspect_token takes an optional DpopContext, and is_authorized_self a
CallerCredential: ApiKey and Bearer send the Bearer scheme, Dpop sends the
DPoP scheme with one dpop metadata entry. No behavior changes yet.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: The gateway DPoP flow, the error mapping, and the challenge

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs` (`require_iam_auth` through `authorize_self` `:68-207`; `require_authenticated` `:250-315`; the reason helpers `:317-354`; `iam_result` `:389-396`; `introspect_error` `:419-431`; `authz_error` `:438-451`; tests)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs` (enum `:59-119`; `parts` `:163-248`; `retryable` `:271-286`; a `status` fn; tests)
- Modify: `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs` (a new fake and a new test)

**Interfaces:**
- Consumes: `AuthState`, `Credentials`, `ProofHeader`, `credentials` (Task 11); `CallerCredential`, `DpopContext` (Task 12); `ErrorReason::{InvalidDpopProof, DpopQuotaExceeded}` (Task 1).
- Produces:

```rust
// paigasus_gateway::adapters::http::error
pub enum GatewayError { /* existing */ InvalidDpopProof }            // 401, invalid_request_error, invalid-dpop-proof, "Invalid DPoP proof.", not retryable
impl GatewayError { pub fn status(self) -> StatusCode; }
// paigasus_gateway::adapters::http::auth
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum SchemeUsed { None, Bearer, Dpop }
pub fn dpop_challenge(dpop_enabled: bool, scheme: SchemeUsed, err: &GatewayError) -> Option<HeaderValue>;
```

IAM `Unauthenticated` + `invalid-dpop-proof` → `InvalidDpopProof`; IAM `ResourceExhausted` + `dpop-quota-exceeded` → `RateLimited { retry_after_secs }` from `RetryInfo` (at least 1), metric label `denied`.

- [ ] **Step 1: `GatewayError::InvalidDpopProof` — failing tests first**

In `error.rs` tests, add to `each_case_maps_to_its_bound_status`:

```rust
        assert_eq!(GatewayError::InvalidDpopProof.into_response().status(), StatusCode::UNAUTHORIZED);
```

and add:

```rust
    /// SMA-700 § 4.10: the envelope of a refused DPoP proof. `status()` is what the middleware
    /// reads to decide on the challenge.
    #[tokio::test]
    async fn an_invalid_dpop_proof_is_a_401_envelope() {
        let resp = GatewayError::InvalidDpopProof.into_response();
        assert_eq!(resp.headers()["paigasus-retryable"], "false");
        assert!(resp.headers().get("www-authenticate").is_none(), "IntoResponse adds no challenge; the middleware does");
        let body = body_json(resp).await;
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert_eq!(body["error"]["code"], "invalid-dpop-proof");
        assert_eq!(body["error"]["message"], "Invalid DPoP proof.");
        assert!(body["error"]["param"].is_null());
        assert_eq!(GatewayError::InvalidDpopProof.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(GatewayError::RateLimited { retry_after_secs: 1 }.status(), StatusCode::TOO_MANY_REQUESTS);
    }
```

Run `cargo nextest run --locked -p paigasus-gateway --lib -E 'test(error::)'`. Expected: compile error `no variant InvalidDpopProof`.

- [ ] **Step 2: Implement it**

In the enum, after `InvalidCredential,`:

```rust
    /// SMA-700: the DPoP proof is missing or invalid, at the gateway or by IAM's
    /// `invalid-dpop-proof` → 401. The middleware adds the `WWW-Authenticate: DPoP` challenge.
    InvalidDpopProof,
```

In `parts`, after the `InvalidCredential` arm:

```rust
            GatewayError::InvalidDpopProof => (StatusCode::UNAUTHORIZED, "invalid_request_error", Some("invalid-dpop-proof"), None, "Invalid DPoP proof."),
```

In `retryable`, add `| Self::InvalidDpopProof` to the `Retryable::No` list. After `retryable`, add:

```rust
    /// The bound HTTP status of this case (the first element of `parts`).
    #[must_use]
    pub fn status(self) -> StatusCode {
        self.parts().0
    }
```

Run the error tests again. Expected: PASS (including `every_gateway_code_is_declared_in_the_canonical_registry`, which now sees `invalid-dpop-proof`).

- [ ] **Step 3: Write the failing middleware tests**

In the `auth.rs` test module: add `use std::sync::atomic::{AtomicUsize, Ordering};`. Extend `FakeIam` with two recorders:

```rust
        /// SMA-700 D15: how often the API-key leg ran.
        api_key_calls: Arc<AtomicUsize>,
        /// SMA-700: the `dpop` argument of the last `introspect_token` call.
        token_dpop: Arc<Mutex<Option<Option<DpopContext>>>>,
```

initialized in `FakeIam::new` as `api_key_calls: Arc::new(AtomicUsize::new(0)), token_dpop: Arc::new(Mutex::new(None)),`. In `introspect_api_key`, add `self.api_key_calls.fetch_add(1, Ordering::SeqCst);` as the first line. In `introspect_token` (now `(&self, _token: &str, dpop: Option<DpopContext>)`), add `*self.token_dpop.lock().unwrap() = Some(dpop);` as the first line.

Add the helpers and tests:

```rust
    // ---- SMA-700: the DPoP flow ------------------------------------------------------------------

    const PROOF: &str = "proof.header.sig";
    const CHALLENGE_PROOF: &str = "DPoP error=\"invalid_dpop_proof\", algs=\"ES256 RS256\"";
    const CHALLENGE_TOKEN: &str = "DPoP error=\"invalid_token\", algs=\"ES256 RS256\"";
    const CHALLENGE_BARE: &str = "DPoP algs=\"ES256 RS256\"";

    fn dpop_req(uri: &str, proofs: &[&str]) -> HttpRequest<Body> {
        let mut builder = HttpRequest::builder().uri(uri).header(header::AUTHORIZATION, format!("DPoP {USER_TOKEN}"));
        for proof in proofs {
            builder = builder.header(DPOP_HEADER, *proof);
        }
        builder.body(Body::empty()).unwrap()
    }

    fn challenges(resp: &Response) -> Vec<String> {
        resp.headers().get_all(header::WWW_AUTHENTICATE).iter().map(|v| v.to_str().unwrap().to_owned()).collect()
    }

    /// A fake whose IAM must never be reached: the key leg answers a connect failure only if it
    /// runs, and `introspect_token` is unconfigured (it panics).
    fn untouched_fake() -> FakeIam {
        FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable)
    }

    #[tokio::test]
    async fn a_dpop_request_forwards_its_context_skips_the_key_leg_and_self_queries_with_dpop() {
        let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Ok(true));
        let (api_key_calls, token_dpop, recorded) = (fake.api_key_calls.clone(), fake.token_dpop.clone(), fake.recorded.clone());
        let resp = build_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(api_key_calls.load(Ordering::SeqCst), 0, "D15: the DPoP scheme skips the API-key leg");
        assert_eq!(
            token_dpop.lock().unwrap().clone().expect("introspect_token ran"),
            Some(DpopContext {
                proof: PROOF.into(),
                method: "GET".into(),
                path: "/x".into()
            })
        );
        let rec = recorded.lock().unwrap().take().expect("is_authorized_self ran");
        assert_eq!(rec.caller, CallerCredential::Dpop { token: USER_TOKEN.into(), proof: PROOF.into() });
        assert_eq!((rec.principal_prn.as_str(), rec.resource_prn.as_str()), (USER_PRN, ORG_A_PRN));
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(String::from_utf8(body.to_vec()).unwrap(), format!("{USER_PRN}|{ORG_A_PRN}|oidc"));
    }

    #[tokio::test]
    async fn a_query_string_is_not_forwarded_in_the_path() {
        // Review Focus 3: IAM refuses a path with `?`, so the gateway must send the path alone.
        let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Ok(true));
        let token_dpop = fake.token_dpop.clone();
        let resp = build_app_with(fake, true).oneshot(dpop_req("/x?trace=1", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(token_dpop.lock().unwrap().clone().flatten().expect("a context").path, "/x");
    }

    #[tokio::test]
    async fn a_missing_or_doubled_proof_is_401_with_no_iam_call() {
        for proofs in [&[][..], &[PROOF, PROOF][..]] {
            let fake = untouched_fake();
            let api_key_calls = fake.api_key_calls.clone();
            let resp = build_app_with(fake, true).oneshot(dpop_req("/x", proofs)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{proofs:?}");
            assert_eq!(challenges(&resp), vec![CHALLENGE_PROOF.to_owned()]);
            assert_eq!(api_key_calls.load(Ordering::SeqCst), 0);
            assert_eq!(body_json(resp).await["error"]["code"], "invalid-dpop-proof");
        }
    }

    #[tokio::test]
    async fn iam_invalid_dpop_proof_is_401_with_the_proof_challenge() {
        let fake = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable)
            .with_token_introspect(TokenIntrospectOutcome::Rpc(Code::Unauthenticated, Some(reason_details(paigasus_proto::paigasus::common::v1::ErrorReason::InvalidDpopProof))));
        let resp = build_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenges(&resp), vec![CHALLENGE_PROOF.to_owned()]);
        assert_eq!(body_json(resp).await["error"]["code"], "invalid-dpop-proof");
    }

    #[tokio::test]
    async fn another_401_on_the_dpop_scheme_gets_the_invalid_token_challenge() {
        let fake = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable).with_token_introspect(rejected_token());
        let resp = build_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenges(&resp), vec![CHALLENGE_TOKEN.to_owned()]);
        assert_eq!(body_json(resp).await["error"]["code"], "invalid-api-key");
    }

    #[tokio::test]
    async fn a_bearer_or_absent_credential_gets_the_bare_challenge_when_dpop_is_on() {
        // RFC 6750 § 3: the error attribute belongs to the scheme the client used.
        let resp = build_app_with(untouched_fake(), true).oneshot(req_no_auth()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenges(&resp), vec![CHALLENGE_BARE.to_owned()]);
        let fake = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable).with_token_introspect(rejected_token());
        let resp = build_app_with(fake, true).oneshot(req_with_auth("Bearer some-token")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenges(&resp), vec![CHALLENGE_BARE.to_owned()]);
        // A 403 gets no challenge.
        let resp = build_app_with(user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Ok(false)), true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(challenges(&resp).is_empty());
    }

    #[tokio::test]
    async fn dpop_off_answers_as_today_with_no_challenge() {
        // D11: the DPoP scheme is ignored (a missing bearer), no IAM call, no WWW-Authenticate.
        let fake = untouched_fake();
        let api_key_calls = fake.api_key_calls.clone();
        let resp = build_app_with(fake, false).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(challenges(&resp).is_empty());
        assert_eq!(api_key_calls.load(Ordering::SeqCst), 0);
        assert_eq!(body_json(resp).await["error"]["code"], "missing-authorization");
        let fake = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable).with_token_introspect(rejected_token());
        let resp = build_app_with(fake, false).oneshot(req_with_auth("Bearer some-token")).await.unwrap();
        assert!(challenges(&resp).is_empty(), "no challenge for Bearer either while off");
    }

    fn quota_details(retry: std::time::Duration) -> tonic_types::ErrorDetails {
        let mut details = reason_details(paigasus_proto::paigasus::common::v1::ErrorReason::DpopQuotaExceeded);
        details.set_retry_info(Some(retry));
        details
    }

    #[tokio::test]
    async fn an_iam_dpop_quota_refusal_is_429_with_retry_after_labelled_denied_and_no_warning() {
        let handle = paigasus_observability::init("test-gateway-dpop-quota");
        let (logs, _guard) = capture_logs();
        let fake = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable)
            .with_token_introspect(TokenIntrospectOutcome::Rpc(Code::ResourceExhausted, Some(quota_details(std::time::Duration::from_secs(7)))));
        let resp = build_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(resp.headers()["retry-after"], "7");
        assert!(challenges(&resp).is_empty(), "a 429 gets no challenge");
        assert_eq!(body_json(resp).await["error"]["code"], "rate-limited");
        let out = handle.render();
        assert!(
            out.lines()
                .any(|l| l.starts_with("gateway_iam_calls_total") && l.contains(r#"operation="introspect_token""#) && l.contains(r#"result="denied""#)),
            "D10: a quota hit is a verdict, not an outage:\n{out}"
        );
        assert!(!logs.text().contains("PermissionDenied with no ErrorInfo"), "{}", logs.text());
    }

    #[tokio::test]
    async fn reading_a_reason_on_a_detail_less_401_writes_no_warning() {
        let (logs, _guard) = capture_logs();
        let fake = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::Unauthenticated, None));
        let resp = build_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(!logs.text().contains("PermissionDenied with no ErrorInfo"), "{}", logs.text());
    }

    #[tokio::test]
    async fn discovery_on_dpop_forwards_the_context_and_never_authorizes() {
        let fake = FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Ok(active_token_response()));
        let (api_key_calls, token_dpop) = (fake.api_key_calls.clone(), fake.token_dpop.clone());
        let resp = build_discovery_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(api_key_calls.load(Ordering::SeqCst), 0);
        assert_eq!(token_dpop.lock().unwrap().clone().flatten().expect("a context").proof, PROOF);
    }

    #[tokio::test]
    async fn discovery_on_dpop_keeps_its_unprovisioned_rule_and_maps_a_bad_proof() {
        // IAM answers identity-not-provisioned only after the proof check passed (D18).
        let fake = FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable)
            .with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied, Some(identity_not_provisioned_details())));
        assert_eq!(build_discovery_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap().status(), StatusCode::OK);
        let fake = FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable)
            .with_token_introspect(TokenIntrospectOutcome::Rpc(Code::Unauthenticated, Some(reason_details(paigasus_proto::paigasus::common::v1::ErrorReason::InvalidDpopProof))));
        let resp = build_discovery_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenges(&resp), vec![CHALLENGE_PROOF.to_owned()]);
    }

    #[test]
    fn the_dpop_challenge_table() {
        use SchemeUsed::{Bearer, Dpop, None as NoScheme};
        let cases = [
            (true, Dpop, GatewayError::InvalidDpopProof, Some(CHALLENGE_PROOF)),
            (true, Dpop, GatewayError::InvalidCredential, Some(CHALLENGE_TOKEN)),
            (true, Dpop, GatewayError::MissingBearer, Some(CHALLENGE_TOKEN)),
            (true, Bearer, GatewayError::InvalidCredential, Some(CHALLENGE_BARE)),
            (true, NoScheme, GatewayError::MissingBearer, Some(CHALLENGE_BARE)),
            (true, Dpop, GatewayError::AuthzDenied, None),
            (true, Dpop, GatewayError::RateLimited { retry_after_secs: 3 }, None),
            (true, Dpop, GatewayError::IamUnavailable, None),
            (false, Dpop, GatewayError::InvalidDpopProof, None),
            (false, Bearer, GatewayError::InvalidCredential, None),
        ];
        for (on, scheme, err, want) in cases {
            assert_eq!(dpop_challenge(on, scheme, &err).map(|v| v.to_str().unwrap().to_owned()), want.map(str::to_owned), "{on} {scheme:?} {err:?}");
        }
    }
```

Add one case to `iam_result_maps_errors_to_bounded_labels`:

```rust
            (
                IamError::Rpc(tonic::Status::with_error_details(Code::ResourceExhausted, "", quota_details(std::time::Duration::from_secs(1)))),
                "denied",
            ),
            (IamError::Rpc(tonic::Status::new(Code::ResourceExhausted, "")), "error"),
```

Run `cargo nextest run --locked -p paigasus-gateway --lib -E 'test(adapters::http::auth)'`. Expected: compile errors (`SchemeUsed`, `dpop_challenge`) and, once those exist, failures in the new tests.

- [ ] **Step 4: Implement the flow**

Imports in `auth.rs`: add `use axum::extract::OriginalUri;`, `use axum::http::{HeaderValue, StatusCode};` (merge with the existing `axum::http` line), `use crate::adapters::iam::DpopContext;` (beside `CallerCredential`), and `use paigasus_proto::paigasus::iam::v1::IntrospectResponse;` (beside `IntrospectApiKeyResponse`).

Replace everything from `pub async fn require_iam_auth` through the end of `authorize_self` (the old `:68-207`, now shifted by Task 11-12) with:

```rust
/// Which `Authorization` scheme the client used, for the challenge (SMA-700 § 4.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemeUsed {
    /// No usable credential (absent, unknown scheme, empty token, or `DPoP` while DPoP is off).
    None,
    Bearer,
    Dpop,
}

impl SchemeUsed {
    fn of(credentials: Option<&Credentials>) -> Self {
        match credentials {
            None => SchemeUsed::None,
            Some(Credentials::Bearer(_)) => SchemeUsed::Bearer,
            Some(Credentials::Dpop { .. }) => SchemeUsed::Dpop,
        }
    }
}

const CHALLENGE_INVALID_PROOF: &str = "DPoP error=\"invalid_dpop_proof\", algs=\"ES256 RS256\"";
const CHALLENGE_INVALID_TOKEN: &str = "DPoP error=\"invalid_token\", algs=\"ES256 RS256\"";
const CHALLENGE_BARE: &str = "DPoP algs=\"ES256 RS256\"";

/// The `WWW-Authenticate` value of a refusal (SMA-700 § 4.10), or `None`. Only when gateway DPoP
/// is on, and only on a 401. The error attribute goes on the scheme the client used (RFC 6750
/// § 3): a Bearer client or one with no credential gets the bare DPoP challenge. With DPoP off the
/// gateway sends no challenge, as before (D11). `algs` is D8's list.
pub fn dpop_challenge(dpop_enabled: bool, scheme: SchemeUsed, err: &GatewayError) -> Option<HeaderValue> {
    if !dpop_enabled || err.status() != StatusCode::UNAUTHORIZED {
        return None;
    }
    let value = match (scheme, err) {
        (SchemeUsed::Dpop, GatewayError::InvalidDpopProof) => CHALLENGE_INVALID_PROOF,
        (SchemeUsed::Dpop, _) => CHALLENGE_INVALID_TOKEN,
        (SchemeUsed::Bearer | SchemeUsed::None, _) => CHALLENGE_BARE,
    };
    Some(HeaderValue::from_static(value))
}

/// Render a refusal, with the challenge appended after `into_response` (`IntoResponse` does not
/// change; the gateway sends one line, through `append`).
fn reject(dpop_enabled: bool, scheme: SchemeUsed, err: GatewayError) -> Response {
    let challenge = dpop_challenge(dpop_enabled, scheme, &err);
    let mut response = err.into_response();
    if let Some(value) = challenge {
        response.headers_mut().append(header::WWW_AUTHENTICATE, value);
    }
    response
}

/// The proof value of a DPoP request. A missing or invalid `DPoP` header is a 401 at once, with
/// no IAM call (§ 4.10).
fn proof_value(proof: ProofHeader) -> Result<String, GatewayError> {
    match proof {
        ProofHeader::One(proof) => Ok(proof),
        ProofHeader::Missing | ProofHeader::Invalid => Err(GatewayError::InvalidDpopProof),
    }
}

/// The DPoP context that IAM checks (§ 4.10, D5): the proof, the method, and the path as the
/// gateway received it, with no query (`OriginalUri`, so a nested router cannot shorten it).
fn dpop_context(req: &Request, proof: &str) -> DpopContext {
    let uri = req.extensions().get::<OriginalUri>().map_or(req.uri(), |original| &original.0);
    DpopContext {
        proof: proof.to_owned(),
        method: req.method().as_str().to_owned(),
        path: uri.path().to_owned(),
    }
}

/// Authenticate + authorize a request before it reaches the protected handler. Wired via
/// `from_fn_with_state(AuthState { iam, dpop_enabled }, require_iam_auth)`. On success the request
/// carries a [`CallerContext`] extension; on any failure it returns the mapped [`GatewayError`],
/// with the DPoP challenge when gateway DPoP is on.
pub async fn require_iam_auth(State(auth): State<AuthState>, req: Request, next: Next) -> Response {
    let credentials = credentials(req.headers(), auth.dpop_enabled);
    let scheme = SchemeUsed::of(credentials.as_ref());
    match iam_auth(auth.iam.as_ref(), credentials, req, next).await {
        Ok(response) => response,
        Err(err) => reject(auth.dpop_enabled, scheme, err),
    }
}

async fn iam_auth(iam: &dyn Iam, credentials: Option<Credentials>, req: Request, next: Next) -> Result<Response, GatewayError> {
    match credentials {
        None => Err(GatewayError::MissingBearer),
        Some(Credentials::Bearer(token)) => bearer_iam_auth(iam, token, req, next).await,
        Some(Credentials::Dpop { token, proof }) => {
            // D15: no API-key leg on the DPoP scheme; API keys are not bound to a key, and the
            // proof goes to IAM on the token leg only.
            let proof = proof_value(proof)?;
            let context = dpop_context(&req, &proof);
            let started = Instant::now();
            let user = match iam.introspect_token(&token, Some(context)).await {
                Ok(resp) if resp.status == "active" => {
                    record_iam_call("introspect_token", "ok", started);
                    resp
                }
                Ok(_) => {
                    record_iam_call("introspect_token", "denied", started);
                    return Err(GatewayError::InvalidCredential);
                }
                Err(err) => {
                    record_iam_call("introspect_token", iam_result(&err), started);
                    return Err(introspect_error(err));
                }
            };
            // IAM's IsAuthorized follow-up: the same token and the same proof bytes (§ 4.8).
            oidc_caller(iam, CallerCredential::Dpop { token, proof }, user, req, next).await
        }
    }
}

/// The Bearer scheme: the API-key leg, then the OIDC leg (SMA-635), unchanged.
async fn bearer_iam_auth(iam: &dyn Iam, token: String, req: Request, next: Next) -> Result<Response, GatewayError> {
    // 2. The API-key leg. An `active` key continues on the unchanged API-key path. Anything else
    //    falls through to the OIDC leg; `api_key_inconclusive` records whether this leg failed to
    //    reach a VERDICT, with the same rule `require_authenticated` uses.
    let started = Instant::now();
    let mut api_key_inconclusive = false;
    match iam.introspect_api_key(&token).await {
        Ok(resp) if resp.status == "active" => {
            record_iam_call("introspect", "ok", started);
            return api_key_caller(iam, &token, resp, req, next).await;
        }
        // IAM answered, and the answer was "not active" — a verdict, not an outage.
        Ok(_) => record_iam_call("introspect", "denied", started),
        Err(err) => {
            let label = iam_result(&err);
            api_key_inconclusive = introspect_error(err) == GatewayError::IamUnavailable;
            record_iam_call("introspect", label, started);
        }
    }

    // 3. The OIDC leg. Unlike `require_authenticated`, `identity-not-provisioned` is REJECTED
    //    here: a user with no provisioned identity has no grants. `introspect_error` maps that
    //    `PermissionDenied` to a 401, and `preserve_outage` widens it to a 503 when the key leg
    //    never reached a verdict.
    let started = Instant::now();
    let user = match iam.introspect_token(&token, None).await {
        Ok(resp) if resp.status == "active" => {
            record_iam_call("introspect_token", "ok", started);
            resp
        }
        Ok(_) => {
            record_iam_call("introspect_token", "denied", started);
            return Err(preserve_outage(api_key_inconclusive, GatewayError::InvalidCredential));
        }
        Err(err) => {
            let label = iam_result(&err);
            let mapped = introspect_error(err);
            record_iam_call("introspect_token", label, started);
            return Err(preserve_outage(api_key_inconclusive, mapped));
        }
    };
    oidc_caller(iam, CallerCredential::Bearer(token), user, req, next).await
}

/// Steps 4-6 of the OIDC leg, shared by the Bearer and the DPoP scheme: the organization, the
/// self-query, the caller context.
async fn oidc_caller(iam: &dyn Iam, caller: CallerCredential, user: IntrospectResponse, mut req: Request, next: Next) -> Result<Response, GatewayError> {
    // 4. The organization (spec §4.2). Every membership node and every grant scope is evidence
    //    for inference; the header, when present, wins and is checked by IAM in step 5.
    let org_prn = {
        let node_prns: Vec<&str> = user
            .memberships
            .iter()
            .map(|m| m.node_prn.as_str())
            .chain(user.role_grants.iter().map(|g| g.scope_prn.as_str()))
            .collect();
        org_header(req.headers()).and_then(|header| resolve_org(header, &node_prns).map_err(GatewayError::from))?.canonical()
    };
    let principal_prn = user.principal_prn;

    // 5. The self-query against the ORG (D4): an org UUID that does not exist is a Deny, not an
    //    error, so the answer does not show whether the org exists.
    authorize_self(iam, &caller, &principal_prn, &org_prn, None).await?;

    // 6. Attach the resolved caller and proceed.
    req.extensions_mut().insert(CallerContext {
        principal_prn,
        scope_prn: org_prn,
        credential: Credential::Oidc,
    });
    Ok(next.run(req).await)
}

/// The API-key path, unchanged since SMA-446 apart from the D5 warning: the key's own
/// `scope_prn` is the scope, and a `paigasus-org` header is never read for it.
async fn api_key_caller(iam: &dyn Iam, token: &str, resp: IntrospectApiKeyResponse, mut req: Request, next: Next) -> Result<Response, GatewayError> {
    if resp.scope_prn.is_empty() {
        // A missing scope is a plumbing bug (introspect should always return one), surfaced as a
        // distinct 500 diagnostic rather than a silent deny. The response body stays generic.
        tracing::error!(
            principal_prn = %resp.principal_prn,
            key_id = %resp.key_id,
            "introspect returned an empty scope_prn — IAM plumbing bug (SMA-446 D11)"
        );
        return Err(GatewayError::MissingScope);
    }
    if req.headers().contains_key(ORG_HEADER) {
        // D5. The header VALUE is never logged: it is caller input.
        tracing::warn!(key_id = %resp.key_id, "paigasus-org ignored for an API key");
    }
    authorize_self(iam, &CallerCredential::ApiKey(token.to_owned()), &resp.principal_prn, &resp.scope_prn, Some(&resp.key_id)).await?;
    req.extensions_mut().insert(CallerContext {
        principal_prn: resp.principal_prn,
        scope_prn: resp.scope_prn,
        credential: Credential::ApiKey { key_id: resp.key_id },
    });
    Ok(next.run(req).await)
}

/// The D9 self-query, shared by every credential: the caller's OWN credential, the caller's OWN
/// introspected principal, `InvokeModel`, and the resolved scope.
async fn authorize_self(iam: &dyn Iam, caller: &CallerCredential, principal_prn: &str, scope_prn: &str, key_id: Option<&str>) -> Result<(), GatewayError> {
    let started = Instant::now();
    match iam.is_authorized_self(caller, principal_prn, INVOKE_MODEL_ACTION, scope_prn).await {
        Ok(true) => {
            record_iam_call("authorize", "ok", started);
            Ok(())
        }
        Ok(false) => {
            record_iam_call("authorize", "denied", started);
            Err(GatewayError::AuthzDenied)
        }
        Err(err) => {
            record_iam_call("authorize", iam_result(&err), started);
            let mapped = authz_error(err);
            if mapped == GatewayError::Internal {
                // An exposure-gate denial of a self-query should be impossible — log it, since this
                // is the single most important thing to see (a broken D9 self-query). `key_id` is
                // logged plainly for an API key; an OIDC caller has none, so the field reads `-`.
                tracing::error!(
                    principal_prn = %principal_prn,
                    key_id = %key_id.unwrap_or("-"),
                    "self-query IsAuthorized returned an unexpected error mapped to 500 — possible broken self-query (SMA-446 D9)"
                );
            }
            Err(mapped)
        }
    }
}
```

Replace `require_authenticated` (its whole body, keeping the long doc comment above it) with:

```rust
pub async fn require_authenticated(State(auth): State<AuthState>, req: Request, next: Next) -> Response {
    let credentials = credentials(req.headers(), auth.dpop_enabled);
    let scheme = SchemeUsed::of(credentials.as_ref());
    match authenticated(auth.iam.as_ref(), credentials, req, next).await {
        Ok(response) => response,
        Err(err) => reject(auth.dpop_enabled, scheme, err),
    }
}

async fn authenticated(iam: &dyn Iam, credentials: Option<Credentials>, req: Request, next: Next) -> Result<Response, GatewayError> {
    match credentials {
        None => Err(GatewayError::MissingBearer),
        Some(Credentials::Bearer(token)) => bearer_authenticated(iam, &token, req, next).await,
        // D15: no API-key leg. The `identity-not-provisioned` rule stays: IAM answers it only
        // after the proof check passed (D18).
        Some(Credentials::Dpop { token, proof }) => {
            let proof = proof_value(proof)?;
            let context = dpop_context(&req, &proof);
            token_authenticated(iam, &token, Some(context), false, req, next).await
        }
    }
}

async fn bearer_authenticated(iam: &dyn Iam, token: &str, req: Request, next: Next) -> Result<Response, GatewayError> {
    let started = Instant::now();
    // Whether the API-key leg failed to reach a VERDICT, as opposed to reaching a rejection.
    // An outage here must not be laundered into a `401` by the fallback below (see
    // `preserve_outage`).
    let mut api_key_inconclusive = false;
    match iam.introspect_api_key(token).await {
        Ok(resp) if resp.status == "active" => {
            record_iam_call("introspect", "ok", started);
            return Ok(next.run(req).await);
        }
        // IAM answered, and the answer was "not active" — a verdict, not an outage.
        Ok(_) => record_iam_call("introspect", "denied", started),
        Err(err) => {
            // `IamError` is not `Clone`, so compute the bounded metric label before
            // `introspect_error` consumes `err`.
            let label = iam_result(&err);
            api_key_inconclusive = introspect_error(err) == GatewayError::IamUnavailable;
            record_iam_call("introspect", label, started);
        }
    }
    token_authenticated(iam, token, None, api_key_inconclusive, req, next).await
}

/// The token leg of discovery: an active identity, or a VALIDATED but unprovisioned one (see the
/// doc of [`require_authenticated`]), reaches the handler.
async fn token_authenticated(iam: &dyn Iam, token: &str, dpop: Option<DpopContext>, api_key_inconclusive: bool, req: Request, next: Next) -> Result<Response, GatewayError> {
    let started = Instant::now();
    match iam.introspect_token(token, dpop).await {
        Ok(resp) if resp.status == "active" => {
            record_iam_call("introspect_token", "ok", started);
            Ok(next.run(req).await)
        }
        // Belt-and-braces: a success carrying a non-active status is a rejected credential.
        Ok(_) => {
            record_iam_call("introspect_token", "denied", started);
            Err(preserve_outage(api_key_inconclusive, GatewayError::InvalidCredential))
        }
        // A validated-but-unprovisioned identity. Recorded as "denied" rather than "ok".
        Err(IamError::Rpc(ref status)) if is_identity_not_provisioned(status) => {
            record_iam_call("introspect_token", "denied", started);
            Ok(next.run(req).await)
        }
        Err(err) => {
            let label = iam_result(&err);
            let mapped = introspect_error(err);
            record_iam_call("introspect_token", label, started);
            Err(preserve_outage(api_key_inconclusive, mapped))
        }
    }
}
```

- [ ] **Step 5: Implement the reason reads and the mapping**

After the `PROVISIONING_FAILED` static, add:

```rust
static INVALID_DPOP_PROOF: LazyLock<String> = LazyLock::new(|| {
    paigasus_proto::paigasus::common::v1::ErrorReason::InvalidDpopProof
        .as_wire_reason()
        .expect("a declared reason is never the sentinel")
});
static DPOP_QUOTA_EXCEEDED: LazyLock<String> = LazyLock::new(|| {
    paigasus_proto::paigasus::common::v1::ErrorReason::DpopQuotaExceeded
        .as_wire_reason()
        .expect("a declared reason is never the sentinel")
});
```

Replace `iam_reason` with two functions:

```rust
/// The `ErrorInfo` reason of an IAM `Status`, read only on the IAM domain — never the message
/// string — and with no log line. For `Unauthenticated` and `ResourceExhausted` (SMA-700), where a
/// status with no `ErrorInfo` is ordinary and must not write the SMA-504 skew warning.
fn iam_reason_quiet(status: &Status) -> Option<String> {
    let details = status.get_error_details();
    let info = details.error_info()?;
    (info.domain == *paigasus_proto::error::IAM_DOMAIN).then(|| info.reason.clone())
}

/// [`iam_reason_quiet`] for a `PermissionDenied`: a status with no `ErrorInfo` there is version
/// skew (IAM must roll before the gateway, SMA-504), so it writes one warning.
fn iam_reason(status: &Status) -> Option<String> {
    if status.get_error_details().error_info().is_none() {
        tracing::warn!("IAM returned a PermissionDenied with no ErrorInfo — rolling-upgrade skew? (SMA-504)");
        return None;
    }
    iam_reason_quiet(status)
}

/// IAM's DPoP quota refusal (SMA-700 D10): `ResourceExhausted` with `dpop-quota-exceeded`.
fn is_dpop_quota(status: &Status) -> bool {
    status.code() == Code::ResourceExhausted && iam_reason_quiet(status).as_deref() == Some(DPOP_QUOTA_EXCEEDED.as_str())
}

/// The `RetryInfo` delay in whole seconds, at least 1; 1 when IAM sent none.
fn retry_after_secs(status: &Status) -> u32 {
    status
        .get_error_details()
        .retry_info()
        .and_then(|info| info.retry_delay)
        .map_or(1, |delay| u32::try_from(delay.as_secs()).unwrap_or(u32::MAX).max(1))
}

/// An IAM `Unauthenticated`: a refused DPoP proof keeps its own code; every other one is the
/// credential rejection it was before.
fn unauthenticated_error(status: &Status) -> GatewayError {
    if iam_reason_quiet(status).as_deref() == Some(INVALID_DPOP_PROOF.as_str()) {
        GatewayError::InvalidDpopProof
    } else {
        GatewayError::InvalidCredential
    }
}

/// An IAM `ResourceExhausted`: the DPoP quota is the gateway's own 429 `rate-limited` with
/// `Retry-After` (D10); anything else stays an IAM failure.
fn resource_exhausted_error(status: &Status) -> GatewayError {
    if is_dpop_quota(status) {
        GatewayError::RateLimited { retry_after_secs: retry_after_secs(status) }
    } else {
        GatewayError::IamUnavailable
    }
}
```

In `iam_result`, add one arm before `IamError::Rpc(_) => "error",`:

```rust
        // SMA-700 D10: a quota hit is a verdict about one client, not an outage.
        IamError::Rpc(status) if is_dpop_quota(status) => "denied",
```

`introspect_error`, the `IamError::Rpc(status) => match status.code() { … }` block becomes:

```rust
        IamError::Rpc(status) => match status.code() {
            Code::Unauthenticated => unauthenticated_error(&status),
            // Inactive/unprovisioned principal on either introspect leg — a client-auth failure,
            // not a 403.
            Code::PermissionDenied => GatewayError::InvalidCredential,
            Code::ResourceExhausted => resource_exhausted_error(&status),
            Code::Unavailable | Code::DeadlineExceeded | Code::Internal => GatewayError::IamUnavailable,
            _ => GatewayError::IamUnavailable,
        },
```

`authz_error`, the same block becomes:

```rust
        IamError::Rpc(status) => match status.code() {
            Code::PermissionDenied => match iam_reason(&status).as_deref() {
                Some(reason) if reason == PRINCIPAL_INACTIVE.as_str() || reason == PROVISIONING_FAILED.as_str() => GatewayError::InvalidCredential,
                _ => GatewayError::Internal,
            },
            Code::Unauthenticated => unauthenticated_error(&status),
            Code::ResourceExhausted => resource_exhausted_error(&status),
            Code::Unavailable | Code::DeadlineExceeded | Code::Internal => GatewayError::IamUnavailable,
            _ => GatewayError::IamUnavailable,
        },
```

Update the module doc (`:1-40`): add a paragraph "## The DPoP scheme (SMA-700)" with three sentences: with gateway DPoP on, `Authorization: DPoP <token>` plus one `DPoP` header skips the API-key leg and sends the proof, the method and the path to `Introspect`; the self-query then sends the same token and proof as IAM's `IsAuthorized` follow-up; every 401 carries one `WWW-Authenticate: DPoP` line (`dpop_challenge`).

- [ ] **Step 6: The `chat_proxy` end-to-end test**

In `tests/chat_proxy.rs`, add `use std::sync::Mutex;` and:

```rust
// ---- SMA-700: one DPoP request end to end ---------------------------------------------------

/// A DPoP user. The API-key leg must not run (D15). `introspect_token` and `is_authorized_self`
/// record what the gateway sent.
#[derive(Default)]
struct DpopIam {
    calls: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl Iam for DpopIam {
    async fn introspect_api_key(&self, _token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
        panic!("D15: the DPoP scheme skips the API-key leg")
    }

    async fn is_authorized_self(&self, caller: &CallerCredential, principal_prn: &str, _action: &str, resource_prn: &str) -> Result<bool, IamError> {
        let caller = match caller {
            CallerCredential::Dpop { token, proof } => format!("dpop {token} {proof}"),
            other => format!("{other:?}"),
        };
        self.calls.lock().unwrap().push(format!("authorize {caller} {principal_prn} {resource_prn}"));
        Ok(true)
    }

    async fn introspect_token(&self, token: &str, dpop: Option<DpopContext>) -> Result<IntrospectResponse, IamError> {
        let dpop = dpop.expect("a DPoP request forwards its context");
        self.calls.lock().unwrap().push(format!("introspect {token} {} {} {}", dpop.method, dpop.path, dpop.proof));
        Ok(IntrospectResponse {
            principal_prn: USER_PRN.to_owned(),
            status: "active".to_owned(),
            issuer: "https://issuer.example.com".to_owned(),
            subject: "user-1".to_owned(),
            expires_at: None,
            memberships: vec![Membership {
                principal_prn: USER_PRN.to_owned(),
                node_prn: USER_ORG_PRN.to_owned(),
                ..Default::default()
            }],
            role_grants: Vec::new(),
        })
    }
}

#[tokio::test]
async fn one_dpop_request_end_to_end() {
    let canned = r#"{"id":"chatcmpl-dpop","object":"chat.completion","choices":[{"index":0}]}"#;
    let mock = MockOpenAi::spawn_json(StatusCode::OK, canned).await;
    let iam = DpopIam::default();
    let calls = iam.calls.clone();
    let cfg = OpenAiConfig {
        base_url: mock.base_url.clone(),
        api_key: SecretString::from(REAL_KEY.to_string()),
        extra_ca_bundle_path: None,
    };
    let openai = OpenAiClient::new(&cfg, Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(300)).expect("client builds");
    let app = router(AppState {
        iam: Arc::new(iam),
        openai: Arc::new(openai),
        max_request_bytes: ONE_MIB,
        capabilities: paigasus_gateway::service_info::Capabilities { chat_stream: true },
        limits: None,
        dpop_enabled: true,
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("DPoP {USER_TOKEN}"))
        .header("DPoP", "proof.for.chat")
        .body(Body::from(NON_STREAM_BODY))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            format!("introspect {USER_TOKEN} POST /v1/chat/completions proof.for.chat"),
            format!("authorize dpop {USER_TOKEN} proof.for.chat {USER_PRN} {USER_ORG_PRN}"),
        ]
    );
    assert_eq!(mock.recorded().unwrap().body, Bytes::from(NON_STREAM_BODY), "the request reached the upstream");
}
```

- [ ] **Step 7: Run the gateway**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
cargo fmt
cargo nextest run --locked -p paigasus-gateway --lib --test chat_proxy --test service_info --test metrics --test limits_http
cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings
python3 ../ci/error-registry/check.py --single-site
```

Expected: all pass; the registry gate is green with no MANIFEST change (`auth.rs` and `error.rs` are already listed).

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): the gateway DPoP flow, its error mapping and the challenge (SMA-700)

The DPoP scheme skips the API-key leg, forwards the proof, the method and
the path with no query, and self-queries with the same token and proof.
IAM's invalid-dpop-proof is a 401 with its own code; the DPoP quota is a
429 rate-limited with Retry-After and the metric label denied. With DPoP
on, every 401 carries one WWW-Authenticate: DPoP line.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: The chart — values, projection, refusals, env rows, golden case

The IAM env list is rendered in `templates/backend-deployment.yaml` (lines 87-155); `_iam-backend.tpl` holds the helpers. No `ci/helm-render/fixtures/` file copies `_iam-backend.tpl`, `values.yaml` or `backend-deployment.yaml`, so no fixture needs a re-sync. No new `tests/*.sh` script is added, so `CHART_SCRIPT_FLOOR` stays 7. The default render must stay byte-identical: the two existing goldens must not change.

**Files:**
- Modify: `charts/paigasus/values.yaml` (after `extraEnv: []`, line 71)
- Modify: `charts/paigasus/templates/_iam-backend.tpl` (`iamReservedEnv` line 17; `validateIamBackend` line 104; new helpers at the end)
- Modify: `charts/paigasus/templates/backend-deployment.yaml` (before `  replicas: 1`, line 23; after the bootstrap-admins block, line 151)
- Modify: `charts/paigasus/tests/refusals.sh` (before line 309)
- Modify: `charts/paigasus/tests/env.sh` (`check_boot_reserved` lines 638-639; a new D group before line 809)
- Modify: `charts/paigasus/tests/render.sh` (after line 72)
- Create: `charts/paigasus/tests/golden/iam-dpop.yaml` (generated)
- Modify: `charts/paigasus/README.md` (the refusals list near line 68; a new section after line 270; the golden-files section lines 327-341)

**Interfaces:**
- Consumes: the env names and forms of Task 6 (`IAM_AUTHN__DPOP__ENABLED=true`, `IAM_AUTHN__DPOP__FORWARDED_BASE_URLS=["…","…"]`).
- Produces: values `zones.iam.backend.dpop.enabled` (default `false`) and `zones.iam.backend.dpop.forwardedBaseUrls` (default `[]`); helpers `paigasus.iamDpopEnabled`, `paigasus.iamDpopForwardedBaseUrls`, `paigasus.validateIamDpop`.

- [ ] **Step 1: Write the failing chart rows**

`tests/refusals.sh`, before line 309 (`expect_render "iam only" …`):

```bash
# SMA-700 (spec § 4.11). zones.iam.backend.dpop copies the IamConfig::validate rules for the URL
# list, because a refused boot stops the one IAM replica. Each needle carries the key path.
DPOP=zones.iam.backend.dpop
expect_fail "dpop on with no URL" "zones.iam.backend.dpop.enabled is true and zones.iam.backend.dpop.forwardedBaseUrls is empty" \
  --set "$DPOP.enabled=true"
expect_fail "dpop enabled not a bool" "zones.iam.backend.dpop.enabled must be true or false" \
  --set-string "$DPOP.enabled=yes"
expect_fail "dpop URLs not a list" "zones.iam.backend.dpop.forwardedBaseUrls must be a list of URLs" \
  --set "$DPOP.forwardedBaseUrls=https://gw.example.test"
expect_fail "dpop URL http not loopback" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"http://gw.example.test\": use https" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=http://gw.example.test"
expect_fail "dpop URL ftp" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"ftp://gw.example.test\": use https" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=ftp://gw.example.test"
expect_fail "dpop URL with no scheme" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"gw.example.test\": use https" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=gw.example.test"
expect_fail "dpop URL with user info" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"https://user@gw.example.test\": it must have no query, fragment or user info" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://user@gw.example.test"
expect_fail "dpop URL with a fragment" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"https://gw.example.test/#x\": it must have no query, fragment or user info" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://gw.example.test/#x"
# A query holds `=`, which --set reads as a second key. A values file carries it as written.
DPOP_QUERY="$(mktemp)"
printf 'zones:\n  iam:\n    backend:\n      dpop:\n        enabled: true\n        forwardedBaseUrls: ["https://gw.example.test/?a=1"]\n' >"$DPOP_QUERY"
expect_fail "dpop URL with a query" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"https://gw.example.test/?a=1\": it must have no query, fragment or user info" \
  -f "$DPOP_QUERY"
rm -f "$DPOP_QUERY"
expect_fail "dpop URL with a space" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"https://gw.example.test/a b\": use printable ASCII only" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://gw.example.test/a b"
# Decision P8: a bad entry is refused also while DPoP is off.
expect_fail "dpop off with a bad URL" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"http://gw.example.test\": use https" \
  --set "$DPOP.forwardedBaseUrls[0]=http://gw.example.test"
expect_fail "extraEnv sets the DPoP switch" "the chart sets IAM_AUTHN__DPOP__ENABLED itself" \
  --set 'zones.iam.backend.extraEnv[0].name=IAM_AUTHN__DPOP__ENABLED' --set 'zones.iam.backend.extraEnv[0].value=true'
expect_render "dpop https with a prefix" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://gw.example.test" --set "$DPOP.forwardedBaseUrls[1]=https://edge.example.test/api/"
expect_render "dpop loopback http" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=http://localhost:8088" --set "$DPOP.forwardedBaseUrls[1]=http://127.0.0.1:8088"
# Review Focus 5.
expect_render "dpop IPv6 loopback http" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=http://[::1]:8088"
expect_render "dpop reuse-values-no-key" --set "$DPOP=null"
```

`tests/env.sh` B6 — in `check_boot_reserved`, replace the `check_boot …` call (lines 638-639) with:

```bash
  check_boot "$label" "[{issuer=\"$ISS\",subject=\"s\"}]" - "$reserved" \
    --set oidc.caBundle.existingConfigMap=idp-ca --set "$ADMIN0.issuer=$ISS" --set "$ADMIN0.subject=s" \
    --set zones.iam.backend.dpop.enabled=true --set 'zones.iam.backend.dpop.forwardedBaseUrls[0]=https://gw.example.test'
```

and update its comment (lines 628-629): "the chart's own names, with every optional entry on (the CA bundle, one admin, and DPoP since SMA-700), equal the paigasus.iamReservedEnv list."

`tests/env.sh`, before `if [ "$ec" -eq 0 ]; then echo "== chart env OK =="; fi` (line 809), add:

```bash
# zones.iam.backend.dpop (SMA-700). Renders go to a file, as for check_boot. One row per property:
#   D1 default      No DPoP env renders.
#   D2 on           IAM_AUTHN__DPOP__ENABLED is "true", and IAM_AUTHN__DPOP__FORWARDED_BASE_URLS is
#                   the exact figment inline string. config.rs parses this form in
#                   dpop_env_in_the_chart_form_parses.
#   D3 reuse-values-no-key
#                   `--set zones.iam.backend.dpop=null`. No entry renders.
#   D4 off-with-urls
#                   A URL list with enabled false. No entry renders.
#   D5 restart-scope
#                   Turning DPoP on changes the IAM pod template and no console pod template.
# A seventh row counter reds the script when a D row call line is deleted.
DPOP_ROWS=0
DPOP_ROWS_WANT=5
DPOP=zones.iam.backend.dpop

# check_dpop <label> <want FORWARDED_BASE_URLS value or -> [helm args...]
check_dpop() {
  local label="$1" urls="$2"; shift 2
  local out
  DPOP_ROWS=$((DPOP_ROWS + 1))
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" >"$TMP/dpop.yaml" 2>"$TMP/dpop.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/dpop.err"; ec=1; return 0
  fi
  if ! out="$(URLS="$urls" python3 -c '
import os, sys, yaml
with open(sys.argv[1]) as fh:
    docs = [d for d in yaml.safe_load_all(fh) if d]
problems = []
deps = [d for d in docs if d.get("kind") == "Deployment"
        and d["spec"]["template"]["metadata"]["labels"].get("app.kubernetes.io/name") == "iam-backend"]
if len(deps) != 1:
    problems.append(str(len(deps)) + " iam-backend Deployment(s), want 1")
else:
    env = {e.get("name"): e.get("value") for e in deps[0]["spec"]["template"]["spec"]["containers"][0].get("env") or []}
    names = ("IAM_AUTHN__DPOP__ENABLED", "IAM_AUTHN__DPOP__FORWARDED_BASE_URLS")
    if os.environ["URLS"] == "-":
        problems += [name + " renders; want it absent" for name in names if name in env]
    else:
        if env.get(names[0]) != "true":
            problems.append(names[0] + " is " + repr(env.get(names[0])) + ", want \"true\"")
        if env.get(names[1]) != os.environ["URLS"]:
            problems.append(names[1] + " is " + repr(env.get(names[1])) + ", want " + repr(os.environ["URLS"]))
print("|".join(problems) if problems else "OK")' "$TMP/dpop.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# check_dpop_restart <label>: DPoP on changes the IAM pod template only. It reuses
# check_boot_restart, so it also counts as a B row; the B floor check ran above, as for M6.
check_dpop_restart() {
  local label="$1"
  DPOP_ROWS=$((DPOP_ROWS + 1))
  check_boot_restart "$label" --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://gw.example.test"
}

check_dpop "D1 default"             -
check_dpop "D2 on"                  '["https://gw.example.test","https://edge.example.test/api"]' \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://gw.example.test" --set "$DPOP.forwardedBaseUrls[1]=https://edge.example.test/api"
check_dpop "D3 reuse-values-no-key" - --set "$DPOP=null"
check_dpop "D4 off-with-urls"       - --set "$DPOP.forwardedBaseUrls[0]=https://gw.example.test"
check_dpop_restart "D5 restart-scope"

if [ "$DPOP_ROWS" -lt "$DPOP_ROWS_WANT" ]; then
  echo "FAIL [dpop rows]: $DPOP_ROWS dpop row(s) ran, want $DPOP_ROWS_WANT"; ec=1
fi
```

Also add `D` to the header comment of `env.sh` that lists the row groups (lines 1-94), one line: `#   D rows (SMA-700): zones.iam.backend.dpop; see the block above the end of the file.`

`tests/render.sh`, after the `iam-and-gateway-httproute` case (line 72):

```bash
# SMA-700. A byte pin of the DPoP projection: two URLs, %q-quoted, and the comment line.
render_one "iam-dpop" --set zones.gateway.enabled=false \
  --set zones.iam.backend.dpop.enabled=true \
  --set 'zones.iam.backend.dpop.forwardedBaseUrls[0]=https://gw.example.test' \
  --set 'zones.iam.backend.dpop.forwardedBaseUrls[1]=https://edge.example.test/api'
```

- [ ] **Step 2: Run them to see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/charts/paigasus
bash tests/refusals.sh --set ingress.host=console.example.test 2>&1 | grep -E "FAIL|OK" | head -30
bash tests/env.sh 2>&1 | grep -E "FAIL|OK" | head -30
bash tests/render.sh 2>&1 | tail -5
```

Expected: the `dpop …` refusal rows FAIL (rendered, expected a refusal), B6 and D2 FAIL, and `iam-dpop` FAILs with "no golden file". Use the bash the CI uses for these scripts; they carry 3.2-safe guards, so `/bin/bash` works.

- [ ] **Step 3: `values.yaml`**

After `      extraEnv: []` (line 71), add:

```yaml
      # NOT required. The DPoP proof check of the gateway path (SMA-700). With enabled true, IAM
      # checks the DPoP proof (RFC 9449) that the gateway forwards. The gateway needs
      # GATEWAY_DPOP__ENABLED too; the chart does not deploy the gateway. Turn DPoP on in IAM first.
      #   forwardedBaseUrls  REQUIRED when enabled: the public URLs at which clients reach the
      #                      gateway, each with any path prefix that a proxy removes. https only, or
      #                      http on localhost, 127.x.x.x or [::1]. No query, fragment or user info.
      #                      Printable ASCII with no space, " or \.
      # The replay store is in memory, so IAM must stay at one replica (it is pinned). A change
      # restarts the IAM pod. See docs/ops/RUNBOOK-chart.md § 6.
      dpop:
        enabled: false
        forwardedBaseUrls: []
```

- [ ] **Step 4: `_iam-backend.tpl`**

Line 17 (the reserved list) becomes:

```
IAM_HTTP_ADDR IAM_GRPC_ADDR IAM_MIGRATION__LOCK_WAIT_SECS IAM_DATABASE_URL IAM_AUTHN__ISSUERS IAM_API_KEYS__PEPPER IAM_AUTHN__EXTRA_CA_BUNDLE_PATH IAM_AUTHZ__BOOTSTRAP_ADMINS IAM_AUTHN__DPOP__ENABLED IAM_AUTHN__DPOP__FORWARDED_BASE_URLS
```

In `paigasus.validateIamBackend`, after `{{- include "paigasus.validateIdTokenMarkerClaims" . -}}` (line 104), add:

```
{{- include "paigasus.validateIamDpop" . -}}
```

and extend its header comment (line 41-42): "…and for oidc.idTokenMarkerClaims (SMA-703) and zones.iam.backend.dpop (SMA-700, paigasus.validateIamDpop below)."

At the end of the file, add:

```
{{/*
paigasus.iamDpopEnabled: "true" when zones.iam.backend.dpop.enabled is true, else "" (SMA-700).
The key can be absent or nil under `helm upgrade --reuse-values` from a release made before it;
then DPoP is off. paigasus.validateIamDpop has already refused a value that is not a map or a bool.
*/}}
{{- define "paigasus.iamDpopEnabled" -}}
{{- $dpop := dig "dpop" dict .Values.zones.iam.backend -}}
{{- if and (kindIs "map" $dpop) (eq (toString (dig "enabled" false $dpop)) "true") -}}
true
{{- end -}}
{{- end -}}

{{/*
paigasus.iamDpopForwardedBaseUrls: the value of IAM_AUTHN__DPOP__FORWARDED_BASE_URLS (SMA-700), in
the figment inline form that IAM_AUTHN__ISSUERS uses: each URL quoted with %q, joined by a comma,
in brackets. The test dpop_env_in_the_chart_form_parses in rs/crates/services/paigasus-iam/src/config.rs
parses this exact form. Called only when paigasus.iamDpopEnabled is "true", so the map exists.
*/}}
{{- define "paigasus.iamDpopForwardedBaseUrls" -}}
{{- $dpop := dig "dpop" dict .Values.zones.iam.backend -}}
{{- $quoted := list -}}
{{- range (dig "forwardedBaseUrls" list $dpop) -}}
{{- $quoted = append $quoted (printf "%q" .) -}}
{{- end -}}
{{- printf "[%s]" (join "," $quoted) -}}
{{- end -}}

{{/*
paigasus.validateIamDpop: the refusals for zones.iam.backend.dpop (SMA-700 § 4.11). A refused boot
stops the one IAM replica (maxSurge 0), so the chart copies IamConfig::validate's rules for the
list: enabled with an empty list; an entry that is not https or loopback http; an entry with a
query, a fragment or user info. Entries are checked also when enabled is false, as IAM checks them.
The character rule is stricter than IAM, as for idTokenMarkerClaims: printable ASCII only, with no
space, no " and no \, because %q writes other characters as escapes that figment does not read.
Loopback http is localhost, a dotted 127.a.b.c, or ::1; IAM also accepts other 127/8 spellings,
so the chart is stricter, never looser. A nil value counts as absent.
*/}}
{{- define "paigasus.validateIamDpop" -}}
{{- $dpop := dig "dpop" dict .Values.zones.iam.backend -}}
{{- if kindIs "invalid" $dpop -}}
{{- $dpop = dict -}}
{{- end -}}
{{- if not (kindIs "map" $dpop) -}}
{{- fail "zones.iam.backend.dpop must be a map with the keys enabled and forwardedBaseUrls" -}}
{{- end -}}
{{- $enabled := dig "enabled" false $dpop -}}
{{- if kindIs "invalid" $enabled -}}
{{- $enabled = false -}}
{{- end -}}
{{- if not (kindIs "bool" $enabled) -}}
{{- fail "zones.iam.backend.dpop.enabled must be true or false" -}}
{{- end -}}
{{- $urls := dig "forwardedBaseUrls" list $dpop -}}
{{- if kindIs "invalid" $urls -}}
{{- $urls = list -}}
{{- end -}}
{{- if not (kindIs "slice" $urls) -}}
{{- fail "zones.iam.backend.dpop.forwardedBaseUrls must be a list of URLs" -}}
{{- end -}}
{{- if and $enabled (not $urls) -}}
{{- fail "zones.iam.backend.dpop.enabled is true and zones.iam.backend.dpop.forwardedBaseUrls is empty. IamConfig::validate refuses it, and IAM does not boot. List the public URLs at which clients reach the gateway" -}}
{{- end -}}
{{- range $i, $u := $urls -}}
{{- if not (kindIs "string" $u) -}}
{{- fail (printf "zones.iam.backend.dpop.forwardedBaseUrls[%d] must be a string" $i) -}}
{{- end -}}
{{- if not (regexMatch `^[!#-\[\]-~]+$` $u) -}}
{{- fail (printf "zones.iam.backend.dpop.forwardedBaseUrls[%d] is %q: use printable ASCII only, with no space, no \" and no \\. IAM cannot read another character from IAM_AUTHN__DPOP__FORWARDED_BASE_URLS" $i $u) -}}
{{- end -}}
{{- $p := urlParse $u -}}
{{- if or $p.query $p.fragment $p.userinfo (contains "?" $u) (contains "#" $u) -}}
{{- fail (printf "zones.iam.backend.dpop.forwardedBaseUrls[%d] is %q: it must have no query, fragment or user info. IamConfig::validate refuses it, and IAM does not boot" $i $u) -}}
{{- end -}}
{{- $host := lower $p.hostname -}}
{{- $loopback := or (eq $host "localhost") (eq $host "::1") (regexMatch `^127\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}$` $host) -}}
{{- if not (and $host (or (eq $p.scheme "https") (and (eq $p.scheme "http") $loopback))) -}}
{{- fail (printf "zones.iam.backend.dpop.forwardedBaseUrls[%d] is %q: use https, or http on localhost, 127.x.x.x or [::1]. IamConfig::validate refuses it, and IAM does not boot" $i $u) -}}
{{- end -}}
{{- end -}}
{{- end -}}
```

- [ ] **Step 5: `backend-deployment.yaml`**

Between `{{- if eq $id "iam" }}` (line 22) and `  replicas: 1` (line 23), insert a template comment. The `{{-` trims only the newline before it, so the render does not change:

```
{{- /*
  SMA-700 (D3, R2): one more reason for this pin. With zones.iam.backend.dpop on, the DPoP replay
  store is in memory, so a second replica would accept one proof once on each, and an IsAuthorized
  follow-up could reach another pod than its Introspect. A template comment, so the golden files
  do not change.
*/}}
```

After the bootstrap-admins block (`{{- end }}` at line 151) and before `{{- with dig "extraEnv" list $z.backend }}`, add:

```
{{- if include "paigasus.iamDpopEnabled" $root }}
            # zones.iam.backend.dpop (SMA-700): authn.dpop. IAM checks the DPoP proof that the
            # gateway forwards, against these public gateway URLs.
            - name: IAM_AUTHN__DPOP__ENABLED
              value: "true"
            - name: IAM_AUTHN__DPOP__FORWARDED_BASE_URLS
              value: {{ include "paigasus.iamDpopForwardedBaseUrls" $root | quote }}
{{- end }}
```

(The comment lines are inside the `if`, so the default render does not change, charts/CLAUDE.md.)

- [ ] **Step 6: Generate the new golden and prove the old ones did not move**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/charts/paigasus
helm version --short
bash tests/render.sh --update
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop status --short charts/paigasus/tests/golden
grep -n "IAM_AUTHN__DPOP" tests/golden/iam-dpop.yaml
```

Expected: `helm version` prints `v3.22.0+g144ca65` (the pinned version; if another version prints, STOP and report — the goldens are a byte pin of that helm). `git status` shows ONLY `?? charts/paigasus/tests/golden/iam-dpop.yaml`: the three existing goldens are unchanged, which proves the default render is byte-identical (AC 6). The `grep` shows the two env names, and the value line reads `value: "[\"https://gw.example.test\",\"https://edge.example.test/api\"]"`.

- [ ] **Step 7: Run every chart script and the gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
out="$(mktemp)"
for s in charts/paigasus/tests/*.sh; do /bin/bash "$s" --set ingress.host=console.example.test >"$out" 2>&1 || { echo "FAIL $s"; tail -20 "$out"; }; done
rm -f "$out"
moon run repo:helm-render
```

Expected: no `FAIL` line; `repo:helm-render` passes (it also runs every negative-control fixture against the live chart, which now carries the new helpers). Note: `refusals.sh` needs the `ingress.host` argument; the other scripts accept it harmlessly through `BASE`. If a script rejects the extra argument, run it with no argument, as `ci/helm-render/run.sh` does (read its call line).

- [ ] **Step 8: `charts/paigasus/README.md`**

After the bootstrap-admins refusal bullet (lines 64-68), add:

```markdown
- **A bad DPoP setting (SMA-700).** `paigasus.validateIamDpop` in `templates/_iam-backend.tpl`
  refuses `zones.iam.backend.dpop.enabled` that is not a boolean, `enabled: true` with an empty
  `forwardedBaseUrls`, and an entry that IAM would refuse at boot: not `https` (or `http` on
  `localhost`, `127.x.x.x` or `[::1]`), or with a query, a fragment or user info. It also refuses a
  character outside printable ASCII, a space, `"` and `\`. It checks the entries also while DPoP is
  off.
```

After the `idTokenMarkerClaims` section (line 270), add:

```markdown
## DPoP on the gateway path (`zones.iam.backend.dpop`)

IAM can check a DPoP proof (RFC 9449) that the gateway forwards (SMA-700). The chart renders the
two values into `IAM_AUTHN__DPOP__ENABLED` and `IAM_AUTHN__DPOP__FORWARDED_BASE_URLS` in
`templates/backend-deployment.yaml`, only when `enabled` is true.

- **Off (the default).** The chart adds nothing. The render is byte-identical to a chart without
  the value. Under `helm upgrade --reuse-values` from an older release, the key is absent, and DPoP
  stays off.
- **On.** `forwardedBaseUrls` lists the public URLs at which clients reach the gateway, each with
  any path prefix that a proxy removes. Each URL is quoted with `%q` in the figment inline form.
  One YAML comment renders above the two entries.
- **The gateway is not deployed by the chart.** Set `GATEWAY_DPOP__ENABLED=true` on the gateway
  after IAM runs with DPoP on.
- **One IAM replica.** The replay store is in memory. The IAM Deployment is pinned to one replica;
  the pin comment in `templates/backend-deployment.yaml` names this reason too.
- A change of the value restarts the IAM pod and no console pod.

`tests/env.sh` holds the rows `D1 default`, `D2 on`, `D3 reuse-values-no-key`, `D4 off-with-urls`
and `D5 restart-scope`, with a seventh row counter. Row `B6 reserved` turns DPoP on, so the
reserved env list includes the two names. `tests/refusals.sh` holds one row for each refusal and
four valid renders. `tests/golden/iam-dpop.yaml` pins the projection. See
`docs/ops/RUNBOOK-chart.md` § 6.
```

In "## The golden files" (lines 327-341): change "re-baselines all three files" to "re-baselines all four files", and add one sentence after the sentence about `iam-and-gateway-httproute.yaml`: "`tests/golden/iam-dpop.yaml` pins the DPoP projection of SMA-700 (DPoP on with two URLs)."

- [ ] **Step 9: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git add charts/paigasus
git commit -m "chore(charts): IAM DPoP values, their projection and their refusals (SMA-700)

zones.iam.backend.dpop.enabled and forwardedBaseUrls render into the two
IAM env names only when enabled. The chart copies IAM's boot rules for
the URL list, reserves both names against extraEnv, and pins the
projection in a new golden. The default render does not change.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 15: Runbook, changelogs, the R8 number, and ADR-0026

**Files:**
- Modify: `docs/ops/RUNBOOK-chart.md` (§ 1 table after the `extraEnv` row, line 21; § 5 table after the `bootstrapAdmins` row, line 136; § 6 the SMA-690 block lines 227-251; § 9 the reserved list lines 600-603)
- Modify: `rs/crates/services/paigasus-iam/CHANGELOG.md` and `rs/crates/services/paigasus-gateway/CHANGELOG.md` (under `## [Unreleased]`)
- Modify: `docs/superpowers/specs/2026-10-04-sma-700-dpop-proof-check-design.md` (§ 4.5 memory-budget bullet, line 291-293; R8 row, line 680)

**Interfaces:**
- Consumes: the R8 measurement `P` bytes per entry and `M` MiB from Task 5 Step 6.
- Produces: documentation only.

- [ ] **Step 1: RUNBOOK § 1 and § 5**

§ 1, after the `zones.iam.backend.extraEnv` row:

```markdown
| `zones.iam.backend.dpop.enabled` | no | Default `false`. `true`: IAM checks the DPoP proof that the gateway forwards (SMA-700). Turn it on in IAM first, then set `GATEWAY_DPOP__ENABLED=true` on the gateway (§ 6) |
| `zones.iam.backend.dpop.forwardedBaseUrls` | when `dpop.enabled` is true | The public URLs at which clients reach the gateway, each with any path prefix that a proxy removes. `https`, or `http` on `localhost`, `127.x.x.x` or `[::1]`. No query, fragment or user info (§ 6) |
```

§ 5, after the `bootstrapAdmins`/`extraEnv` row:

```markdown
| `zones.iam.backend.dpop` | the IAM pod, not the consoles | it changes the env in the IAM pod template (`tests/env.sh` row D5). IAM is not available during the restart, as for `oidc.audience`. The restart also clears the DPoP replay store |
```

- [ ] **Step 2: RUNBOOK § 6, the SMA-690 block made conditional, and the DPoP section**

Replace the paragraph at lines 237-240 ("Keycloak binds an access token when the client sends a `DPoP` header… must log in again without a `DPoP` header.") with:

```markdown
Keycloak binds an access token when the client sends a `DPoP` header to the token endpoint. The
client needs no DPoP setting for this. A bound login stays bound when the client refreshes the
token. **With DPoP off (the default),** a client that calls Paigasus must not send a `DPoP` header
to the token endpoint, and a client that got a bound token must log in again without one. **With
DPoP on (SMA-700, below),** a bound token works on the gateway's protected routes with the `DPoP`
scheme and a proof. The `Bearer` scheme still refuses a bound token, and IAM's own API never
accepts the `DPoP` scheme.
```

After the paragraph that ends "applies as for the refusal of a token that is not an access token." (line 251), add:

```markdown
**DPoP on the gateway path (SMA-700).** IAM and the gateway can accept a DPoP-bound token
(RFC 9449) on the gateway's protected routes, `POST /v1/chat/completions` and
`GET /v1/service-info`. The client sends `Authorization: DPoP <token>` and one `DPoP` header with a
proof for the request. The gateway forwards the proof, the method and the path to IAM. IAM checks
the proof, then the identity.

To turn it on:

1. Set `zones.iam.backend.dpop.enabled: true` and `zones.iam.backend.dpop.forwardedBaseUrls`. List
   each public URL at which clients reach the gateway, with any path prefix that a proxy removes.
   Example: a client calls `https://api.example.com/llm/v1/chat/completions`, and the proxy
   removes `/llm`. Then the entry is `https://api.example.com/llm`. TLS must end in front of the
   gateway. A wrong entry refuses DPoP requests; it does not accept a wrong request.
2. Wait until the IAM pod runs with the new values. Then set `GATEWAY_DPOP__ENABLED=true` on the
   gateway. The other order also fails closed: IAM refuses the DPoP context while it is off.

Rules for clients and operators:

- **The identity must be provisioned.** A DPoP-only client cannot provision itself, because IAM's
  own API refuses the `DPoP` scheme. Link the identity (ADR-0024), or let the user log in to a
  console once with the same subject.
- **A new proof for each attempt.** Make the proof in a per-attempt hook. An SDK retry that sends
  the same headers again is refused as a replay (401 `invalid-dpop-proof`).
- **The quota.** An entry lives up to 2 × `iat_window_secs` (120 s with the defaults). With the
  defaults, one key can make about 8 proofs a second, and one user about 16. A client over its
  quota gets 429 `rate-limited` with `Retry-After`. Set `IAM_AUTHN__DPOP__PER_KEY_QUOTA` and
  `IAM_AUTHN__DPOP__PER_SUBJECT_QUOTA` through `extraEnv` to change them. (These two names and
  `IAM_AUTHN__DPOP__IAT_WINDOW_SECS` and `IAM_AUTHN__DPOP__REPLAY_CAPACITY` are not chart values.)
- **A full replay store.** When the store holds `replay_capacity` entries, IAM answers DPoP
  requests with `Unavailable` (gateway 503) until entries expire. The IAM log shows the `warn` line
  "the DPoP replay store is full" with the entry count. Raise `IAM_AUTHN__DPOP__REPLAY_CAPACITY`
  through `extraEnv`. Memory: about P bytes for each entry (measured, SMA-700 R8), so about M MiB at
  the default capacity of 200 000. The chart sets no IAM memory limit.
- **Restarts.** A restart clears the store. A proof that was used before the restart can be used
  again until it expires, up to 120 s after the restart with the defaults (R1).
- **The challenge.** With gateway DPoP on, every 401 of the gateway carries one
  `WWW-Authenticate: DPoP` line. The gateway uses no server nonce.
- **The log.** IAM logs each refused proof at `info`: "refused a DPoP proof", with the issuer and a
  defect name, rate-limited like the other refusal lines. The line never shows the proof, the
  token or the path.
```

Replace `P` and `M` with the two numbers from Task 5 Step 6.

- [ ] **Step 3: RUNBOOK § 9, the reserved names**

In the sentence that lists the names the chart sets itself (lines 600-603), change "`IAM_AUTHN__EXTRA_CA_BUNDLE_PATH` and `IAM_AUTHZ__BOOTSTRAP_ADMINS`" to "`IAM_AUTHN__EXTRA_CA_BUNDLE_PATH`, `IAM_AUTHZ__BOOTSTRAP_ADMINS`, `IAM_AUTHN__DPOP__ENABLED` and `IAM_AUTHN__DPOP__FORWARDED_BASE_URLS`". (The other `IAM_AUTHN__DPOP__*` names stay settable through `extraEnv`: they do not start with a reserved name and `__`.)

- [ ] **Step 4: The changelogs**

`rs/crates/services/paigasus-iam/CHANGELOG.md`, under `## [Unreleased]`:

```markdown

### Added

- A new table, `[authn.dpop]`, turns on the DPoP proof check of the gateway path (RFC 9449). It is
  off by default. With `enabled = true`, `Introspect` accepts a new `dpop` context with the proof,
  the method and the path, and checks the proof against `forwarded_base_urls` before the identity
  lookup. `IsAuthorized` alone accepts one follow-up with `authorization: DPoP` and `dpop`
  metadata, once, as a self-query. IAM's own HTTP and gRPC API keep refusing the DPoP scheme
  (SMA-700).
- Two new error reasons: `invalid-dpop-proof` (gRPC `Unauthenticated`) and `dpop-quota-exceeded`
  (gRPC `ResourceExhausted`, with `RetryInfo`). A full replay store answers `authn-unavailable`
  (SMA-700).
- IAM refuses a boot with DPoP on and no `forwarded_base_urls`, a URL that is not `https` or
  loopback `http`, a URL with a query, a fragment or user info, a window outside 1 to 300 seconds,
  and a quota of 0 or above the capacity (SMA-700).
- The Helm chart has two new values, `zones.iam.backend.dpop.enabled` and
  `zones.iam.backend.dpop.forwardedBaseUrls`. The default render does not change (SMA-700).
```

`rs/crates/services/paigasus-gateway/CHANGELOG.md`, under `## [Unreleased]`:

```markdown

### Added

- A new setting, `[dpop] enabled` (`GATEWAY_DPOP__ENABLED`), turns on the DPoP scheme on the
  protected routes. It is off by default. With it on, a client sends `Authorization: DPoP <token>`
  and one `DPoP` proof header. The gateway skips the API-key leg, forwards the proof, the method
  and the path to IAM, and sends the same token and proof on the self-query. Turn DPoP on in IAM
  first (SMA-700).
- With DPoP on, every 401 carries one `WWW-Authenticate: DPoP` line. A bad proof is 401
  `invalid-dpop-proof`. IAM's DPoP quota is 429 `rate-limited` with `Retry-After`, and the metric
  label `denied`, not `unavailable`. With DPoP off, no response changes (SMA-700).
```

- [ ] **Step 5: The spec R8 note**

In the spec, § 4.5, append to the memory-budget bullet: `Measured (SMA-700 Task 5): P bytes for each entry, M MiB at 200 000.` In § 9, R8 row, change the Owner cell to: `Measured: P bytes per entry, M MiB at the default (plan Task 5). The runbook states it.` Replace `P` and `M` with the numbers.

- [ ] **Step 6: ADR-0026**

ADR-0026 is in Notion, not in this repository. Do not edit Notion from this task. In the task report, tell the coordinator: "ADR-0026 must state the gateway-path scope (D1, D14, D15, D18) — spec § 4.11 says it is updated to this scope; check it before the PR."

- [ ] **Step 7: Check and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
grep -n " P bytes\| M MiB" docs/ops/RUNBOOK-chart.md docs/superpowers/specs/2026-10-04-sma-700-dpop-proof-check-design.md
git add docs/ops/RUNBOOK-chart.md rs/crates/services/paigasus-iam/CHANGELOG.md rs/crates/services/paigasus-gateway/CHANGELOG.md docs/superpowers/specs/2026-10-04-sma-700-dpop-proof-check-design.md
git commit -m "docs(rs): DPoP runbook, changelogs and the measured replay-store memory (SMA-700)

How to turn DPoP on (IAM first), the forwarded base URLs, a new proof
for each attempt, the quotas, a full store, the restart window, the
precondition that the identity is provisioned, and the measured bytes
per replay-store entry.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Expected: the `grep` prints nothing (every `P` and `M` placeholder holds a number).

---

### Task 16: The mutation battery (§ 5.3)

Each mutation must COMPILE, or it proves only that warnings are denied. Most mutations below keep every item in use (`… && false`) for that reason. Run the battery on a clean tree: commit everything first.

**Files:**
- Create: `docs/superpowers/plans/2026-10-04-sma-700-mutation-results.md`
- Temporarily modify (and restore): the files named in the table.

**Interfaces:**
- Consumes: Tasks 1-15, all committed.
- Produces: the results file.

- [ ] **Step 1: The battery command**

Save as `/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/bf1db97b-bd43-497a-8da8-5920aaa8f743/scratchpad/sma700-battery.sh` (the scratchpad of this session; any scratch path works):

```bash
#!/bin/bash
# SMA-700 § 5.3: one battery run. Prints the failing test names, or "NO TEST FAILED".
set -uo pipefail
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
out="$(PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked --no-fail-fast -p paigasus-iam -p paigasus-gateway \
  -E 'kind(lib) | binary(grpc_authn) | binary(grpc_whoami) | binary(http_authn) | binary(chat_proxy)' 2>&1)"
rc=$?
if printf '%s\n' "$out" | grep -q "error\[E[0-9]*\]\|error: could not compile"; then
  echo "DOES NOT COMPILE — fix the mutation, it proves nothing"; printf '%s\n' "$out" | grep -m5 "error"
  exit 2
fi
fails="$(printf '%s\n' "$out" | grep -E '^\s+FAIL ' | sed -E 's/.*FAIL \[[^]]*\] //' | sort -u)"
if [ -z "$fails" ]; then echo "NO TEST FAILED (rc=$rc)"; else printf '%s\n' "$fails"; fi
```

Run it once on the clean tree: `/bin/bash <script>`. Expected: `NO TEST FAILED (rc=0)`. If anything fails on the clean tree, STOP and report.

- [ ] **Step 2: Run each mutation**

For each row: apply the change with Edit, run the script, record the output in the results file, then restore with `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop checkout -- <file>` (safe here: the tree was clean before the mutation). Check `git status --short` is empty before the next row.

| # | File | Change (exact) | Expected killer (at least) |
|---|---|---|---|
| C1 | `paigasus-iam/src/adapters/oidc/dpop.rs` | in `parse`: `if proof.len() > MAX_PROOF_BYTES {` → `if proof.len() > MAX_PROOF_BYTES && false {` | `check_1_an_oversized_proof_is_malformed` |
| C2 | same | in `UniqueObject`'s visitor: `if members.contains_key(&name) {` → `if members.contains_key(&name) && false {` | `a_repeated_member_name_is_malformed` |
| C3 | same | in `check_typ`: `_ => Err(ProofDefect::Typ),` → `None => Ok(()), Some(_) => Err(ProofDefect::Typ),` | `check_3_…` ("no typ") |
| C4 | same | in `check_alg`: `_ => Err(ProofDefect::Alg),` → `_ => Ok(Algorithm::ES256),` | `check_4_only_es256_and_rs256_are_allowed` |
| C5a | same | `if PRIVATE_JWK_MEMBERS.iter().any(|member| jwk.contains_key(*member)) {` → `… && false {` | `check_5_the_jwk_must_be_a_public_key_of_the_header_alg` |
| C5b | same | `if member("kty")? != "EC" \|\| member("crv")? != "P-256" {` → `if member("kty")? != "EC" {` | `check_5_…` ("P-384 under ES256") |
| C5c | same | `if !RSA_BITS.contains(&bits) {` → `if !RSA_BITS.contains(&bits) && false {` | `check_5_rsa_bounds` |
| C6 | same | in `verify_signature`: `_ => Err(ProofDefect::Signature),` → `_ => Ok(()),` | `check_6_the_signature_must_verify_with_the_header_jwk` |
| C7 | same | in `read_claims`: `jti: read_jti(payload)?,` → `jti: read_jti(payload).unwrap_or_else(\|_\| "jti".to_owned()),` | `check_7_each_claim_must_be_present_and_of_its_type` |
| C8 | same | `if claims.ath != ath_of(token) {` → `if claims.ath != ath_of(token) && false {` | `check_8_ath_must_hash_this_token` |
| C9 | same | `if thumbprint(&key) != jkt.as_str() {` → `if thumbprint(&key) != jkt.as_str() && false {` | `check_9_the_thumbprint_must_equal_the_token_jkt` |
| C10 | `paigasus-iam/src/application/dpop.rs` | `if proof.htm != request.method {` → `if proof.htm != request.method && false {` | `check_10_htm_is_exact` |
| C11 | same | `if !self.htu_matches(&proof.htu, &request.path) {` → `… && false {` | `check_11_htu_rules`, `a_proof_for_a_base_url_that_is_not_configured_is_refused` |
| C12 | same | in `iat_window_end`: `(skew <= self.iat_window_secs)` → `(skew <= self.iat_window_secs \|\| true)` | `check_12_iat_window_edges` |
| C13 | same | `RecordOutcome::Replayed => Err(self.refuse(issuer, ProofDefect::Replayed)),` → `RecordOutcome::Replayed => Ok(()),` | `check_13_…`, `a_dpop_gateway_sequence_passes_once_and_never_again` |
| W1 | `paigasus-iam/src/application/authenticate_token.rs` | in `resolve_dpop`: `DpopInput::Introspect(request) => verifier.verify(&claims, token, request)?,` → `DpopInput::Introspect(_) => {}` | `no_identity_lookup_runs_before_the_proof_check`, `an_unprovisioned_identity_with_a_bad_proof_is_an_invalid_proof` |
| W2 | `paigasus-iam/src/adapters/grpc/authn.rs` | in `introspect`, the `Some(dpop) => { … }` arm: keep the `let context = DpopRequest { … };` line, rename it to `let _context`, and replace `self.state.authn.introspect_dpop(&request.token, context).await` with `self.state.authn.introspect(&request.token).await` | `a_dpop_gateway_sequence_passes_once_and_never_again` |
| W3 | `paigasus-iam/src/application/authenticate_token.rs` | in `resolve_dpop`: move the identity lookup first — replace the last four statements (from `let claims = …` to `self.resolve_claims(claims, provisioning).await`) with `let claims = self.authenticator.authenticate(token, TokenScheme::Dpop).await?; let principal = self.resolve_claims(claims.clone(), provisioning).await?; match &input { DpopInput::Introspect(request) => verifier.verify(&claims, token, request)?, DpopInput::FollowUp(proof) => verifier.redeem(&claims, token, proof)?, } Ok(principal)` | `no_identity_lookup_runs_before_the_proof_check`, `an_unprovisioned_identity_with_a_bad_proof_is_an_invalid_proof` |
| W4 | `paigasus-iam/src/adapters/dpop_replay.rs` | in `redeem_follow_up`: `_ => RedeemOutcome::Refused,` → `_ => RedeemOutcome::Redeemed,` | `a_follow_up_is_redeemed_once_…`, `a_follow_up_with_no_introspect_is_refused` |
| W5 | `paigasus-iam/src/application/dpop.rs` | in `redeem`: `if !self.checker.ath_matches(&follow_up.ath, token) {` → `… && false {` | `the_follow_up_checks_ath_and_the_proof_shape`, `a_follow_up_with_another_token_of_the_same_key_is_refused` |
| W6 | `paigasus-iam/src/adapters/grpc/authn.rs` | `&& req.uri().path() == IS_AUTHORIZED_PATH.as_str()` → `&& !IS_AUTHORIZED_PATH.is_empty()` | `a_follow_up_on_another_rpc_is_an_invalid_token` |
| W7 | `paigasus-iam/src/adapters/grpc/authz.rs` | `follow_up_self_query(is_follow_up, &actor, &principal)?;` → `follow_up_self_query(is_follow_up && false, &actor, &principal)?;` | `a_follow_up_for_another_principal_is_refused` |
| W8 | `paigasus-iam/src/application/dpop.rs` | in `check_request`: `} else if !path_is_well_formed(&request.path) {` → `} else if !path_is_well_formed(&request.path) && false {` | `the_forwarded_path_must_not_move_the_origin` |
| W9 | `paigasus-gateway/src/adapters/http/auth.rs` | in `iam_auth`: `iam.introspect_token(&token, Some(context))` → `iam.introspect_token(&token, Some(context).filter(\|_\| false))` | `a_dpop_request_forwards_its_context_…`, `one_dpop_request_end_to_end` |
| W10 | same | in `iam_auth`, the `Dpop` arm: add `let _ = iam.introspect_api_key(&token).await;` as its first line | `a_dpop_request_forwards_its_context_…`, `one_dpop_request_end_to_end` |
| W11 | same | in `dpop_challenge`: `(SchemeUsed::Bearer \| SchemeUsed::None, _) => CHALLENGE_BARE,` → `(SchemeUsed::Bearer \| SchemeUsed::None, _) => CHALLENGE_INVALID_TOKEN,` | `the_dpop_challenge_table`, `a_bearer_or_absent_credential_gets_the_bare_challenge_when_dpop_is_on` |
| W12 | `paigasus-iam/src/adapters/grpc/mod.rs` | in `router`: delete the `.http2_max_header_list_size(…)` line | `a_large_token_and_a_large_proof_pass_the_transport` |
| W13 | `paigasus-gateway/src/adapters/http/auth.rs` | in `resource_exhausted_error`: `if is_dpop_quota(status) {` → `if is_dpop_quota(status) && false {` | `an_iam_dpop_quota_refusal_is_429_…` |
| D7 | `paigasus-iam/src/adapters/oidc/validator.rs` | in step 7, the Bearer arm: `if let Some(marker) = sender_constraint_marker(&claims) {` → `if let Some(marker) = sender_constraint_marker(&claims) && false {` | `bearer_scheme_still_refuses_a_bound_token_and_binds_nothing`, `a_bound_token_with_no_dpop_context_is_an_invalid_token` |
| D11 | `paigasus-iam/src/application/authenticate_token.rs` | in `resolve_dpop`, the `let Some(verifier) = &self.dpop else { … };` body becomes `let claims = self.authenticator.authenticate(token, TokenScheme::Dpop).await?; return self.resolve_claims(claims, provisioning).await;` | `with_dpop_off_a_dpop_context_is_a_malformed_token`, `with_dpop_off_a_dpop_context_is_an_invalid_token` |

`keycloak_e2e` does not run per mutation (§ 5.3).

- [ ] **Step 3: A surviving mutation gets a test**

If a row prints `NO TEST FAILED`, write a test that fails under that mutation, in the owning task's style, restore the file, commit the test (`test(rs): …`), and run the WHOLE battery again from C1 (a later fix can make an earlier test inert). If a row prints `DOES NOT COMPILE`, change the mutation so it compiles (keep every item in use) and run it again; record the compiling form.

- [ ] **Step 4: Run `keycloak_e2e` once**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test keycloak_e2e
```

Expected: PASS.

- [ ] **Step 5: Write the results file and commit**

Create `docs/superpowers/plans/2026-10-04-sma-700-mutation-results.md`:

```markdown
# SMA-700 mutation results (spec § 5.3)

Battery: `cargo nextest run --locked --no-fail-fast -p paigasus-iam -p paigasus-gateway -E 'kind(lib) | binary(grpc_authn) | binary(grpc_whoami) | binary(http_authn) | binary(chat_proxy)'` with `PAIGASUS_REQUIRE_DOCKER=1`, on commit `<sha of the tree under test>`. Every mutation below compiled.

| # | Mutation | Tests that failed |
|---|---|---|
| C1 | check 1 size passes | <names, from the script output> |
```

with one row per mutation of the table (C1 … D11), the real failing test names from the script, a final line `keycloak_e2e: PASS on <sha>`, and one line for each test that Step 3 added (or `No mutation survived.`). Then:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git status --short   # only the results file (and any Step 3 test, already committed)
git add docs/superpowers/plans/2026-10-04-sma-700-mutation-results.md
git commit -m "docs(rs): SMA-700 mutation battery results

Every check and wiring mutation of spec § 5.3 compiled and failed at
least one test.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 17: Final verification — the full gate graph

**Files:** none changed, unless a gate finds a defect (then fix it in the owning task's files and commit with that task's scope).

- [ ] **Step 1: Format, lint, typecheck**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
(cd rs && cargo fmt --check && cargo clippy --locked --workspace --all-targets -- -D warnings)
moon run ts:fmt paigasus-sdk-ts:typecheck paigasus-proto-ts:typecheck gateway-console-ts:typecheck iam-console-ts:typecheck
(cd contracts && buf format --exit-code && buf breaking --against "$(git rev-parse --git-common-dir)#branch=main,subdir=contracts")
python3 ci/error-registry/check.py --self-test && python3 ci/error-registry/check.py --single-site
```

Expected: all exit 0.

- [ ] **Step 2: Codegen drift**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
moon run contracts:generate --force
git status --short
```

Expected: no change (the committed bindings equal a fresh generation).

- [ ] **Step 3: The full gate graph (root `CLAUDE.md`, `ci-targets` block)**

Run it under system bash 3.2 first (it satisfies `repo:affected-smoke`). Make a shim directory that holds only `bash` → `/bin/bash`, so Moon's `bash` is 3.2 without moving `/bin` ahead of the proto shims:

```bash
mkdir -p /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/bf1db97b-bd43-497a-8da8-5920aaa8f743/scratchpad/bash32 && ln -sf /bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/bf1db97b-bd43-497a-8da8-5920aaa8f743/scratchpad/bash32/bash
export PATH="/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/bf1db97b-bd43-497a-8da8-5920aaa8f743/scratchpad/bash32:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
git fetch origin main
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :moon-diagnosis-exec :test-e2e \
  --base origin/main --include-relations
```

Then re-run the gates that need another bash directly and read THOSE results instead of the `moon ci` verdict for them:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-700-dpop
for gate in ruff next-public publish-metadata version-lockstep nats-permissions; do /opt/homebrew/bin/bash "ci/$gate/run.sh" || echo "FAIL $gate"; done
/opt/homebrew/bin/bash ci/actionlint/run.sh
```

Expected: every gate passes. Rules for reading the results:

- `repo:actionlint` gives a local verdict only when its preflight prints a pipe capacity of at least 8192 bytes. If it exits rc 2 with a "small" pipe message, that is a host condition (SMA-612), not a finding; record it and rely on CI.
- A bash-4 gate that fails under 3.2 with `mapfile: command not found` or `declare: -A: invalid option` is a bash-version artifact: read its direct bash-5 run instead.
- `repo:deny` can red on a new advisory for a crate this branch did not touch (memory note); report it, do not waive it.
- If `moon ci` reports a failure with no clear owner, follow the root `CLAUDE.md` "Diagnosing an unattributed `moon ci` failure" procedure, Step 0 first.

- [ ] **Step 4: Record and report**

No commit unless a fix was needed. Report to the coordinator: the `moon ci` result, each direct gate result (with the actionlint preflight line), the R8 numbers from Task 5, the mutation summary from Task 16, and the ADR-0026 note from Task 15.

---

## Self-review

**Spec coverage.** § 4.1 → Task 1. § 4.2 → Task 2. § 4.3 → Task 3. § 4.4 checks 1-9 → Task 4; checks 10-13 → Task 7. § 4.5 → Task 5 (and R8, Tasks 5 and 15). § 4.6 → Task 7. § 4.7 → Task 8. § 4.8 Introspect and its request checks → Tasks 7-8; the follow-up, the extension, the self-query rule and the header limit → Task 9; the error table → Task 2; the logs → Tasks 3 and 7. § 4.9 → Task 6. § 4.10 config → Task 11; `Iam` trait → Task 12; flow, errors and challenge → Task 13; `gateway.toml.example` → Task 11. § 4.11 chart → Task 14; runbook, changelogs → Task 15; ADR-0026 → Task 15 Step 6 (Notion, reported). § 5.1 → the unit tests of Tasks 2-9, 11-13; RS256 fixtures → decision P1. § 5.2 → Task 10 (IAM), Task 13 (gateway `auth.rs`, `chat_proxy`), Task 14 (chart). § 5.3 → Task 16. § 5.4 AC 1-7 → Tasks 10, 4/7, 5/10, 8/10, 3/10, 10/13/14, 13. § 8 → Task 17.

**Type consistency.** `TokenScheme`, `Jkt`, `ProofDefect`, `ProofClaims`, `FollowUpClaims`, `ProofKey`, `NewProof`, `RecordOutcome`, `RedeemOutcome`, `DpopProofChecker` (`check`, `follow_up_claims`, `ath_matches`), `ReplayStore` (`record`, `redeem_follow_up`), `DpopRequest`, `DpopProofVerifier` (`new`, `check_request`, `verify`, `redeem`), `DpopInput`, `resolve_dpop`, `introspect_dpop`, `with_dpop`, `dpop_enabled`, `InMemoryReplayStore::new`, `JoseDpopProofChecker`, `DpopConfig`, `IS_AUTHORIZED_PATH`, `DpopFollowUp`, `grpc_max_header_list_size`, `GatewayDpopConfig`, `AuthState`, `Credentials`, `ProofHeader`, `credentials`, `CallerCredential`, `DpopContext`, `SchemeUsed`, `dpop_challenge`, `GatewayError::InvalidDpopProof`, `GatewayError::status` — each is defined once (Tasks 2, 4-9, 11-13) and used with the same name and signature later.

**Review Focus.** The five lines each have a test in their owning task (Tasks 4, 6, 7, 13, 14).

**Placeholders.** Two values are measurements, not design gaps: `P` and `M` (Task 5 Step 6 produces them, Task 15 writes them, and its Step 7 `grep` proves none is left). The results file of Task 16 lists real test names from the script output.

