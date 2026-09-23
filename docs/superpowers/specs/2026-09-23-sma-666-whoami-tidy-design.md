# SMA-666 — tidy-up left behind by the WhoAmI RPC (SMA-632)

- **Issue:** SMA-666 (priority Low, milestone "IAM Gaps")
- **Parent work:** SMA-632 (merged as `314ec074`, PR 276), SMA-633 (merged as `f2a78f6e`, PR 278)
- **Status:** revision 2 (after the adversarial challenge). The change log is in § 10.

## 1. Purpose

SMA-632 left five small items. Reviewers found each item and judged it real. The SMA-632 PR
did not include them because that PR was already large. **No runtime behaviour is wrong.** This
issue does four things:

- It corrects one misleading name (item 1).
- It removes one unused client (item 4).
- It makes assertions stronger (items 2 and 3).
- It corrects the cross-spec record, and it adds one test that pins the fact the record states
  (item 5).

**Success criteria.**

1. No production code or test code names `createIntrospectPrincipalResolver`. No comment names
   `IntrospectPrincipalResolver`.
2. An issuer/subject transposition in either WhoAmI mapper fails a test that has no Docker
   dependency. Each mapper's OIDC test also asserts the exact expiry.
3. `who_am_i_seeds_the_bootstrap_admin` fails if a principal for its identity exists before the
   `WhoAmI` call.
4. `IamClients` has no `serviceInfo` field. The fake IAM continues to serve
   `serviceInfo.getServiceInfo` over gRPC and `/v1/service-info` over HTTP. The round-trip test
   fails if a client is added to `IamClients` and the test does not call it.
5. The SMA-632 and SMA-633 specs no longer disagree about role grants for an API-key caller.
6. A test with no Docker dependency fails if `context_for` stops reading grants for an API-key
   principal.

## 2. Decisions

| # | Topic | Decision |
|---|---|---|
| D1 | New name for `createIntrospectPrincipalResolver` | `createPrincipalResolver`. Chosen by Sven on 2026-09-23. |
| D2 | Deprecated alias for the old name | None. `@paigasus/console-core` is `"private": true` and has no `publishConfig`. All its consumers are in this repo. |
| D3 | Historical documents that name the old function | Do not edit them. See § 4.1. |
| D4 | Before-state for item 3 | Assert that the identity is not provisioned. See § 5.2. |
| D5 | The round-trip test that reads `clients.serviceInfo` | Call every client, and assert that the set of called clients equals the set of `IamClients` keys. See § 6.2. |
| D6 | Item 5 | Clarify SMA-632 § 3.2 and § 9.5, and SMA-633 D10. Do not make the correction that the issue text asks for. Chosen by Sven on 2026-09-23. See § 7. |
| D7 | Delivery | One PR. The commits are listed in § 2.1. |
| D8 | A test for the fact that item 5 records | Add one unit test in `authenticate_token.rs`. See § 7.1. |

### 2.1 Commits

Each commit uses a workspace scope from `ts/packages/commitlint-config/index.cjs:42`. No commit
body has a line that starts with `#` followed by a number, and no line of the form `Token: value`
except the trailer. Those lines fail `footer-leading-blank`.

| # | Scope and subject | Content |
|---|---|---|
| 1 | `docs(repo): spec for the WhoAmI tidy-up (SMA-666)` | This spec. Already committed. |
| 2 | `refactor(ts): name the principal resolver for what it returns (SMA-666)` | Item 1. |
| 3 | `test(rs): pin the OIDC arm of both WhoAmI mappers (SMA-666)` | Item 2. |
| 4 | `test(rs): assert the bootstrap identity is absent before WhoAmI (SMA-666)` | Item 3. |
| 5 | `refactor(ts): remove the unused IamClients.serviceInfo client (SMA-666)` | Item 4. |
| 6 | `test(rs): pin that context_for reads an API key's grants (SMA-666)` | Item 5, the test (§ 7.1). |
| 7 | `docs(repo): clarify the SMA-632 and SMA-633 specs on API-key grants (SMA-666)` | Item 5, the notes (§§ 7.2-7.4). |

Commit 6 comes before commit 7, so the notes are written after the fact is pinned. Each commit
must pass the targets that its own files select. The full gate graph runs on the head of the PR
(§ 8).

## 3. What the issue text gets wrong

