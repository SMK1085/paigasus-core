# SMA-666 — tidy-up left behind by the WhoAmI RPC (SMA-632)

- **Issue:** SMA-666 (priority Low, milestone "IAM Gaps")
- **Parent work:** SMA-632 (merged as `314ec074`, PR 276), SMA-633 (merged as `f2a78f6e`, PR 278)
- **Status:** approved design, revision 1

## 1. Purpose

SMA-632 left five small items. Reviewers found each item and judged it real. The SMA-632 PR
did not include them because that PR was already large. **No runtime behaviour is wrong.** This
issue does four things:

- It corrects one misleading name (item 1).
- It removes one unused client (item 4).
- It makes two assertions stronger (items 2 and 3).
- It corrects the cross-spec record (item 5).

**Success criteria.**

1. No production code or test code names `createIntrospectPrincipalResolver`.
2. An issuer/subject transposition in either WhoAmI mapper fails a test that has no Docker
   dependency.
3. `who_am_i_seeds_the_bootstrap_admin` fails if a principal for its identity exists before the
   `WhoAmI` call.
4. `IamClients` has no `serviceInfo` field. The fake IAM continues to serve
   `serviceInfo.getServiceInfo` over gRPC and `/v1/service-info` over HTTP.
5. The SMA-632 and SMA-633 specs no longer disagree about role grants for an API-key caller.

## 2. Decisions

| # | Topic | Decision |
|---|---|---|
| D1 | New name for `createIntrospectPrincipalResolver` | `createPrincipalResolver`. Chosen by Sven on 2026-09-23. |
| D2 | Deprecated alias for the old name | None. `@paigasus/console-core` is `"private": true` and has no `publishConfig`. All its consumers are in this repo. |
| D3 | Historical documents that name the old function | Do not edit them. See § 4.1. |
| D4 | Before-state for item 3 | Assert that the identity is not provisioned. See § 5.2. |
| D5 | The round-trip test that reads `clients.serviceInfo` | Replace that call with calls to the two clients that the test does not cover now. See § 6.2. |
| D6 | Item 5 | Clarify SMA-632 § 3.2 and § 9.5, and SMA-633 D10. Do not make the correction that the issue text asks for. Chosen by Sven on 2026-09-23. See § 7. |
| D7 | Delivery | One PR. One commit for each item. |

## 3. What the issue text gets wrong

The issue text for item 5 asks for this correction: "the OIDC path gets real grants while the
API-key path stays empty". **That statement is false for `WhoAmI`.** This spec does not write it.

The evidence:

- The gRPC `who_am_i` handler (`rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs:105`)
  and the HTTP `whoami` handler (`src/adapters/http/authn.rs:119`) both call
  `state.authn.context_for(principal)` for every credential kind.
- `AuthenticateToken::context_for` (`src/application/authenticate_token.rs:163-184`) calls
  `list_by_principal` without a condition on the credential kind.
- So a service account that calls `WhoAmI` with an API key gets its full grant set.
- Only `IntrospectApiKey` returns an empty list. SMA-632 did not change that RPC (SMA-632 spec
  § 8).
- SMA-633's own D2 says the same thing in its last paragraph: "The rule is about the call site,
  not the credential."

The claim in SMA-632 § 3.2 is about a `WhoAmI` call with an API-key bearer. For that call, the
claim is true. The statement that is not accurate is SMA-633 D10. D10 says that SMA-632 and D2
disagree, but D2's last paragraph agrees with SMA-632. § 7 gives the correction.

The issue's wider lesson stays correct: a spec must not state what a different, unstarted issue
will do. This case adds a second lesson. A correction must be checked against the code before it
is written. The correction that the issue asked for was also a claim about another issue's
behaviour, and it was not correct either.

## 4. Item 1 — rename `createIntrospectPrincipalResolver`

### 4.1 Scope

The function makes one `WhoAmI` call. Its header comment says so
(`ts/packages/paigasus-console-core/src/principal-resolver.ts`). Only its name still says
`Introspect`.

The other `create*` factories in `console-core/src/` use the name of the type they return:
`createIamClients`, `createJsonLogger`, `createConsoleRuntime`, `createMayI`,
`createAppDiscovery`. `createPrincipalResolver` returns a `PrincipalResolver`, so it follows
this convention. A name that does not name an RPC cannot become wrong again when the RPC changes.

Change these places:

| File | Change |
|---|---|
| `ts/packages/paigasus-console-core/src/principal-resolver.ts:36` | Rename the function. |
| `ts/packages/paigasus-console-core/src/index.ts:16` | Rename the export. |
| `ts/apps/iam-console/lib/auth.ts:9,30` | Rename the import and the call. |
| `ts/apps/gateway-console/lib/auth.ts:9,30` | Rename the import and the call. |
| `ts/packages/paigasus-console-core/tests/integration/principal.test.ts` | Rename the import and the eight call sites. |
| `ts/packages/paigasus-auth/src/adapters/claims-resolver.ts:9` | Correct the comment. It names `IntrospectPrincipalResolver` and SMA-508. |
| `ts/packages/paigasus-auth/src/ports/principal-resolver.ts:8` | Correct the comment in the same way. |

The two `paigasus-auth` comments predict where SMA-508 will put the adapter. That adapter now
exists as `createPrincipalResolver` in `@paigasus/console-core`. Each comment must name it and
its package. Each comment must not name a future issue.

**Do not change** `principal.test.ts:169`
(`'names the same principal Introspect names for the same token'`). That test compares `WhoAmI`
with the `Introspect` RPC. The word `Introspect` in it is correct.

**Do not change** the specs and plans under `docs/superpowers/`. They name the old function in
about 30 places across the SMA-506, 508, 511, 512, 632, 633 and 662 documents. Each document is a
record of what was true when it was written.

### 4.2 Verification

- `git grep -n 'createIntrospectPrincipalResolver\|IntrospectPrincipalResolver' -- ':!docs/superpowers'`
  returns no line.
- `moon run paigasus-console-core-ts:typecheck paigasus-console-core-ts:test iam-console-ts:typecheck gateway-console-ts:typecheck`
  passes. These are the project IDs that `moon query projects` reports.
- `ts:fmt` (Prettier) passes. A shorter identifier can change line wrapping.

## 5. Items 2 and 3 — stronger WhoAmI assertions (Rust)

### 5.1 Item 2 — the OIDC arm of both mappers

**`convert.rs`.** `to_who_am_i_response_reports_an_oidc_caller_in_full`
(`rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:1576-1582`) asserts only
`!response.issuer.is_empty()` and `!response.subject.is_empty()`. The fixture `oidc_context()`
(`:1536-1553`) sets issuer `https://idp.example.com` and subject `subject-51`. Both values are
not empty, so a transposition passes.

Change the two assertions:

```rust
assert_eq!(response.issuer, "https://idp.example.com");
assert_eq!(response.subject, "subject-51");
```

`Issuer::parse` trims the input and stores it without other normalization
(`rs/crates/libs/paigasus-iam-core/src/authn.rs:27-40`). So `as_str()` returns the literal
exactly.

Also make the expiry exact. The fixture sets `expires_at: Utc::now() + 1h`, and the test asserts
only `is_some()`. Change `oidc_context()` to take `expires_at: DateTime<Utc>` as a parameter,
in the same way that `api_key_context` does. Then assert
`response.expires_at == Some(ts(expiry))`. The existing test
`to_who_am_i_response_reports_the_callers_role_grants` also calls `oidc_context()`. Update that
call site.

**`dto.rs`.** No test covers the `Credential::Oidc` arm of `WhoAmIResponseDto::from`
(`rs/crates/services/paigasus-iam/src/adapters/http/dto.rs:299-319`). All three WhoAmI DTO tests
use `api_key_context`. Add:

- A fixture `oidc_context(expires_at: DateTime<Utc>) -> PrincipalContext` in the `dto.rs` test
  module. It must use the same literals as the `convert.rs` fixture: issuer
  `https://idp.example.com`, subject `subject-51`, `principal(51)`, `PrincipalKind::User`.
- A test `who_am_i_response_dto_reports_an_oidc_caller_in_full`. It asserts
  `dto.issuer == "https://idp.example.com"`, `dto.subject == "subject-51"` and
  `dto.expires_at == Some(expiry)`.

Do not move the fixtures into a shared module. Each test module in this crate has its own local
fixtures. The SMA-633 tests in the same modules do the same.

**Proof by mutation (mandatory).** For each mapper, swap issuer and subject and run the test:

- `convert.rs` `to_who_am_i_response`: swap the two values in the `Credential::Oidc` arm.
- `dto.rs` `WhoAmIResponseDto::from`: change the tuple at `:304` to
  `(subject, issuer.as_str().to_string())`.

Both values are `String`, so each mutation compiles. Under `-D warnings`, a mutation that does
not compile proves nothing. Run the crate's unit tests with `--no-fail-fast`. The new or changed
OIDC test must fail. Then remove the mutation with the Edit tool. Do not use
`git checkout --`, because that also removes the uncommitted test change. Record the failing test
name for each mutation in the PR description.

### 5.2 Item 3 — before-state for `who_am_i_seeds_the_bootstrap_admin`

The test (`rs/crates/services/paigasus-iam/tests/grpc_whoami.rs:214-243`) asserts one
`platform_admin`@`Root` grant after the `WhoAmI` call. It asserts nothing before the call.

