# SMA-666 WhoAmI Tidy-up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the five items that SMA-632 left: rename `createIntrospectPrincipalResolver`, pin the OIDC arm of both WhoAmI mappers, assert the before-state of the bootstrap-admin test, remove `IamClients.serviceInfo`, and pin plus record the fact that an API-key bearer gets its grants through `WhoAmI`. No runtime behaviour changes.

**Architecture:** Six commits, in the order of spec § 2.1 (commits 2-7). Two TypeScript commits change names, comments and one test. Three Rust commits add or strengthen tests only. One docs commit adds three `**Clarified (SMA-666, 2026-09-23).**` blockquotes to two merged specs. Each test-strengthening change gets a mutation that must red the named test, then the mutation is removed with the Edit tool.

**Tech Stack:** Rust (edition 2024, cargo-nextest, tokio tests), TypeScript (vitest 5, Prettier, ESLint), Moon 2.5.3 through proto shims, Docker for the `paigasus-iam` integration suites and the TS e2e tiers.

**Spec:** docs/superpowers/specs/2026-09-23-sma-666-whoami-tidy-design.md

## Global Constraints

- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy`, branch `feature/sma-666-whoami-tidy`. Use absolute paths. A subagent starts in the MAIN checkout: it must `cd` to the worktree and run `git branch --show-current` first.
- Every command block starts with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Every new source file opens with an SPDX header. This plan adds no new file except this plan.
- Rust crates stay on edition 2024 and rust-version 1.95. `rs/Cargo.toml` has `warnings = "deny"`, so a mutation that leaves an unused binding does not compile and proves nothing.
- Commit subjects, copied from spec § 2.1 (header max 100 chars, scope list at `ts/packages/commitlint-config/index.cjs:42`):
  - Task 1: `refactor(ts): name the principal resolver for what it returns (SMA-666)`
  - Task 2: `test(rs): pin the OIDC arm of both WhoAmI mappers (SMA-666)`
  - Task 3: `test(rs): assert the bootstrap identity is absent before WhoAmI (SMA-666)`
  - Task 4: `refactor(ts): remove the unused IamClients.serviceInfo client (SMA-666)`
  - Task 5: `test(rs): pin that context_for reads an API key's grants (SMA-666)`
  - Task 6: `docs(repo): clarify the SMA-632 and SMA-633 specs on API-key grants (SMA-666)`
- Every commit message ends with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`, after one blank line.
- No body line starts with `#` followed by a number. No body line has the form `Token: value` (a word, a colon, a space). Only the trailer has that form. Such lines fail `footer-leading-blank`.
- Never use `--no-verify`. If `commitlint` is not found, provision the worktree (`proto install`, `pnpm -C ts install`) and commit again.
- Never use `git checkout --` or `git restore` to remove a mutation. It also removes the uncommitted test change. Remove each mutation with the Edit tool.
- Add, do not amend. Each task makes one new commit. Never `git commit --amend`.
- If the sandbox refuses the HEREDOC commit form, write the message to a file in the session scratchpad and run `git commit -F <that file>`.
- Docker-gated Rust tests run with `PAIGASUS_REQUIRE_DOCKER=1`, so that a missing daemon is a failure and not a quiet skip.
- Write every new comment, doc comment, blockquote and commit message in ASD-STE100 Simplified Technical English: short sentences, active voice, approved words. Keep technical names as they are.
- Do not write the base name of moon's CI report JSON file (the `.moon/cache/` report that `moon ci` writes) in any file. `repo:actionlint` check 12 reds a tracked file that holds that token without a `moon-diagnosis:ok` marker. This plan spells it only as the pattern `ci[R]eport`.
- Do not edit the specs and plans under `docs/superpowers/` except the two spec files in Task 6 (spec D3).
- Before Task 1, the controller commits this plan file with `docs(repo): plan for the WhoAmI tidy-up (SMA-666)`. That commit is not one of the § 2.1 commits.

## Review Focus

1. **A transport that stops calling `context_for` for an API-key bearer.** The Task 5 unit test pins `context_for` only. `who_am_i_serves_an_api_key_bearer` (`tests/grpc_whoami.rs:250`) gives the service account no grant, so it cannot assert a non-empty `role_grants`. I checked: a transport test needs a role grant on a service account in a Docker suite, which spec § 7.1 does not ask for. This stays a residual. It is safe today because both handlers call `context_for` for every `AuthContext` (`adapters/grpc/authn.rs:105`, `adapters/http/authn.rs:119`), and the note in Task 6 names both lines.
2. **The key-set check in the round-trip test must be the assertion that fails.** The spec's mutation removes only the `listDeadLetters` call. Then the ordered `toEqual` list fails first, and the key-set check is never proved. Task 4 Step 6 removes the call AND its expected row, so only the key-set check can fail.
3. **The key-set expression must type-check.** The spec's form `sent.map(([method]) => method.split('.')[0])` reads `string | null` from `sent` (`(string | null)[][]`), and `strict` rejects `.split` on it. Task 4 reads `fake.calls` (`method: string`) instead.
4. **An exact expiry in the convert fixture.** `ts()` keeps nanoseconds, so `Some(ts(expiry))` is exact only when the test and the fixture use one `DateTime` value. Task 2 passes one `expiry` value into the fixture and into the assertion.
5. **The literal PRN in the Task 5 assertion.** The test asserts a literal `scope_prn`. `Prn::canonical()` (`rs/crates/libs/paigasus-kernel/src/resource_name.rs:163-166`) re-renders a lowercase hyphenated UUID with the same fields, so the parsed literal and its canonical form are equal. I checked this; no extra test is needed.

---