The issue text for item 5 asks for this correction: "the OIDC path gets real grants while the
API-key path stays empty". **That statement is false for `WhoAmI`.** This spec does not write it.

The evidence. The spec challenger traced it again on both transports and confirmed it.

- **gRPC.** `AuthEnforce::call` sends a token with the API-key prefix to `api_key_auth.resolve`
  (`rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs:217-221`). It inserts an
  `AuthContext` for either credential kind (`:233-238`). `who_am_i` then calls
  `state.authn.context_for(principal)` (`:105`).
- **HTTP.** `require_bearer` does the same (`src/adapters/http/auth_middleware.rs:51-55, 69-74`).
  The `whoami` route is inside `protected` (`src/adapters/http/mod.rs:927, 944`), and the handler
  calls `context_for` (`src/adapters/http/authn.rs:119`).
- **The grant read.** `AuthenticateToken::context_for`
  (`src/application/authenticate_token.rs:163-184`) calls `list_by_principal` without a condition
  on the credential kind. Only `AuthenticateApiKey::introspect` sets `role_grants: Vec::new()`
  (`src/application/authenticate_api_key.rs:262-270`).
- So a service account that calls `WhoAmI` with an API key gets its full grant set. Only
  `IntrospectApiKey` returns an empty list. SMA-632 did not change that RPC (SMA-632 spec § 8).
- SMA-633's D2 says the same thing in the paragraph that starts "The rule is about the call site,
  not the credential" (SMA-633 spec `:85-94`).

The claim in SMA-632 § 3.2 is about a `WhoAmI` call with an API-key bearer. For that call, the
claim is true. The statement that is not accurate is SMA-633 D10 (`:247-252`). D10 says that
SMA-632 and D2 disagree, but D2's own paragraph at `:85-94` agrees with SMA-632. § 7 gives the
correction.

**No test pins this fact today.** `who_am_i_serves_an_api_key_bearer`
(`tests/grpc_whoami.rs:249-300`) does not assert `role_grants`. `context_for_fixture`
(`authenticate_token.rs:867-901`) builds only an OIDC principal. The only API-key `role_grants`
assertions check for an EMPTY list, on `IntrospectApiKey` (`tests/api_keys_grpc.rs:146`,
`tests/http_service_accounts.rs:180`). So a change that skips the grant read for an API key reds
nothing. That is the reading that D10 made. § 7.1 adds the test.

The issue's wider lesson stays correct: a spec must not state what a different, unstarted issue
will do. This case adds a second lesson. A correction must be checked against the code before it
is written, and the fact that it records must be pinned by a test. The correction that the issue
asked for was also a claim about another issue's behaviour, and it was not correct either.

## 4. Item 1 — rename `createIntrospectPrincipalResolver`

### 4.1 Scope

The function makes one `WhoAmI` call. Its header comment says so
(`ts/packages/paigasus-console-core/src/principal-resolver.ts:3-6`). Only its name still says
`Introspect`.

`createPrincipalResolver` returns a `PrincipalResolver`. Three of the other `create*` factories in
`console-core/src/` also use the name of the type they return: `createIamClients`,
`createConsoleRuntime` and `createMayI`. Two do not: `createJsonLogger` returns `ConsoleLogger`
(`src/logger.ts:42`), and `createAppDiscovery` returns `Discovery` (`src/discovery.ts:372`). Those
two names add a qualifier, but neither names an RPC. The reason for D1 is that a name that does
not name an RPC cannot become wrong again when the RPC changes.

**Renames.**

| File | Change |
|---|---|
| `ts/packages/paigasus-console-core/src/principal-resolver.ts:36` | Rename the function. |
| `ts/packages/paigasus-console-core/src/index.ts:16` | Rename the export. |
| `ts/apps/iam-console/lib/auth.ts:9,30` | Rename the import and the call. |
| `ts/apps/gateway-console/lib/auth.ts:9,30` | Rename the import and the call. |
| `ts/packages/paigasus-console-core/tests/integration/principal.test.ts` | Rename the import and the eight call sites. |

**Comment rewrites.** Each rewrite describes a role, not a downstream name or a future issue. A
lower-level package that names a function in a higher-level package goes stale when that function
changes. That is how item 1 started.