A before-query by principal ID is not possible. The principal does not exist until the call
provisions it, and `RoleGrantStore::list_by_principal` takes a `&PrincipalId`. But a grant needs
a principal. So "the identity is not provisioned" before the call proves "the grant is not
present" before the call.

Add this assertion after `spawn_server` and before the `who_am_i` call:

```rust
assert!(
    matches!(state.authn.resolve(&token, Provisioning::Disabled).await, Err(AuthnError::IdentityNotProvisioned)),
    "the bootstrap identity must not exist before the WhoAmI call, or its grant proves nothing",
);
```

`resolve(.., Provisioning::Disabled)` returns `Err(AuthnError::IdentityNotProvisioned)` for an
unknown identity (`src/application/authenticate_token.rs:115`). It never provisions (D10 of the
authn design), so the assertion does not change the state that it examines.

The sibling test `who_am_i_returns_memberships` (`:193`) uses a before-and-after pair of `WhoAmI`
calls. That pattern cannot apply here, because the first `WhoAmI` call itself seeds the grant.

**Proof by mutation.** Before the new assertion, add a temporary line that provisions the
identity: `state.authn.resolve(&token, Provisioning::Enabled).await.unwrap();`
(`Provisioning` is `{ Enabled, Disabled }`, `src/application/authenticate_token.rs:20-23`). The
test must then fail on the new assertion. Remove the line.

**Docker.** This file is Docker-gated. Run it with `PAIGASUS_REQUIRE_DOCKER=1`, so that a
missing daemon causes a failure and not a skip that shows no message. Docker-gated suites in this
crate are flaky under parallel load. If a test fails, run it alone before you blame this change.

### 5.3 Verification

- `cargo nextest run -p paigasus-iam --lib` passes.
- `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_whoami` passes.
- `cargo fmt --check` and `cargo clippy --workspace -- -D warnings` pass.
- The three mutations in §§ 5.1-5.2 each fail the named test.

## 6. Item 4 — remove `IamClients.serviceInfo`

### 6.1 Production code

`ts/packages/paigasus-console-core/src/iam-clients.ts` builds seven Connect clients. No
production code reads `serviceInfo`:

- Capability discovery uses `fetch` against `/v1/service-info`
  (`ts/packages/paigasus-discovery/src/probe.ts`, through `console-core/src/discovery.ts`).
- The principal path uses `authn.whoAmI`.
- No `satisfies IamClients`, `keyof IamClients` or key count exists anywhere in `ts/`.

Change:

- Remove the `serviceInfo` field from the `IamClients` type (`:24`).
- Remove its construction (`:61`).
- Remove `ServiceInfoService` from the import at `:16`. After the change, nothing else in the
  file uses it.
- Change the doc comment at `:51` from "The seven clients" to "The six clients".

### 6.2 Tests and doubles

**`tests/integration/error-info-round-trip.test.ts`** is the only reader of
`clients.serviceInfo` (`:54`). The test is named
`'sends the correlation header from every one of the five clients'`. That name is already wrong:
`IamClients` has seven clients, and the test calls five of them.

Remove the `serviceInfo` call. Add `clients.serviceAccounts.listServiceAccounts({})` and
`clients.outbox.listDeadLetters({})`. Both methods are read-only, and the apps already call them.
Then the test calls all six clients. Rename it to
`'sends the correlation header from every one of the six clients'`. Update the `expect` list to
six entries: `serviceAccounts.listServiceAccounts` and `outbox.listDeadLetters` replace
`serviceInfo.getServiceInfo`.

The fake registers both services in its `SERVICES` map (`testing/fake-iam.ts:66-77`). Neither
service has a default answer, so each unscripted call answers `Unimplemented`. That is
acceptable: the fake records each call before it runs a handler (the comment at `:51-53`), so the
test still sees the header. Extend the comment at `:51-53` to name these two failures.

**`testing/fake-iam.ts:138-144`.** The comment says "`IamClients.serviceInfo` has no production
caller today — only a test reads it directly". After this change, that field does not exist.
Rewrite the comment so it says why the gRPC `serviceInfo.getServiceInfo` route stays: the fake
serves the full IAM surface, and other tests build a `ServiceInfoService` client directly
(`ts/apps/iam-console/tests/integration/doubles/fake-iam.test.ts:146-150`,
`ts/packages/paigasus-sdk/tests/iam-service-info.test.ts`).

**Do not change:**

- `FakeIam.setServiceInfo`, the `SERVICES.serviceInfo` entry, and the default handler for
  `serviceInfo.getServiceInfo` (`fake-iam.ts:71, 287-288, 378-381`).
- The HTTP `/v1/service-info` route of the fake.
- `testing/dev-world.ts` and `tests/unit/dev-world.test.ts`. They check the keys of a handler map,
  not of `IamClients`. The fourth capability key that SMA-633 added (`iam.authn.grants`) is not
  affected.