## Task 1: Rename `createIntrospectPrincipalResolver` (spec commit 2)

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/principal-resolver.ts:8-10` (comment) and `:36` (function name)
- Modify: `ts/packages/paigasus-console-core/src/index.ts:16`
- Modify: `ts/apps/iam-console/lib/auth.ts:9,30`
- Modify: `ts/apps/gateway-console/lib/auth.ts:9,30`
- Modify: `ts/packages/paigasus-console-core/tests/integration/principal.test.ts:13` (import) and the eight call sites `:38, 59, 78, 85, 100, 109, 117, 143`
- Modify: `ts/packages/paigasus-auth/src/adapters/claims-resolver.ts:6-10`
- Modify: `ts/packages/paigasus-auth/src/ports/principal-resolver.ts:8`
- Do NOT modify: `principal.test.ts:169` (`'names the same principal Introspect names for the same token'`).

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: the export `createPrincipalResolver` from `@paigasus/console-core` (same signature as before). No alias for the old name (spec D2).

- [ ] **Step 1: Rename the function.** In `ts/packages/paigasus-console-core/src/principal-resolver.ts:36`:

Before:
```ts
export function createIntrospectPrincipalResolver(deps: {
```
After:
```ts
export function createPrincipalResolver(deps: {
```

- [ ] **Step 2: Rewrite the comment at `principal-resolver.ts:8-10`.**

Before:
```ts
// It lives in the APP because @paigasus/auth must not import @paigasus/sdk
// (ts/packages/paigasus-auth/src/ports/principal-resolver.ts:3-8), and the sdk boundary rule bans
// every @paigasus/* import except proto. SMA-631 records a shared home for SMA-512.
```
After:
```ts
// It lives in @paigasus/console-core, not in @paigasus/auth, because @paigasus/auth must not import
// @paigasus/sdk (ts/packages/paigasus-auth/src/ports/principal-resolver.ts:3-8), and the sdk
// boundary rule bans every @paigasus/* import except proto. So neither of those two packages can
// hold a resolver that needs both.
```

- [ ] **Step 3: Rename the export.** In `ts/packages/paigasus-console-core/src/index.ts:16`:

Before:
```ts
export { createIntrospectPrincipalResolver } from './principal-resolver';
```
After:
```ts
export { createPrincipalResolver } from './principal-resolver';
```

- [ ] **Step 4: Rename the import and the call in both apps.** In `ts/apps/iam-console/lib/auth.ts` AND `ts/apps/gateway-console/lib/auth.ts` (the two files are identical at these lines):

Line 9, before:
```ts
import { createIntrospectPrincipalResolver, logger, requestCorrelationId } from '@paigasus/console-core';
```
Line 9, after:
```ts
import { createPrincipalResolver, logger, requestCorrelationId } from '@paigasus/console-core';
```
Line 30, before:
```ts
    resolver: createIntrospectPrincipalResolver({ clientsForToken: async (token) => iamClientsForToken(token, await requestCorrelationId()), logger }),
```
Line 30, after:
```ts
    resolver: createPrincipalResolver({ clientsForToken: async (token) => iamClientsForToken(token, await requestCorrelationId()), logger }),
```

- [ ] **Step 5: Rename in the integration test.** In `ts/packages/paigasus-console-core/tests/integration/principal.test.ts`, use Edit with `replace_all: true`: old `createIntrospectPrincipalResolver`, new `createPrincipalResolver`. This changes nine lines: the import at `:13` and the calls at `:38, 59, 78, 85, 100, 109, 117, 143`. Line 169 does not contain the identifier and does not change.

- [ ] **Step 6: Rewrite the comment at `ts/packages/paigasus-auth/src/adapters/claims-resolver.ts:6-10`.**

Before:
```ts
// IAM's Introspect is what mints a principal PRN and returns memberships and role grants, and it
// is not reachable from TypeScript yet: contracts/buf.gen.yaml runs only bufbuild/es for TS
// (descriptors, no client), @paigasus/proto exports only common/v1, and @paigasus/sdk is a stub.
// SMA-508 lands the transport; IntrospectPrincipalResolver then slots in behind this same port
// with no change to the session shape or the store.
```
After:
```ts
// It uses the ID-token claims only, so it is the resolver for a host that has no IAM transport. A
// host with IAM (the consoles) supplies an IAM-backed resolver behind this same port, with no
// change to the session shape or the store.
```

- [ ] **Step 7: Rewrite `ts/packages/paigasus-auth/src/ports/principal-resolver.ts:8`.** Keep it on ONE line, so that the pointer `ports/principal-resolver.ts:3-8` in `console-core/src/principal-resolver.ts:9` stays correct.

Before:
```ts
// When IntrospectPrincipalResolver lands in SMA-508, the ADAPTER maps proto to these types.
```
After:
```ts
// An IAM-backed ADAPTER maps proto to these types.
```

- [ ] **Step 8: Prove that no old name remains.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
git grep -n 'IntrospectPrincipalResolver' -- ':!docs/superpowers'
```
Expected: no output, exit status 1. The pattern also matches `createIntrospectPrincipalResolver`.

```bash
sed -n 3,8p /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy/ts/packages/paigasus-auth/src/ports/principal-resolver.ts
```
Expected: six lines, the last one is `// An IAM-backed ADAPTER maps proto to these types.`

- [ ] **Step 9: Run the selected unit and type targets.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
moon run paigasus-console-core-ts:typecheck paigasus-console-core-ts:test iam-console-ts:typecheck iam-console-ts:test gateway-console-ts:typecheck gateway-console-ts:test paigasus-auth-ts:typecheck paigasus-auth-ts:test
moon run ts:fmt ts:lint
```
Expected: every target passes. If `ts:fmt` fails, run `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy/ts exec prettier --write <the named files>` and run `ts:fmt` again.

- [ ] **Step 10: Run the Docker tiers that these files select (spec § 8).** Docker must run.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
moon run paigasus-auth-ts:test-e2e paigasus-console-core-ts:test-e2e iam-console-ts:test-e2e gateway-console-ts:test-e2e
```
Expected: all pass. If one fails, follow the flake procedure in Task 7 Step 5 before you change code. If Docker is not available, write that in the task report. Task 7 runs these tiers again on the head.

- [ ] **Step 11: Commit.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
git add ts/packages/paigasus-console-core/src/principal-resolver.ts ts/packages/paigasus-console-core/src/index.ts ts/apps/iam-console/lib/auth.ts ts/apps/gateway-console/lib/auth.ts ts/packages/paigasus-console-core/tests/integration/principal.test.ts ts/packages/paigasus-auth/src/adapters/claims-resolver.ts ts/packages/paigasus-auth/src/ports/principal-resolver.ts
git commit -F - <<'EOF'
refactor(ts): name the principal resolver for what it returns (SMA-666)

The resolver makes one WhoAmI call, but its name still said Introspect.
The new name, createPrincipalResolver, names the type that it returns.
A name that does not name an RPC does not go stale when the RPC changes.
There is no alias, because console-core is private and all consumers
are in this repo.

Two comments in @paigasus/auth named the old resolver and a finished
issue. They now describe a role. One comment in console-core said that
the resolver lives in the app. It now gives the real location.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
git log --oneline -1
```
Expected: the new commit is on top of the branch.

---

## Task 2: Pin the OIDC arm of both WhoAmI mappers (spec commit 3)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:1534-1553` (fixture `oidc_context`), `:1575-1582` (test `to_who_am_i_response_reports_an_oidc_caller_in_full`), `:1590` (second caller)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/dto.rs:741` (test-module import) and add a fixture plus a test after `:915` (before the module's closing `}` at `:916`)
- Mutation sites (temporary, removed in this task): `convert.rs:430`, `convert.rs:442`, `dto.rs:304`, `dto.rs:301`

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: `convert.rs` test fixture `oidc_context(expires_at: DateTime<Utc>) -> PrincipalContext`; `dto.rs` test fixture `oidc_context(expires_at: DateTime<Utc>) -> PrincipalContext`; new test `adapters::http::dto::tests::who_am_i_response_dto_reports_an_oidc_caller_in_full`.

- [ ] **Step 1: Give the `convert.rs` fixture an `expires_at` parameter.** In `convert.rs:1534-1553`:

Before:
```rust
    /// `PrincipalContext` fixture for an OIDC-authenticated caller — `Issuer::parse` mirrors
    /// `application::authenticate_token`'s own test fixtures.
    fn oidc_context() -> PrincipalContext {
        use paigasus_iam_core::{AuthnPrincipal, Issuer, PrincipalKind, PrincipalStatus};

        PrincipalContext {
            principal: AuthnPrincipal {
                principal_id: principal(51),
                kind: PrincipalKind::User,
                status: PrincipalStatus::Active,
                credential: Credential::Oidc {
                    issuer: Issuer::parse("https://idp.example.com").unwrap(),
                    subject: "subject-51".to_string(),
                    expires_at: Utc::now() + chrono::Duration::hours(1),
                },
            },
            memberships: Vec::new(),
            role_grants: Vec::new(),
        }
    }
```
After:
```rust
    /// `PrincipalContext` fixture for an OIDC-authenticated caller — `Issuer::parse` mirrors
    /// `application::authenticate_token`'s own test fixtures. `expires_at` is a parameter, as in
    /// `api_key_context`, so that a test can assert the exact expiry.
    fn oidc_context(expires_at: DateTime<Utc>) -> PrincipalContext {
        use paigasus_iam_core::{AuthnPrincipal, Issuer, PrincipalKind, PrincipalStatus};

        PrincipalContext {
            principal: AuthnPrincipal {
                principal_id: principal(51),
                kind: PrincipalKind::User,
                status: PrincipalStatus::Active,
                credential: Credential::Oidc {
                    issuer: Issuer::parse("https://idp.example.com").unwrap(),
                    subject: "subject-51".to_string(),
                    expires_at,
                },
            },
            memberships: Vec::new(),
            role_grants: Vec::new(),
        }
    }
```

- [ ] **Step 2: Make the `convert.rs` OIDC test assert literal values.** Replace `convert.rs:1575-1582`:

Before:
```rust
    #[test]
    fn to_who_am_i_response_reports_an_oidc_caller_in_full() {
        let ctx = oidc_context();
        let response = to_who_am_i_response(&ctx);
        assert!(!response.issuer.is_empty());
        assert!(!response.subject.is_empty());
        assert!(response.expires_at.is_some());
    }
```
After:
```rust
    /// SMA-666: literal values, not "not empty". Both fixture values are not empty, so the old
    /// assertions passed with the issuer and the subject transposed. The exact expiry catches a
    /// fabricated timestamp.
    #[test]
    fn to_who_am_i_response_reports_an_oidc_caller_in_full() {
        let expiry = Utc::now() + chrono::Duration::hours(1);
        let ctx = oidc_context(expiry);
        let response = to_who_am_i_response(&ctx);
        assert_eq!(response.issuer, "https://idp.example.com");
        assert_eq!(response.subject, "subject-51");
        assert_eq!(response.expires_at, Some(ts(expiry)));
    }
```

- [ ] **Step 3: Update the second caller.** In `to_who_am_i_response_reports_the_callers_role_grants` (was `:1590`, now two lines lower):

Before:
```rust
        let mut ctx = oidc_context();
```
After:
```rust
        let mut ctx = oidc_context(Utc::now() + chrono::Duration::hours(1));
```

- [ ] **Step 4: Import `Issuer` in the `dto.rs` test module.** In `dto.rs:741`:

Before:
```rust
    use paigasus_iam_core::{ApiKeyId, AuthnPrincipal, PrincipalKind, PrincipalStatus, ProjectId, Slug, TeamId};
```
After:
```rust
    use paigasus_iam_core::{ApiKeyId, AuthnPrincipal, Issuer, PrincipalKind, PrincipalStatus, ProjectId, Slug, TeamId};
```

- [ ] **Step 5: Add the `dto.rs` fixture and test.** Insert after `dto.rs:915` (the `    }` that closes `who_am_i_response_dto_reports_the_callers_role_grants`), before the module's closing `}`:

```rust

    /// `PrincipalContext` fixture for an OIDC-authenticated caller. It uses the same literals as
    /// `grpc::convert`'s own `oidc_context` test fixture. `expires_at` is a parameter so that a
    /// test can assert the exact expiry.
    fn oidc_context(expires_at: DateTime<Utc>) -> PrincipalContext {
        PrincipalContext {
            principal: AuthnPrincipal {
                principal_id: principal(51),
                kind: PrincipalKind::User,
                status: PrincipalStatus::Active,
                credential: Credential::Oidc {
                    issuer: Issuer::parse("https://idp.example.com").unwrap(),
                    subject: "subject-51".to_string(),
                    expires_at,
                },
            },
            memberships: Vec::new(),
            role_grants: Vec::new(),
        }
    }

    /// SMA-666: before this test, no test covered the `Credential::Oidc` arm of
    /// `WhoAmIResponseDto::from`. Literal values catch an issuer/subject transposition, which a
    /// "not empty" check does not. The exact expiry catches a fabricated timestamp.
    #[test]
    fn who_am_i_response_dto_reports_an_oidc_caller_in_full() {
        let expiry = Utc::now() + chrono::Duration::hours(1);
        let dto = WhoAmIResponseDto::from(oidc_context(expiry));
        assert_eq!(dto.issuer, "https://idp.example.com");
        assert_eq!(dto.subject, "subject-51");
        assert_eq!(dto.expires_at, Some(expiry));
    }
```

- [ ] **Step 6: Run the unit tests green.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy/rs
cargo nextest run --locked -p paigasus-iam --lib who_am_i
```
Expected: PASS. The run includes `adapters::grpc::convert::tests::to_who_am_i_response_reports_an_oidc_caller_in_full` and `adapters::http::dto::tests::who_am_i_response_dto_reports_an_oidc_caller_in_full`.

- [ ] **Step 7: Mutation A (convert.rs, transposition).** Edit `convert.rs:430`:

Before:
```rust
        Credential::Oidc { issuer, subject, .. } => (issuer.as_str().to_string(), subject.clone()),
```
Mutation:
```rust
        Credential::Oidc { issuer, subject, .. } => (subject.clone(), issuer.as_str().to_string()),
```
Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy/rs
cargo nextest run --locked -p paigasus-iam --lib --no-fail-fast who_am_i
```
Expected: FAIL. The failing tests include `adapters::grpc::convert::tests::to_who_am_i_response_reports_an_oidc_caller_in_full` (the issuer assertion). Record the failing test names in a scratchpad file for the PR description. Remove the mutation with Edit (restore the "Before" line). Do not use `git checkout --`.

- [ ] **Step 8: Mutation B (convert.rs, expiry).** Edit `convert.rs:442`:

Before:
```rust
        expires_at: ctx.principal.expires_at().map(ts),
```
Mutation:
```rust
        expires_at: Some(ts(Utc::now())),
```
Run the Step 7 command. Expected: FAIL, including `adapters::grpc::convert::tests::to_who_am_i_response_reports_an_oidc_caller_in_full` (the expiry assertion). The two API-key expiry tests also fail; that is correct. Record the names. Remove the mutation with Edit.

- [ ] **Step 9: Mutation C (dto.rs, transposition).** Edit `dto.rs:304`:

Before:
```rust
            Credential::Oidc { issuer, subject, .. } => (issuer.as_str().to_string(), subject),
```
Mutation:
```rust
            Credential::Oidc { issuer, subject, .. } => (subject, issuer.as_str().to_string()),
```
Run the Step 7 command. Expected: FAIL, including `adapters::http::dto::tests::who_am_i_response_dto_reports_an_oidc_caller_in_full`. Record the names. Remove the mutation with Edit.

- [ ] **Step 10: Mutation D (dto.rs, expiry).** Edit `dto.rs:301`. This form leaves no unused binding, so it compiles under `warnings = "deny"`.

Before:
```rust
        let expires_at = ctx.principal.expires_at();
```
Mutation:
```rust
        let expires_at = Some(Utc::now());
```
Run the Step 7 command. Expected: FAIL, including `adapters::http::dto::tests::who_am_i_response_dto_reports_an_oidc_caller_in_full`. The API-key expiry tests in `dto.rs` also fail; that is correct. Record the names. Remove the mutation with Edit.

- [ ] **Step 11: Prove that all four mutations are gone, and run green again.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
git diff -U0 -- rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs rs/crates/services/paigasus-iam/src/adapters/http/dto.rs | grep -e '^+++' -e '^@@'
cd rs
cargo nextest run --locked -p paigasus-iam --lib
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```
Expected: every hunk header for `convert.rs` starts at line 1534 or later (the test module starts at `:607`, the mapper is at `:428-446`), and every hunk header for `dto.rs` starts at line 741 or later (the mapper is at `:299-319`). So no mutation remains in a mapper. All unit tests pass. `cargo fmt --check` passes; if it fails, run `cargo fmt` and check again. Clippy passes. `--all-targets` is required, because the changes are in `#[cfg(test)]` modules (`.moon/tasks/rust.yml:79`).

- [ ] **Step 12: Commit.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
git add rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs rs/crates/services/paigasus-iam/src/adapters/http/dto.rs
git commit -F - <<'EOF'
test(rs): pin the OIDC arm of both WhoAmI mappers (SMA-666)

The gRPC test asserted only that the issuer and the subject are not
empty, so a transposition passed. It now asserts the literal values and
the exact expiry. The HTTP DTO had no test for the OIDC arm. It now has
one, with the same literals.

Four mutations prove the tests. A transposition and a fabricated expiry
in each mapper each fail the new assertions.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
git log --oneline -1
```

---

## Task 3: Assert the bootstrap identity is absent before WhoAmI (spec commit 4)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs:31` (import)
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs:232` (insert after it, inside `who_am_i_seeds_the_bootstrap_admin`, which starts at `:218`)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: a stronger `who_am_i_seeds_the_bootstrap_admin`. No new API.

- [ ] **Step 1: Import `AuthnError`.** In `grpc_whoami.rs:31`:

Before:
```rust
use paigasus_iam_core::GrantScope;
```
After:
```rust
use paigasus_iam_core::{AuthnError, GrantScope};
```

- [ ] **Step 2: Add the before-state assertion.** Insert after `grpc_whoami.rs:232` (`    let mut client = AuthnServiceClient::new(ch);`) and before the blank line that precedes `let who = client.who_am_i(...)`:

```rust

    // Before-state (SMA-666). The principal does not exist yet, so a query by principal ID is not
    // possible. A grant needs a principal, so "not provisioned" proves "no grant" before the call.
    // `Provisioning::Disabled` writes nothing. It only fills the JWKS cache.
    let before = state.authn.resolve(&token, Provisioning::Disabled).await;
    assert!(
        matches!(before, Err(AuthnError::IdentityNotProvisioned)),
        "the bootstrap identity must not exist before the WhoAmI call, or its grant proves nothing: {before:?}",
    );
```
`AuthnError` has no `PartialEq` (`rs/crates/libs/paigasus-iam-core/src/authn.rs:183`), so use `matches!`, not `assert_eq!`. `Result<AuthnPrincipal, AuthnError>` is `Debug`, so `{before:?}` compiles.

- [ ] **Step 3: Run the test green.** Docker must run.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test grpc_whoami
```
Expected: PASS for all tests in `grpc_whoami`, including `who_am_i_seeds_the_bootstrap_admin`. If a test fails, run it alone (`... --test grpc_whoami who_am_i_seeds_the_bootstrap_admin`) before you blame this change. The Docker suites in this crate are flaky under parallel load.

- [ ] **Step 4: Mutation (pre-provision the identity).** Insert this temporary line directly before `let before = ...`:

```rust
    state.authn.resolve(&token, Provisioning::Enabled).await.unwrap(); // SMA-666 MUTATION, remove
```
Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test grpc_whoami --no-fail-fast who_am_i_seeds_the_bootstrap_admin
```
Expected: FAIL in `who_am_i_seeds_the_bootstrap_admin`, with the message `the bootstrap identity must not exist before the WhoAmI call, or its grant proves nothing: Ok(AuthnPrincipal { ... })`. The nextest retry budget re-runs it; every attempt fails. Record the failing test name. Remove the marked line with Edit.

- [ ] **Step 5: Verify.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
grep -n 'SMA-666 MUTATION' rs/crates/services/paigasus-iam/tests/grpc_whoami.rs
cd rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test grpc_whoami
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```
Expected: the grep prints nothing (exit 1). The suite passes. fmt and clippy pass.

- [ ] **Step 6: Commit.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
git add rs/crates/services/paigasus-iam/tests/grpc_whoami.rs
git commit -F - <<'EOF'
test(rs): assert the bootstrap identity is absent before WhoAmI (SMA-666)

who_am_i_seeds_the_bootstrap_admin asserted the grant after the call and
nothing before it. A grant from an earlier step would also pass. The
test now asserts that the identity is not provisioned before the call.
A grant needs a principal, so this also proves that no grant exists.

A mutation that provisions the identity first fails the new assertion.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
git log --oneline -1
```

---

## Task 4: Remove `IamClients.serviceInfo` (spec commit 5)

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/iam-clients.ts:16` (import), `:24` (type field), `:51` (doc comment), `:61` (construction)
- Modify: `ts/packages/paigasus-console-core/tests/integration/error-info-round-trip.test.ts:48-66`
- Modify: `ts/apps/iam-console/tests/unit/action-session.test.ts:102, 109, 110`
- Modify: `ts/packages/paigasus-console-core/testing/fake-iam.ts:3-6` (header, same line count), `:89` (one line), `:138-144` (doc comment of `setServiceInfo`, becomes `:138-147`)
- Modify: `ts/packages/paigasus-console-core/testing/dev-world.ts:192`
- Do NOT modify: `FakeIam.setServiceInfo`, `SERVICES.serviceInfo` (`fake-iam.ts:71`), the `serviceInfo.getServiceInfo` default (`:287-288`), the `setServiceInfo` body (`:378-381`), the fake's HTTP route, the MSW `serviceInfoHandlers` helpers, `tests/unit/dev-world.test.ts`, the e2e worlds and harnesses.

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: `IamClients` with six keys: `tenancy`, `authn`, `authz`, `audit`, `serviceAccounts`, `outbox`.

- [ ] **Step 1: Strengthen the round-trip test first.** Replace `error-info-round-trip.test.ts:48-66` (the whole `it('sends the correlation header from every one of the five clients', ...)` block):

Before:
```ts
  it('sends the correlation header from every one of the five clients', async () => {
    const clients = createIamClients({ baseUrl: fake.grpcUrl, token: 'token-a', correlationId: REQUEST_ID });
    // Some calls fail (the beforeEach handler denies getOrganization, and the fake has no default
    // for listAuditEntries). That does not matter here: the fake records each call as it ARRIVED,
    // before it runs a handler, so the header of every call is in fake.calls.
    await callIam(() => clients.tenancy.getOrganization({ prn: ORG }));
    await callIam(() => clients.serviceInfo.getServiceInfo({}));
    await callIam(() => clients.authn.introspect({ token: 'token-a' }));
    await callIam(() => clients.authz.isAuthorized({ principalPrn: fake.principalPrnFor('token-a'), action: 'ListOrganizations', resourcePrn: ORG }));
    await callIam(() => clients.audit.listAuditEntries({}));
    const sent = fake.calls.map((call) => [call.method, call.correlationId]);
    expect(sent).toEqual([
      ['tenancy.getOrganization', REQUEST_ID],
      ['serviceInfo.getServiceInfo', REQUEST_ID],
      ['authn.introspect', REQUEST_ID],
      ['authz.isAuthorized', REQUEST_ID],
      ['audit.listAuditEntries', REQUEST_ID],
    ]);
  });
```
After:
```ts
  it('sends the correlation header from every client', async () => {
    const clients = createIamClients({ baseUrl: fake.grpcUrl, token: 'token-a', correlationId: REQUEST_ID });
    // Some calls fail. The beforeEach handler denies getOrganization, and the fake has no default
    // answer for listAuditEntries, listServiceAccounts or listDeadLetters, so each of those three
    // answers Unimplemented. That does not matter here: the fake records each call as it ARRIVED,
    // before it runs a handler (also when `defaults()` throws), so the header of every call is in
    // fake.calls. The last assertion compares the called clients with the keys of IamClients, so a
    // new client fails this test until the test calls it (SMA-666).
    await callIam(() => clients.tenancy.getOrganization({ prn: ORG }));
    await callIam(() => clients.authn.introspect({ token: 'token-a' }));
    await callIam(() => clients.authz.isAuthorized({ principalPrn: fake.principalPrnFor('token-a'), action: 'ListOrganizations', resourcePrn: ORG }));
    await callIam(() => clients.audit.listAuditEntries({}));
    await callIam(() => clients.serviceAccounts.listServiceAccounts({}));
    await callIam(() => clients.outbox.listDeadLetters({}));
    const sent = fake.calls.map((call) => [call.method, call.correlationId]);
    expect(sent).toEqual([
      ['tenancy.getOrganization', REQUEST_ID],
      ['authn.introspect', REQUEST_ID],
      ['authz.isAuthorized', REQUEST_ID],
      ['audit.listAuditEntries', REQUEST_ID],
      ['serviceAccounts.listServiceAccounts', REQUEST_ID],
      ['outbox.listDeadLetters', REQUEST_ID],
    ]);
    expect(new Set(fake.calls.map((call) => call.method.split('.')[0]))).toEqual(new Set(Object.keys(clients)));
  });
```
The key-set line reads `fake.calls` (`method: string`), not `sent` (`(string | null)[][]`). The spec's `sent.map(([method]) => ...)` form does not type-check under `strict`.

- [ ] **Step 2: Run the round-trip test and see it RED before the production change.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
pnpm -C ts/packages/paigasus-console-core exec vitest run tests/integration/error-info-round-trip.test.ts
```
Expected: FAIL in `the ErrorInfo round trip > sends the correlation header from every client`. The key-set check fails: the expected set holds `serviceInfo`, and the called set does not. This is the RED for Step 3. (If vitest reports a missing generated file, run `moon run contracts:generate` first.)

- [ ] **Step 3: Remove the client from production code.** In `ts/packages/paigasus-console-core/src/iam-clients.ts`:

Line 16, before:
```ts
import { AuditService, AuthnService, AuthorizationService, createIamClient, OutboxService, ServiceAccountService, ServiceInfoService, TenancyService } from '@paigasus/sdk/iam';
```
Line 16, after:
```ts
import { AuditService, AuthnService, AuthorizationService, createIamClient, OutboxService, ServiceAccountService, TenancyService } from '@paigasus/sdk/iam';
```
Delete line 24:
```ts
  serviceInfo: Client<typeof ServiceInfoService>;
```
Line 51, before:
```ts
/** The seven clients for one bearer token, over one base URL. Pure: tests call it with a fake IAM. */
```
Line 51, after:
```ts
/** The six clients for one bearer token, over one base URL. Pure: tests call it with a fake IAM. */
```
Delete line 61:
```ts
    serviceInfo: withCorrelation(createIamClient(ServiceInfoService, transport, auth), id),
```

- [ ] **Step 4: Run the round-trip test GREEN.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
pnpm -C ts/packages/paigasus-console-core exec vitest run tests/integration/error-info-round-trip.test.ts
```
Expected: PASS, three tests.

- [ ] **Step 5: Update the key list in `ts/apps/iam-console/tests/unit/action-session.test.ts`.**

Line 102, before:
```ts
  it('returns the seven clients when the session resolves', async () => {
```
Line 102, after:
```ts
  it('returns the six clients when the session resolves', async () => {
```
Lines 109-110, before:
```ts
    // SMA-636 added serviceAccounts to IamClients, and SMA-629 added outbox.
    if (result.ok) expect(Object.keys(result.value).sort()).toEqual(['audit', 'authn', 'authz', 'outbox', 'serviceAccounts', 'serviceInfo', 'tenancy']);
```
Lines 109-110, after:
```ts
    // SMA-636 added serviceAccounts to IamClients, SMA-629 added outbox, and SMA-666 removed serviceInfo.
    if (result.ok) expect(Object.keys(result.value).sort()).toEqual(['audit', 'authn', 'authz', 'outbox', 'serviceAccounts', 'tenancy']);
```

- [ ] **Step 6: Mutation (prove the key-set check alone).** In `error-info-round-trip.test.ts`, delete BOTH of these lines with Edit (the call and its expected row). If you delete only the call, the ordered `toEqual` list fails first and the key-set check proves nothing.

```ts
    await callIam(() => clients.outbox.listDeadLetters({}));
```
```ts
      ['outbox.listDeadLetters', REQUEST_ID],
```
Run the Step 4 command. Expected: FAIL in `the ErrorInfo round trip > sends the correlation header from every client`, on the `new Set(...)` assertion (the expected set holds `outbox`, the called set does not). The `toEqual(sent)` assertion passes. Record the failing test name. Put both lines back with Edit, at the same positions. Run the Step 4 command again. Expected: PASS.

- [ ] **Step 7: Rewrite the header of `testing/fake-iam.ts:3-6`.** Keep four lines, so that the line numbers below stay valid.

Before:
```ts
// An in-process fake of IAM for the integration and e2e tiers (spec § 9.1): a real gRPC server over
// h2c for the seven services the console calls (SMA-636 added ServiceAccountService, SMA-629
// OutboxService), and a plain HTTP server for `GET /v1/service-info`, which @paigasus/discovery
// probes.
```
After:
```ts
// An in-process fake of IAM for the integration and e2e tiers (spec § 9.1): a real gRPC server over
// h2c for seven services, and a plain HTTP server for `GET /v1/service-info`, which @paigasus/discovery
// probes. The console calls six of the services (SMA-636 added ServiceAccountService, SMA-629
// OutboxService). It has no client for ServiceInfoService (SMA-666); see `setServiceInfo` below.
```

- [ ] **Step 8: Rewrite `testing/fake-iam.ts:89`.** One line.

Before:
```ts
/** Every RPC the fake serves, as `<client key>.<method localName>` — the same keys as `IamClients`. */
```
After:
```ts
/** Every RPC the fake serves, as `<client key>.<method localName>` — the keys of `IamClients`, plus `serviceInfo`. */
```

- [ ] **Step 9: Rewrite the `setServiceInfo` doc comment at `testing/fake-iam.ts:138-144`.** The new comment takes lines 138-147.

Before:
```ts
  /**
   * A descriptor changes both the gRPC and the HTTP answer. `{ status }` changes ONLY the HTTP
   * route (the discovery probe): the gRPC GetServiceInfo keeps the last descriptor, because
   * `IamClients.serviceInfo` has no production caller today — only a test reads it directly, and
   * a test that scripts a real descriptor must not have it clobbered by an unrelated HTTP status
   * override.
   */
```
After:
```ts
  /**
   * A descriptor changes both the gRPC and the HTTP answer. `{ status }` changes ONLY the HTTP
   * route (the discovery probe). The gRPC GetServiceInfo keeps the last descriptor, so an unrelated
   * HTTP status override does not replace a descriptor that a test scripted.
   *
   * `IamClients` has no `serviceInfo` client (SMA-666), but the fake keeps the gRPC
   * `serviceInfo.getServiceInfo` route. The fake serves the full IAM surface, and
   * ts/apps/iam-console/tests/integration/doubles/fake-iam.test.ts:145-146 calls the route with its
   * own `ServiceInfoService` client.
   */
```

- [ ] **Step 10: Correct the citation at `testing/dev-world.ts:192`.**

Before:
```ts
    // the gRPC and the HTTP answer (fake-iam.ts:130-134). Leaving it unscripted lets `defaults()`
```
After:
```ts
    // the gRPC and the HTTP answer (fake-iam.ts:138-147). Leaving it unscripted lets `defaults()`
```
Check the range:
```bash
sed -n 138,148p /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy/ts/packages/paigasus-console-core/testing/fake-iam.ts
sed -n 145,146p /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy/ts/apps/iam-console/tests/integration/doubles/fake-iam.test.ts
```
Expected: line 138 is `  /**`, line 147 is `   */`, line 148 starts `  setServiceInfo(`. The second command prints the `createIamClient(ServiceInfoService, ...)` line and the `info.getServiceInfo({})` line.

- [ ] **Step 11: Review every `serviceInfo` hit.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
git grep -n 'serviceInfo' -- ts ':!**/generated/**'
```
Expected: hits only in `testing/fake-iam.ts` (`:71`, the rewritten comment, `:287-288`), `testing/dev-world.ts:188`, `tests/unit/dev-world.test.ts:48,55`, `tests/integration/discovery.test.ts` and both `tests/support/msw.ts` files (`serviceInfoHandlers`), `apps/iam-console/tests/integration/doubles/msw.test.ts`, and `apps/iam-console/tests/integration/doubles/fake-iam.test.ts:146`. No hit reads `IamClients` or `clients.serviceInfo`.

- [ ] **Step 12: Run the selected targets.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
moon run paigasus-console-core-ts:typecheck paigasus-console-core-ts:test iam-console-ts:typecheck iam-console-ts:test gateway-console-ts:typecheck gateway-console-ts:test
moon run ts:fmt ts:lint
moon run paigasus-console-core-ts:test-e2e iam-console-ts:test-e2e gateway-console-ts:test-e2e
```
Expected: all pass. The e2e line needs Docker; if one fails, use Task 7 Step 5. If Docker is not available, write that in the task report.

- [ ] **Step 13: Commit.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
git add ts/packages/paigasus-console-core/src/iam-clients.ts ts/packages/paigasus-console-core/tests/integration/error-info-round-trip.test.ts ts/apps/iam-console/tests/unit/action-session.test.ts ts/packages/paigasus-console-core/testing/fake-iam.ts ts/packages/paigasus-console-core/testing/dev-world.ts
git commit -F - <<'EOF'
refactor(ts): remove the unused IamClients.serviceInfo client (SMA-666)

No production code read IamClients.serviceInfo. Discovery uses fetch
against /v1/service-info, and the principal path uses authn.whoAmI.
IamClients now has six clients.

The round-trip test now calls every client, and it compares the called
clients with the keys of IamClients. A new client fails the test until
the test calls it, so the test name needs no number.

The fake IAM keeps its gRPC and HTTP service-info routes, because it
serves the full IAM surface and a fake-iam test calls the gRPC route.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
git log --oneline -1
```

---

## Task 5: Pin that `context_for` reads an API key's grants (spec commit 6)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs:242` (test-module import)
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` insert after `:1099` (the `    }` that closes `introspect_returns_an_empty_list_for_a_principal_without_grants`), before the module's closing `}` at `:1100`. After the insert, the new `async fn` is at `:1109`.
- Mutation site (temporary): after `authenticate_token.rs:181` (`role_grants.sort_by(...)`), inside `context_for` (`:163-184`).

Note: the SMA-633 grant tests are at `:948-1099`, and their fakes `FailingGrants` and `FixedOrderGrants` are at `:488-547`. Spec § 7.1 cites `:517-740`; that range is wrong.

**Interfaces:**
- Consumes: the test-module fakes `AuthnStore`, `InMemoryIdentities`, `InMemoryPrincipals`, `InMemoryMemberships`, `PanicIfCalledAuthenticator`; the helpers `principal_id(n)` and `epoch()`; `crate::application::fakes::{InMemoryRoleGrants, SeqIds, FixedClock}` (already imported at `:239`).
- Produces: `application::authenticate_token::tests::context_for_returns_the_grants_of_an_api_key_principal`. Task 6 cites this name and the line `authenticate_token.rs:1109`.

- [ ] **Step 1: Import `ApiKeyId` in the test module.** In `authenticate_token.rs:242`:

Before:
```rust
    use paigasus_iam_core::{GrantScope, Membership, MembershipRecord, RoleGrant, Stamp, TenancyNodeRef, TokenDefect, Transaction};