| File and lines | Now | Rewrite to say |
|---|---|---|
| `ts/packages/paigasus-auth/src/adapters/claims-resolver.ts:6-10` | Introspect is "not reachable from TypeScript yet", `@paigasus/sdk` "is a stub", "SMA-508 lands the transport; IntrospectPrincipalResolver then slots in". All of it is false today. | This resolver derives identity from the ID-token claims only. It is the resolver for a host that has no IAM transport. A host with IAM (the consoles) supplies an IAM-backed resolver behind the same port, with no change to the session shape or the store. |
| `ts/packages/paigasus-auth/src/ports/principal-resolver.ts:8` | "When IntrospectPrincipalResolver lands in SMA-508, the ADAPTER maps proto to these types." | An IAM-backed adapter maps proto to these types. Keep it on one line, so that the pointer `ports/principal-resolver.ts:3-8` in `principal-resolver.ts:9` stays correct. |
| `ts/packages/paigasus-console-core/src/principal-resolver.ts:8-10` | "It lives in the APP because …" and "SMA-631 records a shared home for SMA-512." The resolver is now in `@paigasus/console-core`, and both issues are done. | It lives in `@paigasus/console-core`, not in `@paigasus/auth`, because `@paigasus/auth` must not import `@paigasus/sdk` (keep the pointer to `ports/principal-resolver.ts:3-8`), and the sdk boundary rule bans every `@paigasus/*` import except proto. Remove the SMA-631 and SMA-512 sentence. |

**Do not change** `principal.test.ts:169`
(`'names the same principal Introspect names for the same token'`). That test compares `WhoAmI`
with the `Introspect` RPC. The word `Introspect` in it is correct.

**Do not change** the specs and plans under `docs/superpowers/`. They name the old function in
about 30 places across the SMA-506, 508, 511, 512, 632, 633 and 662 documents. Each document is a
record of what was true when it was written. This rule is for the rename only. § 7 adds
clarification notes to two of these specs, and it changes no existing line in them.

### 4.2 Verification

- `git grep -n 'IntrospectPrincipalResolver' -- ':!docs/superpowers'` returns no line. This
  pattern also matches `createIntrospectPrincipalResolver`.
- These targets pass: `paigasus-console-core-ts:typecheck`, `paigasus-console-core-ts:test`,
  `iam-console-ts:typecheck`, `iam-console-ts:test`, `gateway-console-ts:typecheck`,
  `gateway-console-ts:test`, `paigasus-auth-ts:typecheck` and `paigasus-auth-ts:test`.
- `ts:fmt` (Prettier) and `ts:lint` pass. A shorter identifier can change line wrapping.

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
`response.expires_at == Some(ts(expiry))`. The helper `ts` (`convert.rs:196-201`) is
deterministic, and the mapper applies it to the same value, so the comparison is exact. The API-key
test at `:1564` already uses this pattern. `oidc_context()` has two callers (`:1577`, `:1590`).
Update both.

**`dto.rs`.** No test covers the `Credential::Oidc` arm of `WhoAmIResponseDto::from`
(`rs/crates/services/paigasus-iam/src/adapters/http/dto.rs:299-319`). All three WhoAmI DTO tests
use `api_key_context`. Add:

- A fixture `oidc_context(expires_at: DateTime<Utc>) -> PrincipalContext` in the `dto.rs` test
  module. It must use the same literals as the `convert.rs` fixture: issuer
  `https://idp.example.com`, subject `subject-51`, `principal(51)`, `PrincipalKind::User`.
  Import `Issuer` from `paigasus_iam_core`. Neither the test module (`:741`) nor the file header
  (`:10-13`) imports it today.
- A test `who_am_i_response_dto_reports_an_oidc_caller_in_full`. It asserts
  `dto.issuer == "https://idp.example.com"`, `dto.subject == "subject-51"` and
  `dto.expires_at == Some(expiry)`.

Do not move the fixtures into a shared module. Each test module in this crate has its own local
fixtures. The SMA-633 tests in the same modules do the same.

**Proof by mutation (mandatory).** For each mapper, do two mutations, one at a time:

| Mapper | Mutation | Test that must fail |
|---|---|---|
| `convert.rs` `to_who_am_i_response` | Swap issuer and subject in the `Credential::Oidc` arm. | `to_who_am_i_response_reports_an_oidc_caller_in_full` |
| `convert.rs` `to_who_am_i_response` | Replace the OIDC expiry with `Some(ts(Utc::now()))`. | the same test |
| `dto.rs` `WhoAmIResponseDto::from` | Change the tuple at `:304` to `(subject, issuer.as_str().to_string())`. | `who_am_i_response_dto_reports_an_oidc_caller_in_full` |
| `dto.rs` `WhoAmIResponseDto::from` | Replace `expires_at` with `Some(Utc::now())`. | the same test |