- The e2e worlds and harnesses of both apps. They call `setServiceInfo` on the fake server.

### 6.3 Verification

- `git grep -n 'clients\.serviceInfo\|IamClients\.serviceInfo' -- ts` returns no line.
- The console-core `typecheck` and `test` targets pass. The two apps' `typecheck` and `test`
  targets pass.
- The round-trip test lists six methods, one for each client.

## 7. Item 5 — clarify the SMA-632 and SMA-633 specs

### 7.1 Form of the note

The repo corrects a merged document with a note. The rest of the document stays unchanged. The
example is the `**Superseded (SMA-597).**` block in
`docs/superpowers/plans/2026-08-19-sma-541-ci-target-coverage-gate.md:1284-1288`. It names the
issue, states what is wrong, points to the correct source, and says that the rest of the
document is "left otherwise unedited".

This change adds a blockquote that starts with `**Clarified (SMA-666, 2026-09-23).**`. The word
is "Clarified", not "Superseded", because the original claim is true for `WhoAmI` and stays in
place. The note goes directly after the paragraph that it clarifies, not at the end of the file.
The reader needs it at the point of the claim. Do not add an HTML marker comment. The
`moon-diagnosis:superseded` marker belongs to one gate.

### 7.2 SMA-632 spec (`docs/superpowers/specs/2026-09-20-sma-632-whoami-rpc-design.md`)

**After § 3.2's last paragraph (the one that ends "§ 9.5 carries the corrected note.").** The
note must say:

- "Both credential kinds" means both kinds of bearer on `WhoAmI`. That statement is true.
  `WhoAmI` calls `context_for` for an OIDC token and for an API key
  (`adapters/grpc/authn.rs:105`, `adapters/http/authn.rs:119`), and SMA-633 fills
  `role_grants` there.
- `IntrospectApiKey` returns an empty `role_grants` list. SMA-633's D2 decided this. The reason:
  the gateway calls it on every request (`require_iam_auth` and `require_authenticated`, through
  `paigasus-gateway/src/adapters/iam/client.rs:105`). Nothing reads the field. A grant-store read
  on that path would turn a `role_grant` outage into a gateway 503.
- This spec never changed `IntrospectApiKey` (§ 8). So the two decisions do not disagree.
- The rest of this spec is left otherwise unedited.

**After § 9.5 (item 5 of the § 9 list).** A shorter note: "The two messages" are
`IntrospectResponse` (OIDC only) and `WhoAmIResponse` (both credential kinds). SMA-633 filled
both. The third message, `IntrospectApiKeyResponse`, stays empty by SMA-633 D2. Refer to the § 3.2
note for the reason.

### 7.3 SMA-633 spec (`docs/superpowers/specs/2026-09-20-sma-633-introspect-role-grants-design.md`)

**After D10's "One divergence to know about" paragraph (`:247-252`).** The note must say:

- The divergence is not real. SMA-632 § 3.2 and § 9.5 describe `WhoAmI` and `IntrospectResponse`.
  They do not describe `IntrospectApiKey`.
- D2's own last paragraph ("The rule is about the call site, not the credential") gives the same
  result as SMA-632: an API-key bearer gets its grants through `WhoAmI`.
- The SMA-632 spec now has a clarifying note at § 3.2 and § 9.5.

### 7.4 Verification

- Each note cites file and line references that exist on `main` when the note is written. Check
  each reference with `sed -n`.
- `git diff` on each spec shows only added lines.

## 8. Testing summary

| Item | Tier | Proof |
|---|---|---|
| 1 | TS typecheck, unit, integration | grep returns no line. All targets pass. |
| 2 | Rust unit (always runs) | Two literal-value tests. Two mutations fail them. |
| 3 | Rust integration (Docker) | New assertion passes. A pre-provision mutation fails it. Run with `PAIGASUS_REQUIRE_DOCKER=1`. |
| 4 | TS typecheck, integration | grep returns no line. The round-trip test covers six clients. |
| 5 | Docs | References checked. Diff has only added lines. |

Before the push, run the full gate graph from the root `CLAUDE.md` (`moon ci … --base origin/main
--include-relations`). The changed files affect `paigasus-iam-rs`, console-core and both apps.
Read the local-bash rules in the root `CLAUDE.md` before you read a gate result.

## 9. Out of scope

- Historical specs and plans that name the old function (D3).
- A shared Rust fixture module for OIDC identities. Each test module keeps its own fixtures.
- Any change to `IntrospectApiKey`, the gateway or the fake's `setServiceInfo`.
- A credential-kind discriminator on `WhoAmIResponse`. SMA-632 § 9.4 records that limit.