```
After:
```rust
    use paigasus_iam_core::{ApiKeyId, GrantScope, Membership, MembershipRecord, RoleGrant, Stamp, TenancyNodeRef, TokenDefect, Transaction};
```

- [ ] **Step 2: Add the test.** Insert after `authenticate_token.rs:1099`:

```rust

    /// SMA-666. `context_for` reads the grants of an API-key principal too, not only of an OIDC
    /// principal. `WhoAmI` calls `context_for` for both credential kinds
    /// (`adapters/grpc/authn.rs:105`, `adapters/http/authn.rs:119`), so a service account that
    /// calls `WhoAmI` with an API key gets its grants. Only `IntrospectApiKey` returns an empty
    /// list (SMA-633 D2), and it does not call this function. Before this test, no test read the
    /// grants of an API-key principal, so a change that skipped the grant read for an API key
    /// failed no test.
    #[tokio::test]
    async fn context_for_returns_the_grants_of_an_api_key_principal() {
        let org_prn = "prn:pgs:iam:::organization/22222222-2222-2222-2222-222222222222";
        let pid = principal_id(2);
        let org = TenancyNodeRef::from_prn(Prn::parse(org_prn).unwrap()).unwrap();
        let grants = InMemoryRoleGrants::default();
        grants
            .grant(&RoleGrant {
                id: Uuid::from_u128(3),
                principal: pid.clone(),
                role_key: "org_viewer".into(),
                scope: GrantScope::Node(org),
                linked_policy_id: "lp-3".into(),
                created_at: epoch(),
            })
            .await
            .unwrap();

        let store = AuthnStore::default();
        let uc = AuthenticateToken::new(
            PanicIfCalledAuthenticator,
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(grants),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[]),
        );

        let principal = AuthnPrincipal {
            principal_id: pid,
            kind: PrincipalKind::ServiceAccount,
            status: PrincipalStatus::Active,
            credential: Credential::ApiKey {
                key_id: ApiKeyId::from_uuid(Uuid::from_u128(2)),
                expires_at: None,
                scope_prn: org_prn.to_string(),
            },
        };

        let ctx = uc.context_for(principal).await.unwrap();
        assert_eq!(
            ctx.role_grants,
            vec![RoleGrantRef {
                scope_prn: org_prn.to_string(),
                role_key: "org_viewer".to_string(),
            }],
            "context_for must read the grants of an API-key principal: WhoAmI reports them for both credential kinds"
        );
    }
