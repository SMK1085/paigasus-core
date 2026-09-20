# SMA-632 — an IAM `WhoAmI` RPC, so the consoles provision a principal explicitly

**Issue:** [SMA-632](https://linear.app/smaschek/issue/SMA-632/iam-whoami-rpc-so-the-console-can-provision-a-principal-explicitly)
**Date:** 2026-09-20
**Status:** draft, for approval

---

## 1. The problem

IAM provisions a principal just in time (JIT), and seeds the bootstrap `platform_admin` grant,
only inside a bearer-enforced RPC. `AuthEnforce::call` does both
(`rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs:180-196`).

`AuthnService.Introspect` is exempt from bearer enforcement
(`adapters/grpc/authn.rs:139-141`) and resolves with `Provisioning::Disabled`
(`application/authenticate_token.rs:99-141`, decision D10 of SMA-444). For an identity that
IAM has never seen, it returns `PermissionDenied` with the reason `identity-not-provisioned`
(`adapters/grpc/convert.rs:139-154`).

The consoles therefore call `ServiceInfoService.GetServiceInfo` first. They do not want the
service descriptor. They call it because it is bearer-enforced and checks no Cedar action
(`adapters/grpc/service_info.rs`). The call site says so
(`ts/packages/paigasus-console-core/src/principal.ts:6-12`).

Provisioning depends on a side effect of an unrelated RPC. Nothing records that dependency in
IAM. A person who moved `GetServiceInfo` out of the bearer layer would break console login, and
no IAM test would fail.

## 2. The outcome

`AuthnService` gets a third RPC, `WhoAmI`. It is bearer-enforced. The middleware provisions the
caller before the handler runs. The handler returns the caller's principal PRN, status,
credential facts and memberships.

The consoles replace `GetServiceInfo` + `Introspect` (and a conditional retry) with one call.

## 3. Decisions

| # | Question | Decision |
|---|---|---|
| D1 | Scope | One issue, one spec, one PR: contracts, `rs/` and `ts/`. |
| D2 | Response message | Reuse `IntrospectResponse`. No new response message. |
| D3 | RPC name | `WhoAmI`. HTTP twin `GET /v1/authn/whoami`. |
| D4 | An API-key bearer | Answer it. `issuer` and `subject` stay empty strings. |
| D5 | Membership paging | Collapse the existing duplicate into one helper. Do not add a third copy. |
| D6 | `AuthContext` | Add `kind` and `status`. Do not re-read the principal. |
| D7 | HTTP twin | Yes, inside the bearer layer. |
| D8 | `role_grants` | Stays empty. SMA-633 owns it. |
| D9 | `Introspect` | Unchanged. It keeps the exemption and `Provisioning::Disabled`. |
| D10 | `GetServiceInfo` | Unchanged. It stays bearer-enforced. |

### 3.1 Why D2

`WhoAmI` answers the same question as `Introspect` about a different subject. A second message
with the same seven fields would need a second mapper in `convert.rs`, a second DTO in
`dto.rs`, and a second mapping in the console. SMA-633 would have to fill both.

One shape costs one coupling: the two RPCs cannot diverge later without a new message. Nothing
in this issue asks them to diverge.

### 3.2 Why D4

`AuthEnforce::call` routes on the token prefix (`adapters/grpc/authn.rs:178-181`). A Paigasus
API key resolves through `state.api_key_auth.resolve`. So a `WhoAmI` call can carry an API-key
bearer. `IntrospectResponse` has no `key_id` and no `scope_prn` field; those belong to
`IntrospectApiKeyResponse`.

The RPC answers such a call. It fills `principal_prn`, `status`, `expires_at` and `memberships`.
It leaves `issuer` and `subject` empty. A service account has no `(issuer, subject)` pair.

**This decision makes reachable code that two `debug_assert!(false)` calls declare unreachable.**
See § 5.3. That is the one non-obvious consequence of D4, and § 5.3 is where the work lands.

## 4. The contract change

`contracts/proto/paigasus/iam/v1/iam.proto`:

```proto
message WhoAmIRequest {}

service AuthnService {
  rpc Introspect(IntrospectRequest) returns (IntrospectResponse);
  rpc IntrospectApiKey(IntrospectApiKeyRequest) returns (IntrospectApiKeyResponse);
  // The caller's own principal. BEARER-ENFORCED: this RPC is deliberately ABSENT from
  // `is_exempt` (adapters/grpc/authn.rs), so `AuthEnforce` resolves the bearer with
  // `Provisioning::Enabled` and seeds the bootstrap platform_admin grant BEFORE the handler
  // runs. That is the whole mechanism — the handler never sees a token.
  //
  // Serves both credential kinds. For a Paigasus API-key bearer, `issuer` and `subject` are
  // empty: a service account has no (issuer, subject) pair, and this response carries no
  // `key_id`/`scope_prn` (those are IntrospectApiKeyResponse's).
  rpc WhoAmI(WhoAmIRequest) returns (IntrospectResponse);
}
```

The change is additive. buf's `FILE` breaking category permits a new message and a new RPC.
`WhoAmIRequest` is empty on purpose: the bearer header is the only input.

**No `ErrorReason` registry change.** Every error `WhoAmI` can produce already has a registered
reason. `invalid-token`, `identity-not-provisioned`, `provisioning-failed`, `principal-inactive`
and `authn-unavailable` come from `AuthEnforce::call`, before the handler runs. The handler adds
only `missing-auth-context` and `internal`. Both are registered
(`contracts/proto/paigasus/common/v1/error.proto`).

**Codegen.** `buf generate` writes the Rust, Python and TypeScript bindings. The proto edit and
the regenerated files go in one commit, or `repo:breaking` and the codegen-drift gate disagree.
`buf format -w` runs before the commit, or `contracts:fmt` fails without printing a diff.

## 5. The IAM service change

### 5.1 `AuthContext` gains `kind` and `status` (D6)

`IntrospectResponse.status` reports the principal's status. `AuthContext` does not carry it
today (`adapters/auth.rs:19-22`). The middleware does have it: `resolve` returns a full
`AuthnPrincipal { principal_id, kind, status, credential }`
(`application/authenticate_token.rs:132-145`), and `AuthEnforce::call` keeps two of the four
fields (`adapters/grpc/authn.rs:191-194`).

```rust
pub struct AuthContext {
    pub principal_id: PrincipalId,
    pub kind: PrincipalKind,
    pub status: PrincipalStatus,
    pub credential: Credential,
}
```

The change is additive, so the seven `actor_context` readers need no edit (`grpc/users.rs`,
`grpc/audit.rs`, `grpc/authz.rs`, `grpc/tenancy.rs`, `grpc/service_accounts.rs`,
`grpc/dead_letters.rs`, `http/audit.rs`). Three construction sites change:

| Site | Change |
|---|---|
| `adapters/grpc/authn.rs:191` | pass `principal.kind` and `principal.status` |
| `adapters/http/auth_middleware.rs:69` | the same |
| `adapters/http/authz_middleware.rs:200` | a test helper; give it the two fields |

The rejected alternative is a second `find_principal` query in the handler, for data the
middleware read one moment earlier and threw away.

**Note on `status`.** `resolve` rejects a principal that is not `Active`
(`authenticate_token.rs:125-130`), and `PrincipalStatus` has only `Active` in M2. So `status`
reads `"active"` on every successful call today. The field is not vacuous — it becomes real when
a later milestone adds suspend or disable — but no test can currently prove it varies. § 7 records
this limit rather than writing a test that cannot fail.

### 5.2 One membership-paging helper (D5)

Two use cases page memberships with byte-identical loops, each with its own
`MEMBERSHIP_PAGE_SIZE: u64 = 200`:

- `application/authenticate_token.rs:146-166`
- `application/authenticate_api_key.rs:261-275` — whose own comment reads "identical to".

`WhoAmI` needs the same loop. Without this decision it becomes the third copy, and D13's comment
("this is the only entry point that fetches memberships") becomes wrong in a third place.

One helper takes the membership repository and a `PrincipalId` and returns every row. Both
existing callers and the new `WhoAmI` path call it. The constant lives with the helper.

This touches `authenticate_api_key.rs`, which the issue does not name. That is deliberate: the
alternative is to knowingly write a third copy of a loop whose own comment says it is a
duplicate.

### 5.3 The `debug_assert!(false)` calls must go (consequence of D4)

Two places convert a `PrincipalContext` into the `IntrospectResponse` shape, and both assert that
the `Credential::ApiKey` arm is unreachable:

- `adapters/grpc/convert.rs:399-419` — `to_introspect_response`
- `adapters/http/dto.rs:255-270` — `impl From<PrincipalContext> for IntrospectResponseDto`

Both read:

```rust
Credential::ApiKey { .. } => {
    debug_assert!(false, "token introspection resolved an ApiKey credential; only Oidc is reachable here");
    (String::new(), String::new(), Utc::now())
}
```

The assertion was correct while `Introspect` was the only caller. D4 gives both functions a
second caller that CAN carry an API-key credential. In a debug build — which is every test build
— an API-key `WhoAmI` call would panic inside the mapper.

Both arms change to:

```rust
// WhoAmI (SMA-632) makes this arm reachable: AuthEnforce accepts an API-key bearer, and this
// response carries no key_id/scope_prn, so issuer and subject stay empty.
Credential::ApiKey { expires_at, .. } => (String::new(), String::new(), *expires_at),
```

This also removes a fabrication. The old arm returned `Utc::now()` as the expiry. The `ApiKey`
variant carries a real `expires_at` (`paigasus-iam-core/src/authn.rs`), so the new arm reports it.

The assertion guarded a property of one caller from inside a shared function. Deleting it does
not lose a guarantee about `Introspect`: `AuthenticateToken::introspect` still resolves only
through the OIDC authenticator, and § 6 adds a test that says so directly, at the level where the
property actually holds.

### 5.4 The gRPC handler

`adapters/grpc/authn.rs`, a third method on `AuthnGrpc`:

1. Read the `AuthContext` with `actor_context(&request)`. Absent means
   `convert::missing_auth_context()`.
2. Rebuild an `AuthnPrincipal` from its four fields.
3. Load memberships with the § 5.2 helper.
4. Build `PrincipalContext { principal, memberships, role_grants: Vec::new() }`.
5. Return `convert::to_introspect_response(&ctx)`.
6. Wrap in `record_grpc("Authentication", "WhoAmI", started, &result)`, as both siblings do.

`grpc/authn.rs` currently has no `actor_context` helper, because neither of its RPCs is
bearer-enforced. It gets the same six-line function the other six modules have. § 8 records why
this spec does not unify those seven copies.

`is_exempt` is **not** touched. That omission is the mechanism. § 6 pins it with a test.

### 5.5 The HTTP twin (D7)

`GET /v1/authn/whoami`, returning `IntrospectResponseDto`.

It must sit **inside** the bearer layer, which is the opposite of `POST /v1/authn/introspect`.
`app_routes` (`adapters/http/mod.rs:912-958`) merges `authn::router(...)` OUTSIDE `protected`,
so `require_bearer` never covers it. The new route merges INTO `protected`, before the
`route_layer` at line 940.

`authn.rs` gains a second router function for it, kept separate from `router(body_limit)` so the
two postures cannot be confused. The handler reads the `AuthContext` that
`auth_middleware::require_bearer` inserted and drives the same § 5.2 helper.

The route-conflict test at `adapters/http/mod.rs:1022` reproduces `app_routes`'s merge chain for
all eight capability combinations. The new route is added there too, or the test stops
reproducing the chain it claims to reproduce.

## 6. Tests

### 6.1 Rust unit tests

| Test | Asserts |
|---|---|
| `who_am_i_is_not_exempt` | `is_exempt("/paigasus.iam.v1.AuthnService/WhoAmI")` is `false`. Direct on the mechanism. |
| the § 5.2 helper's tests | A principal with 0, 1, 200 and 201 memberships returns all of them. 200 is the page size, so 201 proves the loop runs twice. |
| `to_introspect_response` on an API-key credential | `issuer` and `subject` are empty, and `expires_at` is the key's own expiry, not the current time. Fails on the old `Utc::now()` arm. |
| `IntrospectResponseDto` on an API-key credential | The same, on the HTTP side. |
| `introspect_resolves_only_oidc` | Replaces the deleted `debug_assert!`, at the level where the property holds. |

### 6.2 Rust integration tests

A new `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs`, following the conventions of
`tests/grpc_service_info.rs` (real migrated Postgres, mock IdP, `grpc::router` on an ephemeral
port, quiet skip when Docker is unreachable).

| Test | Asserts |
|---|---|
| `who_am_i_requires_a_bearer` | No `authorization` metadata gives `Unauthenticated`. Mirrors `get_service_info_requires_a_bearer`. |
| `who_am_i_provisions_a_new_identity` | **The issue's own claim.** A token for an identity IAM has never seen: `Introspect` first returns `PermissionDenied` / `identity-not-provisioned`; one `WhoAmI` call then succeeds and names a principal; `Introspect` then succeeds too. No `GetServiceInfo` call anywhere in the test. |
| `who_am_i_returns_memberships` | After a membership is attached, `WhoAmI` reports it. |
| `who_am_i_seeds_the_bootstrap_admin` | For a configured bootstrap `(issuer, subject)`, one `WhoAmI` call is enough for the `platform_admin` grant to exist. Follows `tests/authz_bootstrap_admin.rs`. |
| `who_am_i_serves_an_api_key_bearer` | With an API key as the bearer: it succeeds, names the service account's principal, and returns empty `issuer`/`subject`. This is the test that would panic without § 5.3. |
| `who_am_i_grpc_http_parity` | `GET /v1/authn/whoami` and the gRPC call, with the same bearer, describe the same principal. Follows `the_grpc_and_http_transports_describe_the_same_build`. |

### 6.3 A negative control for the mechanism

`who_am_i_provisions_a_new_identity` is the control for the whole issue. If somebody later adds
`WhoAmI` to `is_exempt`, that test fails, and so does `who_am_i_is_not_exempt`. Before this
issue, nothing in IAM failed when provisioning broke.

### 6.4 TypeScript

The fake IAM server is the harness for every tier below
(`ts/packages/paigasus-console-core/testing/fake-iam.ts`). It gains an `authn.whoAmI` entry that
marks the token provisioned — the role `serviceInfo.getServiceInfo` plays there today — and then
answers with the `authn.introspect` body.

| File | Change |
|---|---|
| `console-core/testing/fake-iam.ts` | The new method in `dispatch`, `defaults` and `FakeIamMethod`. |
| `console-core/testing/dev-world.ts:178` | The comment names `serviceInfo.getServiceInfo` as deliberately unscripted; re-state it for the new method. |
| `console-core/tests/integration/principal.test.ts` | The ordering assertions at lines 40, 67 and 155 become one call. The `identity-not-provisioned` retry case goes away. A NEW test asserts the resolver makes exactly one IAM call. |
| `gateway-console/tests/integration/provisioning.test.ts:41` | "calls GetServiceInfo BEFORE Introspect" becomes "makes one WhoAmI call", keeping the correlation-id assertion. |
| `iam-console/tests/e2e/login.spec.ts:32,49` | The same. |
| `gateway-console/tests/e2e/login.spec.ts:28` | The same. |
| `gateway-console/tests/e2e/call-count.spec.ts:33` | `'authn.introspect': 1` becomes `'authn.whoAmI': 1`. The total does not change: the provisioning call was never in the formula, because a warm session never made it. |

## 7. The consoles

`ts/packages/paigasus-console-core/`:

- **`src/principal.ts`** — `introspectWithProvisioning` becomes `whoAmI`. Two calls and a
  conditional retry become one call. The `provisionFirst` option is deleted: there is no
  ordering left to express. The `ErrorReason.IDENTITY_NOT_PROVISIONED` branch goes with it.
- **`src/principal-resolver.ts`** — the numbered steps 1 and 2 become one call. Steps 3's
  mapping is unchanged, `issuer` and `subject` included: the login path always carries an OIDC
  bearer. The degrade-never-fail contract, the two log events and the broad `try` all stay.
- **`src/runtime.ts:99`** — `currentPrincipal()` drops `{ provisionFirst: false }`.
- **`src/index.ts:19`** — the renamed export.
- **`src/iam-clients.ts`** — no change. `WhoAmI` is on `AuthnService`, so it is
  `clients.authn.whoAmI({})`. The `serviceInfo` client stays: the capability-discovery probe
  still uses it, over HTTP.

The file-head comments in `principal.ts` and `principal-resolver.ts` explain the
`GetServiceInfo` dance with file-and-line references into `rs/`. They are rewritten, not
deleted: the new text says that `WhoAmI` is bearer-enforced and therefore provisions, and points
at `is_exempt`.

`ts/apps/iam-console/lib/auth.ts:15` and `ts/apps/gateway-console/lib/auth.ts:15` name the two
calls in a comment about correlation ids. Both are corrected.

`ts/packages/paigasus-sdk/src/iam.ts:8` says the console calls `GetServiceInfo` as its
provisioning call. Corrected. `ts/packages/paigasus-sdk/tests/iam-service-info.test.ts:4` repeats
the claim in a comment; the test itself only checks the generated client and does not change.

## 8. Out of scope

- **`role_grants` (D8).** It stays empty. SMA-633 owns it. Because of D2 it fills both RPCs at
  once.
- **`Introspect` (D9).** It keeps its exemption and its `Provisioning::Disabled`. D10 of SMA-444
  is a deliberate property of a service-to-service introspection endpoint, and this issue does
  not disagree with it.
- **`GetServiceInfo` (D10).** It stays bearer-enforced. It stops being load-bearing for
  provisioning; it does not change. Making it exempt is a separate question with its own
  security argument.
- **The seven copies of `actor_context`.** § 5.4 adds an eighth. They are duplicated across a
  module boundary the crate keeps on purpose. Unifying them is a wider refactor than this issue,
  and unlike § 5.2's loop it is not a duplicate this issue would otherwise extend by writing new
  paging code.
- **`IntrospectApiKey`.** Unchanged.

## 9. Recorded limits

1. **`status` cannot be proven to vary** (§ 5.1). `PrincipalStatus` has only `Active` in M2, and
   `resolve` rejects anything else. No test asserts a non-active status, because none can be
   produced.
2. **The TypeScript call-count evidence is against a fake.** `call-count.spec.ts` says so in its
   own header. This issue removes one call per cold principal resolution. It does not measure
   latency against a real IAM.
3. **`WhoAmI` reports the bearer's OWN memberships only.** It is not a lookup of another
   principal. A console that needs another principal's memberships uses
   `TenancyService.ListMemberships`, which is Cedar-checked. `WhoAmI` checks no Cedar action —
   like `GetServiceInfo`, and for the same reason: a caller asking about itself needs no
   authorization decision.
4. **One PR is large** (D1). It spans `contracts/`, `rs/` and `ts/`. The codegen-drift gate wants
   the proto and its bindings in one commit, and an RPC with no caller proves nothing, so the
   split was rejected. The review surface is the cost.

## 10. Verification before the PR

The full gate graph, per the root `CLAUDE.md` — this change adds a proto message, so
`:breaking`, `:typecheck` and the codegen-drift gate all have something to say about it:

```
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site
  :http-extractor-envelope :input-liveness :promtool :observability-drift
  :nats-permissions :release-parity :release-parity-py :release-parity-ts
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci
  :next-public-free :test-e2e
  --base origin/main --include-relations
```

Two host facts from `CLAUDE.md` apply to the local run. No single local bash runs every gate, so
the gates that need bash 4+ are re-run directly afterwards. `paigasus-kernel-ts:build` can fail
`moon ci` in GitHub Actions with a `.napi-stage-<random>` path in a `cargo metadata` error; that
is a CI concurrency flake, confirmed by the exact error text, not a defect in this diff.

The IAM integration tests need Docker. Without it they skip quietly, so a green `moon ci` alone
does not prove § 6.2 ran. The PR states whether Docker was reachable.