Each mutation keeps the types, so it compiles. Under `-D warnings`, a mutation that does not
compile proves nothing. If a mutation leaves a binding unused, prefix it with `_` so that it still
compiles. Run the crate's unit tests with `--no-fail-fast`. Then remove the mutation with the
Edit tool. Do not use `git checkout --`, because that also removes the uncommitted test change.
Record the failing test name for each mutation in the PR description.

### 5.2 Item 3 — before-state for `who_am_i_seeds_the_bootstrap_admin`

The test (`rs/crates/services/paigasus-iam/tests/grpc_whoami.rs:214-243`) asserts one
`platform_admin`@`Root` grant after the `WhoAmI` call. It asserts nothing before the call.

A before-query by principal ID is not possible. The principal does not exist until the call
provisions it, and `RoleGrantStore::list_by_principal` takes a `&PrincipalId`. But a grant needs
a principal. So "the identity is not provisioned" before the call proves "the grant is not
present" before the call.

Add this after `spawn_server` and before the `who_am_i` call:

```rust
let before = state.authn.resolve(&token, Provisioning::Disabled).await;
assert!(
    matches!(before, Err(AuthnError::IdentityNotProvisioned)),
    "the bootstrap identity must not exist before the WhoAmI call, or its grant proves nothing: {before:?}",
);
```

Add `AuthnError` to the file's imports (`:23-39`). `AuthnError` does not implement `PartialEq`
(`paigasus-iam-core/src/authn.rs:183-184`), so `assert_eq!` does not compile, and `matches!` is
correct. The message prints the actual result, so an IdP or JWKS fault does not read as a state
leak.

`resolve(.., Provisioning::Disabled)` returns `Err(AuthnError::IdentityNotProvisioned)` only from
the branch where no identity exists (`src/application/authenticate_token.rs:114-115`). The same
call already succeeds later in this test (`:236`), so the IdP and JWKS path works with this
configuration. The call writes nothing to the database. It only fills the JWKS cache.

The sibling test `who_am_i_returns_memberships` (`:193`) uses a before-and-after pair of `WhoAmI`
calls. That pattern cannot apply here, because the first `WhoAmI` call itself seeds the grant.

**Proof by mutation.** Before the new assertion, add a temporary line that provisions the
identity: `state.authn.resolve(&token, Provisioning::Enabled).await.unwrap();`
(`Provisioning` is `{ Enabled, Disabled }`, `src/application/authenticate_token.rs:20-23`, and
the file already imports it at `:29`). The test must then fail on the new assertion. Remove the
line.

**Docker.** This file is Docker-gated. Run it with `PAIGASUS_REQUIRE_DOCKER=1`, so that a
missing daemon causes a failure and not a skip that shows no message. Docker-gated suites in this
crate are flaky under parallel load. If a test fails, run it alone before you blame this change.

### 5.3 Verification

- `cargo nextest run -p paigasus-iam --lib` passes.
- `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_whoami` passes.
- `cargo fmt --check` passes.
- `cargo clippy --locked --all-targets -- -D warnings` passes, or `moon run paigasus-iam-rs:lint`.
  Without `--all-targets`, clippy does not lint `#[cfg(test)]` modules or `tests/*.rs`, and every
  Rust change in this issue is test code (`.moon/tasks/rust.yml:79`).
- Each mutation in §§ 5.1-5.2 fails the named test.

## 6. Item 4 — remove `IamClients.serviceInfo`

### 6.1 Production code

`ts/packages/paigasus-console-core/src/iam-clients.ts` builds seven Connect clients. No
production code reads `serviceInfo`:

- Capability discovery uses `fetch` against `/v1/service-info`
  (`ts/packages/paigasus-discovery/src/probe.ts`, through `console-core/src/discovery.ts`).
- The principal path uses `authn.whoAmI`.

No `satisfies IamClients` or `keyof IamClients` exists in `ts/`. **One test lists the keys:**
`ts/apps/iam-console/tests/unit/action-session.test.ts:102-110` (§ 6.2). Revision 1 of this spec
said that no key count exists. That was wrong.

Change:

- Remove the `serviceInfo` field from the `IamClients` type (`:24`).
- Remove its construction (`:61`).
- Remove `ServiceInfoService` from the import at `:16`. After the change, nothing else in the
  file uses it.
- Change the doc comment at `:51` from "The seven clients" to "The six clients".

### 6.2 Tests, doubles and comments

**`tests/integration/error-info-round-trip.test.ts:47-65`** is the only reader of
`clients.serviceInfo` (`:54`). The test is named
`'sends the correlation header from every one of the five clients'`. That name has gone stale
twice: after SMA-636 added `serviceAccounts` and after SMA-629 added `outbox`.

- Remove the `serviceInfo` call.
- Add `clients.serviceAccounts.listServiceAccounts({})` and `clients.outbox.listDeadLetters({})`.
  Both methods are read-only, and the apps already call them. `outbox-client.test.ts:51` already
  calls `listDeadLetters({})`.
- Update the `expect` list to six entries.
- Add a key-set check, so that the name cannot go stale again:
  `expect(new Set(fake.calls.map((call) => call.method.split('.')[0]))).toEqual(new Set(Object.keys(clients)))`.
  Read `fake.calls`, not `sent`: `sent` holds `string | null` values, and `strict` rejects
  `.split` on them.
  An eighth client in `IamClients` then reds this test until the test calls it.
- Rename the test to `'sends the correlation header from every client'`. The key-set check makes
  "every" true, so the name does not need a number.
- Extend the comment at `:50-52` to name the two new calls that fail. The fake registers both
  services (`testing/fake-iam.ts:66-77`), and neither has a default answer, so each call answers
  `Unimplemented`. The fake records each call before it runs a handler (`fake-iam.ts:305`),
  also when `defaults()` throws `Unimplemented` (`:296`). So the test still sees the header.

**`ts/apps/iam-console/tests/unit/action-session.test.ts:102-110`.** Rename the test from
`'returns the seven clients when the session resolves'` to `'returns the six clients when the
session resolves'`. Remove `'serviceInfo'` from the key list. Add to the comment at `:109` that
SMA-666 removed `serviceInfo`.

**Comments in `testing/`.**

| File and line | Now | Change |
|---|---|---|
| `testing/fake-iam.ts:4` | "the seven services the console calls" | The fake serves seven services. The console calls six of them. `ServiceInfoService` stays in the fake for the reason below. |
| `testing/fake-iam.ts:89` | `FakeIamMethod` has "the same keys as `IamClients`" | The keys of `IamClients`, plus `serviceInfo`. |
| `testing/fake-iam.ts:138-144` | "`IamClients.serviceInfo` has no production caller today — only a test reads it directly" | Keep the gRPC `serviceInfo.getServiceInfo` route, because the fake serves the full IAM surface and `ts/apps/iam-console/tests/integration/doubles/fake-iam.test.ts:145-146` calls it with its own `ServiceInfoService` client. Keep the rest of the comment (the HTTP override must not clobber a scripted descriptor). |
| `testing/dev-world.ts:192` | cites `fake-iam.ts:130-134` | Cite the line range of the rewritten comment. The old range is already stale. |

**Do not change:**

- `FakeIam.setServiceInfo`, the `SERVICES.serviceInfo` entry, and the default handler for
  `serviceInfo.getServiceInfo` (`fake-iam.ts:71, 287-288, 378-381`).
- The HTTP `/v1/service-info` route of the fake, and the MSW `serviceInfoHandlers` helpers.
- `tests/unit/dev-world.test.ts`. It checks the keys of a handler map, not of `IamClients`. The
  fourth capability key that SMA-633 added (`iam.authn.grants`) is not affected.
- The e2e worlds and harnesses of both apps. They call `setServiceInfo` on the fake server.

### 6.3 Verification

- `git grep -n 'serviceInfo' -- ts ':!**/generated/**'`: review each hit. After the change, the
  only hits are the fake's own route, `setServiceInfo`, the HTTP descriptor path, the MSW helpers,
  and `fake-iam.test.ts`. No hit reads `IamClients`.
- These targets pass: `paigasus-console-core-ts:typecheck`, `paigasus-console-core-ts:test`,
  `iam-console-ts:typecheck`, `iam-console-ts:test`, `gateway-console-ts:typecheck` and
  `gateway-console-ts:test`.