```
Why these APIs: `AuthenticateToken::new` takes `(A, E, P, M, Arc<dyn RoleGrantStore>, I, C, JitPolicy)` (`authenticate_token.rs:91`). `InMemoryRoleGrants::grant` is the `RoleGrantStore` trait method, in scope through `use super::*`. `InMemoryRoleGrants::list_by_principal` filters by principal (`src/application/fakes.rs:841-843`). `RoleGrantRef` derives `Debug, Clone, PartialEq, Eq` (`paigasus-iam-core/src/authz/model.rs:155`). `PanicIfCalledAuthenticator` proves that `context_for` verifies no token.

- [ ] **Step 3: Run the test green.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy/rs
cargo nextest run --locked -p paigasus-iam --lib context_for
grep -n 'async fn context_for_returns_the_grants_of_an_api_key_principal' crates/services/paigasus-iam/src/application/authenticate_token.rs
```
Expected: PASS for `context_for_returns_the_grants_of_an_api_key_principal`, `context_for_does_not_re_authenticate` and `context_for_returns_the_principals_memberships`. The grep prints `1109:`. If the line differs (for example after `cargo fmt`), use the printed number in Task 6.

- [ ] **Step 4: Mutation (skip the grant read for an API key).** Insert after `authenticate_token.rs:181` (`        role_grants.sort_by(|a, b| (&a.scope_prn, &a.role_key).cmp(&(&b.scope_prn, &b.role_key)));`):

```rust
        if matches!(principal.credential, Credential::ApiKey { .. }) { role_grants.clear(); } // SMA-666 MUTATION, remove
```
Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy/rs
cargo nextest run --locked -p paigasus-iam --lib --no-fail-fast
```
Expected: exactly one failure, `application::authenticate_token::tests::context_for_returns_the_grants_of_an_api_key_principal`, with left `[]` and right one `RoleGrantRef`. Every other unit test passes, because all other `context_for` and `introspect` tests use an OIDC principal. Record the failing test name. Remove the marked line with Edit.

- [ ] **Step 5: Verify.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
grep -n 'SMA-666 MUTATION' rs/crates/services/paigasus-iam/src/application/authenticate_token.rs
cd rs
cargo nextest run --locked -p paigasus-iam --lib
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```
Expected: the grep prints nothing. All unit tests pass. fmt and clippy pass. If `cargo fmt --check` fails, run `cargo fmt`, repeat the grep in Step 3, and use the new line number in Task 6.

- [ ] **Step 6: Commit.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
git add rs/crates/services/paigasus-iam/src/application/authenticate_token.rs
git commit -F - <<'EOF'
test(rs): pin that context_for reads an API key's grants (SMA-666)

WhoAmI calls context_for for an OIDC bearer and for an API-key bearer,
so a service account that calls WhoAmI gets its grants. No test read the
grants of an API-key principal. A change that skipped the grant read for
an API key failed no test.