- `ts:fmt` and `ts:lint` pass.
- **Mutation.** Temporarily remove the `outbox.listDeadLetters` call AND its row in the ordered
  `expect` list. The key-set check must fail. Restore both. If only the call is removed, the
  ordered list fails first, and the key-set check proves nothing.

## 7. Item 5 — pin the fact, then clarify the SMA-632 and SMA-633 specs

### 7.1 The test (commit 6)

Add a unit test to the test module of
`rs/crates/services/paigasus-iam/src/application/authenticate_token.rs`, for example
`context_for_returns_the_grants_of_an_api_key_principal`. It has no Docker dependency.

- Build an `AuthenticateToken` with an `InMemoryRoleGrants` that holds one grant for the
  principal. Follow the SMA-633 grant tests in the same module (`:517-740`). The existing
  `context_for_fixture` (`:867-901`) builds an OIDC principal with an empty grant store, so it
  does not fit.
- Build an `AuthnPrincipal` with `kind: PrincipalKind::ServiceAccount` and
  `credential: Credential::ApiKey { .. }`.
- Assert that `context_for(principal)` returns that one grant, with its exact `scope_prn` and
  `role_key`.

**Proof by mutation.** Add this to `context_for`, after the grant read:
`if matches!(principal.credential, Credential::ApiKey { .. }) { role_grants.clear(); }`.
It compiles, and the new test must fail. Remove it.

This test pins the fact that SMA-633's D2 rests on. It does not change any behaviour.

### 7.2 Form of the note

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

### 7.3 SMA-632 spec (`docs/superpowers/specs/2026-09-20-sma-632-whoami-rpc-design.md`)

**After § 3.2's last paragraph (`:110-118`, which ends "§ 9.5 carries the corrected note.").**
The note must say:

- "Both credential kinds" means both kinds of bearer on `WhoAmI`. That statement is true.
  `WhoAmI` calls `context_for` for an OIDC token and for an API key
  (`adapters/grpc/authn.rs:105`, `adapters/http/authn.rs:119`), and SMA-633 fills
  `role_grants` there. The test from § 7.1 of the SMA-666 spec pins it (name the test).
- `IntrospectApiKey` returns an empty `role_grants` list. SMA-633's D2 decided this. The reasons:
  the gateway calls it on every request, through `require_iam_auth` (the model-invocation path)
  and `require_authenticated` (`paigasus-gateway/src/adapters/iam/client.rs:105`). Nothing reads
  the field. A grant-store read on the `require_iam_auth` path would turn a `role_grant` outage
  into a gateway 503 for model invocations.
- Do not say that the gateway has no `role_grant` dependency. The OIDC leg of
  `require_authenticated` already reaches the grant store through `Introspect`
  (`paigasus-gateway/src/adapters/http/auth.rs:187`, SMA-633 D6 at `:169-178`).
- This spec never changed `IntrospectApiKey` (§ 8). So the two decisions do not disagree.
- The rest of this spec is left otherwise unedited.

**After § 9.5 (item 5 of the § 9 list, `:510-517`).** A shorter note: "The two messages" are
`IntrospectResponse` (OIDC only) and `WhoAmIResponse` (both credential kinds). SMA-633 filled
both. The third message, `IntrospectApiKeyResponse`, stays empty by SMA-633 D2. Refer to the § 3.2
note for the reason.

### 7.4 SMA-633 spec (`docs/superpowers/specs/2026-09-20-sma-633-introspect-role-grants-design.md`)

**After D10's "One divergence to know about" paragraph (`:247-252`).** The note must say:

- The divergence is not real. SMA-632 § 3.2 and § 9.5 describe `WhoAmI` and `IntrospectResponse`.
  They do not describe `IntrospectApiKey`.
- D2's own paragraph at `:85-94` ("The rule is about the call site, not the credential") gives the
  same result as SMA-632: an API-key bearer gets its grants through `WhoAmI`.
- The SMA-632 spec now has a clarifying note at § 3.2 and § 9.5. Name the test from § 7.1.

### 7.5 Verification

- Each note cites file and line references that exist on the branch when the note is written.
  Check each reference with `sed -n`.
- `git diff` on each spec shows only added lines.

## 8. Testing summary