The new unit test gives an API-key principal one grant and asserts that
context_for returns it. A mutation that clears the list for an API key
fails the test. SMA-633 D2 rests on this fact.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
git log --oneline -1
```

---

## Task 6: Clarify the SMA-632 and SMA-633 specs (spec commit 7)

**Files:**
- Modify: `docs/superpowers/specs/2026-09-20-sma-632-whoami-rpc-design.md` (insert after `:118` and after the § 9.5 item that ends at `:517` before the first insert)
- Modify: `docs/superpowers/specs/2026-09-20-sma-633-introspect-role-grants-design.md` (insert after `:252`)

**Interfaces:**
- Consumes: the test name `context_for_returns_the_grants_of_an_api_key_principal` and its line (`authenticate_token.rs:1109`) from Task 5.
- Produces: three blockquotes. No code.

Every reference that the notes cite was checked against the branch on 2026-09-23:

| Reference | Content at that line |
|---|---|
| `rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs:105` | `self.state.authn.context_for(principal)` in `who_am_i` |
| `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs:119` | `state.authn.context_for(principal)` in the `whoami` handler |
| `rs/crates/services/paigasus-iam/src/application/authenticate_api_key.rs:262-270` | `AuthenticateApiKey::introspect`, `role_grants: Vec::new()` at `:268` |
| `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs:64` | `iam.introspect_api_key(&key)` in `require_iam_auth` (starts `:53`) |
| `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs:166` | `iam.introspect_api_key(&token)` in `require_authenticated` (starts `:153`) |
| `rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs:105` | the gRPC `introspect_api_key` call inside `IamClient` |
| `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs:187` | `iam.introspect_token(&token)`, the OIDC leg of `require_authenticated` |
| SMA-633 spec `:85-94` | D2's paragraph "The rule is about the call site, not the credential" |
| SMA-633 spec `:169-178` | D6's "honest blast radius" paragraphs |
| SMA-632 spec `:479` | `## 8. Out of scope`; `:490` is "`IntrospectApiKey`. Unchanged." |