| Item | Tier | Proof |
|---|---|---|
| 1 | TS typecheck, unit, integration | grep returns no line. All targets pass. |
| 2 | Rust unit (always runs) | Two literal-value tests with exact expiry. Four mutations fail them. |
| 3 | Rust integration (Docker) | New assertion passes. A pre-provision mutation fails it. Run with `PAIGASUS_REQUIRE_DOCKER=1`. |
| 4 | TS typecheck, unit, integration | grep review. The round-trip test covers every client and has a key-set check. One mutation fails it. |
| 5 | Rust unit, docs | One new test. One mutation fails it. References checked. The diff has only added lines. |

**Affected targets.** The changed files select more than the unit tiers. Plan for these:

- `paigasus-auth-ts:test` and `paigasus-auth-ts:test-e2e` (from the two comment edits in
  `ts/packages/paigasus-auth/src/`, `ts/packages/paigasus-auth/moon.yml:40-54`). The e2e tier
  uses Docker and Playwright, has `cache: false`, and has had Keycloak flakes (SMA-652).
- `paigasus-console-core-ts:test-e2e` (from the edits in console-core `src/`). It uses Docker and
  Redis, has no skip option, and has `cache: false`.
- `iam-console-ts` and `gateway-console-ts` e2e tiers (from the `lib/auth.ts` edits). They use
  Docker.
- `paigasus-iam-rs:test`, which includes the Docker-gated suites.

Before the push, run the full gate graph from the root `CLAUDE.md` (`moon ci … --base origin/main
--include-relations`). Read the local-bash rules in the root `CLAUDE.md` before you read a gate
result. If an e2e tier fails, run it again alone and compare with unmodified `origin/main` before
you blame this change.

## 9. Out of scope

- Historical specs and plans that name the old function (D3).
- A shared Rust fixture module for OIDC identities. Each test module keeps its own fixtures.
- Any change to `IntrospectApiKey`, the gateway or the fake's `setServiceInfo`.
- A credential-kind discriminator on `WhoAmIResponse`. SMA-632 § 9.4 records that limit.
- A change to the commitlint scope list. Sven confirmed on 2026-09-23 that the workspace scopes
  stay.

## 10. Change log

**Revision 2 (2026-09-23), after the adversarial challenge.** Verdict: APPROVE WITH CHANGES. No
BLOCKER. The challenger traced § 3 on both transports and confirmed it.

| Finding | Severity | Action |
|---|---|---|
| `action-session.test.ts:102-110` lists the `IamClients` keys; § 6.1 said no key count exists | MAJOR | Folded in: § 6.1, § 6.2. |
| No test pins that an API-key bearer gets grants through `WhoAmI` | MAJOR | Folded in: D8, § 7.1, criterion 6, commit 6. |
| `cargo clippy` without `--all-targets` does not lint test code | MINOR | Folded in: § 5.3. |
| `AuthnError` and `Issuer` imports missing | MINOR | Folded in: §§ 5.1, 5.2. |
| The § 5.2 assertion message can report the wrong cause | MINOR | Folded in: bind and print the result. |
| `iam-service-info.test.ts` does not use the fake; wrong line citations | MINOR | Folded in: § 6.2. |
| `fake-iam.ts:4`, `:89` and `dev-world.ts:192` go stale | MINOR | Folded in: § 6.2 table. |
| `claims-resolver.ts:6-10` and `principal-resolver.ts:8-10` are stale in full | MINOR | Folded in: § 4.1 table. |
| New comments must describe a role, not name a downstream function | MINOR | Folded in: § 4.1. |
| The naming rationale was partly false for two factories | MINOR | Folded in: § 4.1 text. D1 is unchanged. |
| "D2's last paragraph" is the wrong citation | MINOR | Folded in: cite `:85-94`. |
| The 503 sentence must be limited to `require_iam_auth` | MINOR | Folded in: § 7.3. |
| § 8 lists too few affected targets and does not mention Docker | MINOR | Folded in: § 8, §§ 4.2, 6.3. |
| D7 does not name the commit scopes | MINOR | Folded in: § 2.1. The challenger suggested `docs(docs)`. This spec uses `docs(repo)`, because Sven chose it for commit 1 and earlier spec commits use it. |
| The round-trip test name has gone stale twice | MINOR | Folded in: key-set check, D5. |
| The exact-expiry change has no criterion and no mutation | MINOR | Folded in: criterion 2, § 5.1 mutation table. |
| Which commit holds the new test? | QUESTION | Answered: commit 6, before the notes in commit 7. |
| Must each commit pass on its own? | QUESTION | Answered in § 2.1: each commit passes the targets its own files select; the full graph runs on the head. |

No finding was rejected.