- [ ] **Step 1: Re-check the references.** Run each command and compare with the table.

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
sed -n 105p rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs
sed -n 119p rs/crates/services/paigasus-iam/src/adapters/http/authn.rs
sed -n 262,270p rs/crates/services/paigasus-iam/src/application/authenticate_api_key.rs
sed -n 64p rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs
sed -n 166p rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs
sed -n 187p rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs
sed -n 105p rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs
sed -n 1109p rs/crates/services/paigasus-iam/src/application/authenticate_token.rs
sed -n 85,94p docs/superpowers/specs/2026-09-20-sma-633-introspect-role-grants-design.md
sed -n 169,178p docs/superpowers/specs/2026-09-20-sma-633-introspect-role-grants-design.md
sed -n 490p docs/superpowers/specs/2026-09-20-sma-632-whoami-rpc-design.md
```
Expected: each line matches the table. Line 1109 is `    async fn context_for_returns_the_grants_of_an_api_key_principal() {`. If Task 5 reported a different line, use that number in Steps 2 and 4.

- [ ] **Step 2: Add the § 3.2 note to the SMA-632 spec.** The anchor is line 118, which is exactly:

```
credential kinds. § 9.5 carries the corrected note.
```
Insert after it (line 119 is blank today and stays blank before `### 3.3`):

```markdown

> **Clarified (SMA-666, 2026-09-23).** "Both credential kinds" above means both kinds of bearer on
> `WhoAmI`, and for that RPC the statement is true. `WhoAmI` calls `context_for` for an OIDC token
> and for an API key (`rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs:105`,
> `adapters/http/authn.rs:119`), and SMA-633 fills `role_grants` there. The unit test
> `context_for_returns_the_grants_of_an_api_key_principal`
> (`src/application/authenticate_token.rs:1109`) pins this.
>
> `IntrospectApiKey` is a different RPC. It returns an empty `role_grants` list
> (`src/application/authenticate_api_key.rs:262-270`), and SMA-633's D2 decided that. The gateway
> calls `IntrospectApiKey` on every request, through `require_iam_auth` (the model-invocation path)
> and through `require_authenticated`
> (`rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs:64` and `:166`, over
> `adapters/iam/client.rs:105`). Nothing reads the field. A grant-store read on the
> `require_iam_auth` path would turn a `role_grant` outage into a gateway 503 for model
> invocations.
>
> This does not mean that the gateway has no `role_grant` dependency. The OIDC leg of
> `require_authenticated` already reaches the grant store through `Introspect`
> (`paigasus-gateway/src/adapters/http/auth.rs:187`, SMA-633 D6 at `:169-178` of that spec).
>
> This spec never changed `IntrospectApiKey` (§ 8). So the two decisions do not disagree. The rest
> of this spec is left otherwise unedited.
```

- [ ] **Step 3: Add the § 9.5 note to the SMA-632 spec.** Step 2 adds 23 lines, so the § 9.5 item now ends at line 540, not 517. Find the anchor by text, not by number. The anchor is the line that is exactly:

```
   once with no further change here.
```
(It is the last line of item 5, directly before `6. **One PR is large** (D1).`.) Insert after it. The three-space indent keeps the note inside list item 5:

```markdown

   > **Clarified (SMA-666, 2026-09-23).** "The two messages" above are `IntrospectResponse` (OIDC
   > only) and `WhoAmIResponse` (both credential kinds). SMA-633 filled both. A third message,
   > `IntrospectApiKeyResponse`, stays empty by SMA-633 D2. The note at the end of § 3.2 gives the
   > reason.
```
Make sure one blank line separates the note from `6. **One PR is large** (D1).`.

- [ ] **Step 4: Add the D10 note to the SMA-633 spec.** The anchor is line 252, which is exactly:

```
Where they disagree, D2 governs this issue, and § 9 records why.
```
Insert after it (line 253 is blank today and stays blank before `## 4. Change list`):

```markdown

> **Clarified (SMA-666, 2026-09-23).** The divergence above is not real. SMA-632 § 3.2 and § 9.5
> describe `WhoAmI` and `IntrospectResponse`. They do not describe `IntrospectApiKey`. D2's own
> paragraph at `:85-94` ("The rule is about the call site, not the credential") gives the same
> result as SMA-632: an API-key bearer gets its grants through `WhoAmI`, and `IntrospectApiKey`
> returns an empty list. The SMA-632 spec now has a clarifying note at § 3.2 and at § 9.5. The
> unit test `context_for_returns_the_grants_of_an_api_key_principal`
> (`rs/crates/services/paigasus-iam/src/application/authenticate_token.rs:1109`) pins the fact.
> The rest of this spec is left otherwise unedited.
```

- [ ] **Step 5: Verify that the diff only adds lines, and that the notes sit at their anchors.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
git diff --numstat -- docs/superpowers/specs/2026-09-20-sma-632-whoami-rpc-design.md docs/superpowers/specs/2026-09-20-sma-633-introspect-role-grants-design.md
grep -n 'Clarified (SMA-666, 2026-09-23)' docs/superpowers/specs/2026-09-20-sma-632-whoami-rpc-design.md docs/superpowers/specs/2026-09-20-sma-633-introspect-role-grants-design.md
grep -c 'ci[R]eport' docs/superpowers/specs/2026-09-20-sma-632-whoami-rpc-design.md docs/superpowers/specs/2026-09-20-sma-633-introspect-role-grants-design.md
```
Expected: `--numstat` shows `28	0` for the SMA-632 spec and `9	0` for the SMA-633 spec (added lines, 0 deleted). The first grep prints `:120` and `:542` for the SMA-632 spec and `:254` for the SMA-633 spec. The last grep prints `0` for both files. Read each note in context once (`sed -n 116,142p` and the lines around the § 9.5 hit, and `sed -n 250,264p` on the SMA-633 spec).

- [ ] **Step 6: Commit.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
git add docs/superpowers/specs/2026-09-20-sma-632-whoami-rpc-design.md docs/superpowers/specs/2026-09-20-sma-633-introspect-role-grants-design.md
git commit -F - <<'EOF'
docs(repo): clarify the SMA-632 and SMA-633 specs on API-key grants (SMA-666)

SMA-633 D10 said that SMA-632 and D2 disagree about the grants of an
API-key caller. They do not. SMA-632 describes WhoAmI, which reports the
grants for both credential kinds. D2 describes IntrospectApiKey, which
stays empty. D2's own paragraph on the call site says the same thing.

Three notes record this where the claims are. Each note names the unit
test that pins the fact. The rest of each spec is unchanged.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
git log --oneline -1
```

---

## Task 7: Whole-branch verification (no commit)

**Files:** none are changed.

**Interfaces:**
- Consumes: the head of the branch after Task 6.
- Produces: the evidence for the PR description (gate results and the mutation table).

- [ ] **Step 1: Provision the worktree for the full graph.** `ts/node_modules` exists in this worktree; `py/.venv` does not.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
proto install
(cd py && uv sync)
(cd rs && cargo fetch --locked)
git fetch origin main
```

- [ ] **Step 2: Run the static checks once more on the head.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
git grep -n 'IntrospectPrincipalResolver' -- ':!docs/superpowers'
git grep -n 'clients.serviceInfo\|IamClients.serviceInfo' -- ts ':!**/generated/**'
git log --oneline origin/main..HEAD
```
Expected: the first two commands print nothing. The log shows the spec commits, the plan commit and the six task commits.

- [ ] **Step 3: Run the full gate graph.** The command is copied from the root `CLAUDE.md` (the `ci-targets` block):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :test-e2e \
  --base origin/main \
  --include-relations
```
Expected: every selected target passes, or fails only for a host reason from Step 4.

- [ ] **Step 4: Read each gate result with the bash rules of this Mac.** No single local bash runs every gate (root `CLAUDE.md`, "This development Mac only"). Moon runs `bash` from PATH, which is Homebrew bash 5.3.15 here.
  - `repo:affected-smoke` needs system bash 3.2. If it hangs (about 0% CPU) or fails, run it again through Moon with a bash-only shim. Do not put `/bin` first on PATH, because that also downgrades `python3` to 3.9.
    ```bash
    SHIM=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/87289770-06d6-44ce-a1be-3fddef35af24/scratchpad/bashshim
    mkdir -p "$SHIM" && ln -sf /bin/bash "$SHIM/bash"
    cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-666-whoami-tidy
    PATH="$SHIM:$HOME/.proto/shims:$HOME/.proto/bin:$PATH" moon run repo:affected-smoke --force
    ```
    Use the scratchpad of the session that runs this step, not the path above, if it differs.
  - `repo:ruff-ci`, `repo:next-public-free` and `repo:publish-metadata` need bash 4 or later. A wall of "expected rc 0" rows, or an empty stdout with `declare: -A: invalid option`, means bash 3.2 ran them. Run the gate directly: `/opt/homebrew/bin/bash ci/<gate>/run.sh`.
  - `repo:actionlint` gives a local verdict only when its pipe preflight reports at least 8192 bytes. An rc 2 with a "pipe ... holds only 512 bytes" message is a host condition, not a finding. CI is then its only verdict.
  - To diagnose any other unattributed failure, follow Step 0 to Step 2a between the `moon-diagnosis` markers in the root `CLAUDE.md`. Copy the moon cache report and the task state directory BEFORE any re-run.

- [ ] **Step 5: Treat Docker e2e failures as possible flakes first (spec § 8).** The changed files select `paigasus-auth-ts:test-e2e` (Keycloak, known flakes in SMA-652), `paigasus-console-core-ts:test-e2e` (Redis, no skip option, `cache: false`), `iam-console-ts:test-e2e`, `gateway-console-ts:test-e2e` (Docker) and `paigasus-iam-rs:test` (Docker-gated suites, flaky under parallel load; the `authz_policy_store.rs` concurrency tests are known to red). If one of them fails:
  1. Run the one target alone: `moon run <target> --force`.
  2. If it fails again, run the same target on unmodified `origin/main` in a separate worktree, and compare.
  3. Blame this branch only if the failure reproduces here and not on `origin/main`. This branch changes no runtime behaviour.

- [ ] **Step 6: Collect the PR evidence.** Write in the PR description:
  - The mutation table: for each of the seven mutations (Task 2 A-D, Task 3, Task 4, Task 5), the mutation and the failing test name that was recorded.
  - The gate results, with the bash that produced each verdict for the gates in Step 4.
  - Any gate that has no local verdict, and why.
