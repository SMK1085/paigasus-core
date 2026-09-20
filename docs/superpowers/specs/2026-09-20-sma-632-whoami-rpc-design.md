# SMA-632 — an IAM `WhoAmI` RPC, so the consoles provision a principal explicitly

**Issue:** [SMA-632](https://linear.app/smaschek/issue/SMA-632/iam-whoami-rpc-so-the-console-can-provision-a-principal-explicitly)
**Date:** 2026-09-20
**Status:** revision 2, for approval. Revision 1 went through the adversarial challenge; § 11
records what changed.

---

## 1. The problem

IAM provisions a principal just in time (JIT), and seeds the bootstrap `platform_admin` grant,
only inside a bearer-enforced RPC. `AuthEnforce::call` does both
(`rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs:175-196`).

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

**Precondition, recorded because the middleware alone is not sufficient.** `resolve` provisions
only when the issuer's JIT flag is set. Under `Provisioning::Enabled` it still returns
`IdentityNotProvisioned` when `JitPolicy::allows(issuer)` is false
(`application/authenticate_token.rs:107-109`), and `allows` answers `false` for an issuer absent
from the configuration (`:43-45`). So `WhoAmI` provisions a caller from a JIT-enabled issuer, and
refuses one from any other issuer with the same error `Introspect` gives today. § 7 keeps the
console's branch for it.

## 3. Decisions

| # | Question | Decision |
|---|---|---|
| D1 | Scope | One issue, one spec, one PR: contracts, `rs/` and `ts/`. |
| D2 | Response message | A new `WhoAmIResponse`, mirroring `IntrospectResponse`'s fields, with `expires_at` optional. **Forced by buf lint — see § 3.1.** |
| D3 | RPC name | `WhoAmI`. |
| D4 | An API-key bearer | Answer it. `issuer` and `subject` stay empty. `expires_at` is absent when the key has none. |
| D5 | Membership paging | Collapse the existing duplicate into one helper. Do not add a third copy. |
| D6 | `AuthContext` | Add `kind` and `status`. Do not re-read the principal. |
| D7 | HTTP twin | `POST /v1/authn/whoami`, inside the bearer layer. **POST, not GET — see § 3.3.** |
| D8 | `role_grants` | Stays empty. SMA-633 owns it. |
| D9 | `Introspect` | Unchanged — exemption, `Provisioning::Disabled`, message, DTO and mapper all untouched. |
| D10 | `GetServiceInfo` | Unchanged. It stays bearer-enforced. |
| D11 | Capability gating | None. `WhoAmI` is always mounted, like `service_info` and `authz::decision_router`. |

### 3.1 Why D2 changed: buf lint forbids reusing `IntrospectResponse`

Revision 1 reused `IntrospectResponse` as `WhoAmI`'s response type, to avoid a second message.
**That does not pass `contracts:lint`.** `contracts/buf.yaml:8-14` sets `lint.use: [STANDARD]`
and excepts only `PACKAGE_DIRECTORY_MATCH`. STANDARD contains two rules the reuse breaks:

- `RPC_RESPONSE_STANDARD_NAME` — the response must be named `WhoAmIResponse`.
- `RPC_REQUEST_RESPONSE_UNIQUE` — a message may not serve two RPCs.

Every one of the RPCs in `iam.proto` follows the `<Rpc>Request` / `<Rpc>Response` pattern, and
the repo has already recorded why: SMA-499's spec says the `GetServiceInfoRequest` /
`GetServiceInfoResponse` wrappers "exist only to satisfy buf lint STANDARD's
`RPC_REQUEST_STANDARD_NAME` and `RPC_RESPONSE_STANDARD_NAME`. They carry no meaning."
(`docs/superpowers/specs/2026-08-14-sma-499-service-info-capability-descriptor-design.md:214-215`).
No `// buf:lint:ignore` exists anywhere in the repo, so adding the first one is a larger decision
than this issue should take.

**The forced change improves the design.** With its own message, `WhoAmI` gets its own mapper and
its own DTO, so D9 becomes literally true: `Introspect`'s message, DTO, mapper and
`debug_assert!` are all untouched, and its Oidc-only invariant keeps its guard. § 5.3 records
what revision 1 got out of this.

**The honest cost.** A second message with the same field set can drift from the first, and **no
Rust test pins them together.** Revision 2 planned one; prost's generated structs carry no
field-name reflection, so a direct comparison would mean parsing the `.proto` at test time. What
ships instead is `buf lint` plus `buf breaking`, which reject a renamed or renumbered field but
not a field added to one message and forgotten in the other. § 9.7 records the residual.

### 3.2 Why D4

`AuthEnforce::call` routes on the token prefix (`adapters/grpc/authn.rs:175-179`). A Paigasus
API key resolves through `state.api_key_auth.resolve`. So a `WhoAmI` call can carry an API-key
bearer, and the RPC answers it.

`WhoAmIResponse` carries no `key_id` and no `scope_prn`; those belong to
`IntrospectApiKeyResponse`. For an API-key bearer, `issuer` and `subject` are empty.

**`expires_at` is optional, and that is not cosmetic.** `Credential::ApiKey.expires_at` is
`Option<DateTime<Utc>>` (`rs/crates/libs/paigasus-iam-core/src/authn.rs:80-87`) — a key minted
without an expiry has `None`. `AuthnPrincipal::expires_at()` (`authn.rs:104-109`) already returns
`Option<DateTime<Utc>>` for both credential kinds, and both the mapper and the DTO use it. The
proto field is a message, so absence is already expressible on the wire.

**Known consequence.** `WhoAmIResponse` carries no discriminator, so a caller tells the two
credential kinds apart only by `issuer` and `subject` being empty. That does not block this issue;
§ 9.4 records it so SMA-633 does not discover it.

**A consequence this spec previously claimed, and withdraws.** Revision 2 said an API-key caller's
role grants must respect the key's `scope_prn`. **That is wrong, and the codebase says so.**
`ApiKey.scope_actions` and `scope_roles` are never read — `authenticate_api_key.rs:32-34` calls
them "stored, v1-UNENFORCED metadata" — and `application/api_keys.rs:7-22` (D15) states that an
API key is a bearer credential for the service account's **entire** current grant set, narrowed
only at mint time by an anti-escalation check. A scope filter at introspection time would
therefore report **less** authority than the key actually wields, which is worse than reporting
none: it would describe a key as weaker than it is. SMA-633 reports the full grant set for both
credential kinds. § 9.5 carries the corrected note.

### 3.3 Why D7 is POST, not GET

`WhoAmI` performs writes. It creates a principal row and a user row, and for a configured
bootstrap identity it seeds a `platform_admin` role grant
(`adapters/http/auth_middleware.rs:66-68`). RFC 9110 makes GET a safe method: browsers, proxies,
link prefetchers and automatic retries all issue GETs freely, and Next.js prefetches.

A GET that provisions a user would reproduce, behind a stronger promise, exactly the hidden side
effect this issue exists to remove. The route is `POST /v1/authn/whoami`, which also matches
`POST /v1/authn/introspect`'s verb.

## 4. The contract change

`contracts/proto/paigasus/iam/v1/iam.proto`:

```proto
message WhoAmIRequest {}

message WhoAmIResponse {
  string principal_prn = 1;
  string status = 2; // principal status
  string issuer = 3; // empty for an API-key bearer
  string subject = 4; // empty for an API-key bearer
  // Absent when the credential has no expiry — an API key minted without one.
  google.protobuf.Timestamp expires_at = 5;
  repeated Membership memberships = 6; // reuse tenancy message
  // Empty until SMA-633 populates it, exactly as IntrospectResponse.role_grants is today.
  repeated RoleGrantRef role_grants = 7;
}

service AuthnService {
  rpc Introspect(IntrospectRequest) returns (IntrospectResponse);
  rpc IntrospectApiKey(IntrospectApiKeyRequest) returns (IntrospectApiKeyResponse);
  // The caller's own principal. BEARER-ENFORCED: this RPC is deliberately ABSENT from
  // `is_exempt` (adapters/grpc/authn.rs), so `AuthEnforce` resolves the bearer with
  // `Provisioning::Enabled` and seeds the bootstrap platform_admin grant BEFORE the handler
  // runs. That is the whole mechanism — the handler never sees a token.
  //
  // Provisioning still obeys the issuer's JIT policy: an issuer with JIT off yields
  // `identity-not-provisioned`, the same answer Introspect gives.
  //
  // Serves both credential kinds. For a Paigasus API-key bearer, `issuer` and `subject` are
  // empty: a service account has no (issuer, subject) pair, and this response carries no
  // `key_id`/`scope_prn` (those are IntrospectApiKeyResponse's).
  rpc WhoAmI(WhoAmIRequest) returns (WhoAmIResponse);
}
```

`WhoAmIResponse` has no reserved field numbers: it is new, so it inherits none of
`IntrospectResponse`'s history (which reserves 7 for the retired `role_group_prns`). `role_grants`
therefore takes 7, not 8.

The change is additive. buf's `FILE` breaking category permits new messages and a new RPC.
`WhoAmIRequest` is empty on purpose: the bearer header is the only input.

**No `ErrorReason` registry change.** Every error `WhoAmI` can produce already has a registered
reason. `invalid-token`, `identity-not-provisioned`, `provisioning-failed`, `principal-inactive`
and `authn-unavailable` come from `AuthEnforce::call`, before the handler runs. The handler adds
only `missing-auth-context` and `internal`. Both are registered
(`contracts/proto/paigasus/common/v1/error.proto`).

**The `repo:error-code-single-site` gate.** It fires when a file spells a canonical code literal
and is not on `ci/error-registry/check.py`'s `MANIFEST` (`check.py:87`). § 5.2's new module
spells none — it maps repository failures through the existing `backend` helper. No `MANIFEST`
row is added. If implementation puts a literal code in the new file, it needs a row and a
membership test.

**Codegen.** `buf generate` writes the Rust, Python and TypeScript bindings. The proto edit and
the regenerated files go in one commit, or `repo:breaking` and the codegen-drift gate disagree.
`buf format -w` runs before the commit, or `contracts:fmt` fails without printing a diff.

## 5. The IAM service change

### 5.1 `AuthContext` gains `kind` and `status` (D6)

`WhoAmIResponse.status` reports the principal's status. `AuthContext` does not carry it today
(`adapters/auth.rs:19-22`). The middleware does have it: `resolve` returns a full
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

`kind` has no reader in this issue. It exists so the handler can rebuild a complete
`AuthnPrincipal` rather than a partial one; the mapper reads `principal_id`, `status` and
`credential` only. The field carries a comment saying so, or the next reader deletes it.

**Three construction sites change:**

| Site | Change |
|---|---|
| `adapters/grpc/authn.rs:191` | pass `principal.kind` and `principal.status` |
| `adapters/http/auth_middleware.rs:69` | the same |
| `adapters/http/authz_middleware.rs:200` | a test helper; give it the two fields |

**No reader changes, and the census matters.** There are two distinct reader mechanisms, and
revision 1 conflated them:

- Six gRPC modules each hold a private `actor_context` copy: `grpc/tenancy.rs:106`,
  `grpc/audit.rs:46`, `grpc/dead_letters.rs:60`, `grpc/authz.rs:61`, `grpc/users.rs:54`,
  `grpc/service_accounts.rs:65`.
- About fourteen HTTP handlers read `Extension(ctx): Extension<AuthContext>` instead — a
  different mechanism, not an `actor_context` copy. `http/audit.rs:118` is one of these, not one
  of the six.

Adding a field breaks neither group, because both destructure by name or read fields they
already read. That is the point of choosing an additive change; the census above is what lets a
reviewer check the claim rather than take it.

`adapters/auth.rs:15-16` documents the field set as "deliberately fixed". That comment is
updated: the set is fixed in the sense that both middlewares attach the same shape, which stays
true.

### 5.2 One membership-paging helper (D5), and how it is reached

Two use cases page memberships with byte-identical loops, each with its own
`MEMBERSHIP_PAGE_SIZE: u64 = 200`:

- `application/authenticate_token.rs:149-159` (constant at `:49`)
- `application/authenticate_api_key.rs:264-274` (constant at `:46`), whose own comment at
  `:44-45` reads "identical to".

A new module `application/principal_context.rs` holds one `pub(crate)` free function, generic
over `M: MembershipRepository`, returning `Result<Vec<MembershipRecord>, AuthnError>` — the error
type both existing callers already produce, through the same `backend` mapping. The constant
moves there. Both existing callers lose their loop and their constant.

**The composition problem revision 1 missed.** The handler cannot call this helper directly.
`AppState` exposes `memberships: MembershipSvc`, which is `MembershipService<…>` — a use case
whose `list` takes a PRN string, does not page, and returns `TenancyError`
(`application/memberships.rs:241-252`). The membership *repository* is a private field of
`AuthenticateToken` and of `AuthenticateApiKey`.

**So `AuthenticateToken` gains one method**, next to `introspect`, which already holds the
repository as its fourth type parameter (`adapters/http/mod.rs:158`):

```rust
/// The full context for an ALREADY-RESOLVED principal — the bearer-enforced path's peer of
/// `introspect`, which resolves a token first. `WhoAmI` (SMA-632) uses this: `AuthEnforce`
/// has already resolved and provisioned the caller, so re-verifying its token here would
/// repeat work and could disagree with the middleware's own answer.
pub async fn context_for(&self, principal: AuthnPrincipal) -> Result<PrincipalContext, AuthnError>
```

`introspect` becomes `self.context_for(self.resolve(token, Disabled).await?).await`. `AppState`
needs no new field: the handler calls `state.authn.context_for(principal)`, and `state.authn` is
already there.

`authenticate_api_key.rs` calls the free function directly, so its own `introspect` keeps its
shape. It is touched only to delete the duplicate loop and constant.

### 5.3 A separate mapper, and what `Introspect` keeps

Revision 1 found that `to_introspect_response` (`adapters/grpc/convert.rs:403-408`) and
`impl From<PrincipalContext> for IntrospectResponseDto` (`adapters/http/dto.rs:260-265`) both
carry `debug_assert!(false, "… only Oidc is reachable here")` on the `Credential::ApiKey` arm.
Under revision 1's shared-message design, D4 made that arm reachable, and every debug and test
build would have panicked on an API-key `WhoAmI` call.

**D2's forced change removes the problem instead of patching it.** `WhoAmI` gets its own mapper
and its own DTO, so `Introspect`'s two functions keep their assertion and keep their guarantee.
`AuthenticateToken::introspect` still resolves only through the OIDC authenticator, so the
assertion is still true of its only caller.

The new mapper `convert::to_who_am_i_response` and the new `WhoAmIResponseDto` both handle the
`ApiKey` arm honestly, and both read the expiry through `AuthnPrincipal::expires_at()`, which is
already `Option<DateTime<Utc>>`:

```rust
let (issuer, subject) = match &ctx.principal.credential {
    Credential::Oidc { issuer, subject, .. } => (issuer.as_str().to_string(), subject.clone()),
    // A service account has no (issuer, subject) pair, and this response carries no
    // key_id/scope_prn — those are IntrospectApiKeyResponse's.
    Credential::ApiKey { .. } => (String::new(), String::new()),
};
let expires_at = ctx.principal.expires_at(); // Option: an API key may have no expiry.
```

`IntrospectResponseDto` is not touched, so `POST /v1/authn/introspect`'s JSON response shape does
not change. D9 holds literally.

`WhoAmIResponseDto.expires_at` is `Option<DateTime<Utc>>`, serialized with
`skip_serializing_if = "Option::is_none"` so an absent expiry is an absent key, not a null.

### 5.4 The gRPC handler

`adapters/grpc/authn.rs`, a third method on `AuthnGrpc`:

1. Read the `AuthContext` with `actor_context(&request)`. Absent means
   `convert::missing_auth_context()`.
2. Rebuild an `AuthnPrincipal` from its four fields.
3. `self.state.authn.context_for(principal).await`, mapping the error with
   `convert::authn_status`.
4. Return `convert::to_who_am_i_response(&ctx)`.
5. Wrap in `record_grpc("Authentication", "WhoAmI", started, &result)`, the service label both
   siblings use.

`grpc/authn.rs` has no `actor_context` helper today, because neither of its RPCs is
bearer-enforced. It gets the same six-line function the other six modules have. § 8 records why
this spec does not unify the seven copies.

**`is_exempt`'s body is not touched. Its doc comment is.** The body's omission is the mechanism.
The doc at `grpc/authn.rs:120-138` enumerates the exempt set and its reasoning ("both
credential-introspection RPCs carry their token in the request body"); it gains a sentence
saying `WhoAmI` is deliberately absent, and why. § 6.3 relies on that reasoning being written
down.

### 5.5 The HTTP twin (D7)

`POST /v1/authn/whoami`, returning `WhoAmIResponseDto`, erroring through `AuthnApiError` — the
authn surface's own funnel, not the tenancy `ApiError`.

It must sit **inside** the bearer layer, which is the opposite of `POST /v1/authn/introspect`.
`app_routes` (`adapters/http/mod.rs:912-958`) merges `authn::router(...)` OUTSIDE `protected`, so
`require_bearer` never covers it. The new route merges INTO `protected`, before the `route_layer`
at line 940. Per D11 it is not behind a capability flag.

The handler reads the `AuthContext` that `auth_middleware::require_bearer` inserted, via
`Extension<AuthContext>` like every other protected HTTP handler, and drives the same
`context_for` method. A missing extension cannot happen inside `protected`; axum's default
`Extension` rejection is plain text and outside the `{"error":{code,message}}` envelope, so the
handler takes `Option<Extension<AuthContext>>` and maps `None` to `AuthnApiError`.

**Two registration sites, not one.** The route-conflict test
`protected_router_merge_has_no_path_conflicts_in_any_capability_combination`
(`adapters/http/mod.rs:1033`) reproduces the `protected` chain for all eight capability
combinations; the new route is added there too. That test does **not** reproduce the final
`Router::new().merge(protected).merge(authn_api).merge(api_key_introspect_api)` at
`mod.rs:947-950`, so a conflict between a new `/v1/authn/*` route inside `protected` and
`authn::router`'s routes outside it would go uncovered. `/v1/authn/whoami` does not collide with
`/v1/authn/introspect`, but the gap is real: § 6.1 adds one test that reproduces the outer merge.

## 6. Tests

### 6.1 Rust unit tests

| Test | Asserts |
|---|---|
| `who_am_i_is_not_exempt` | `is_exempt("/paigasus.iam.v1.AuthnService/WhoAmI")` is `false`. Direct on the mechanism. |
| the § 5.2 helper's tests | A principal with 0, 1, 200 and 201 memberships returns all of them. 200 is the page size, so 201 proves the loop runs twice. |
| *(no Rust unit test)* | D2's drift guard (§ 3.1) ships as `buf lint` plus `buf breaking`, not a Rust test — see § 9 limit 7. |
| `to_who_am_i_response_on_an_api_key` | `issuer` and `subject` are empty, and `expires_at` is the key's own expiry. |
| `to_who_am_i_response_on_an_api_key_without_expiry` | `expires_at` is absent, not the current time. |
| `who_am_i_response_dto_omits_an_absent_expiry` | The JSON has no `expires_at` key, rather than `null`. |
| `outer_router_merge_has_no_path_conflicts` | Reproduces `mod.rs:947-950`'s merge, which the existing conflict test does not cover (§ 5.5). |

### 6.2 Rust integration tests

A new `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs`, following the conventions of
`tests/grpc_service_info.rs` (real migrated Postgres, mock IdP, `grpc::router` on an ephemeral
port, quiet skip when Docker is unreachable).

| Test | Asserts |
|---|---|
| `who_am_i_without_a_bearer_is_rejected_by_enforcement` | No `authorization` metadata gives `Unauthenticated` **with `ErrorInfo.reason == "invalid-token"`**. The reason is the assertion: `missing-auth-context` is also `Unauthenticated`, so a code-only check would pass even if `WhoAmI` were exempt. |
| `who_am_i_provisions_a_new_identity` | **The issue's own claim.** A token for an identity IAM has never seen: `Introspect` first returns `PermissionDenied` / `identity-not-provisioned`; one `WhoAmI` call then succeeds and names a principal; `Introspect` then succeeds too. No `GetServiceInfo` call anywhere in the test. |
| `who_am_i_obeys_the_jit_policy` | For an issuer with JIT off, `WhoAmI` returns `identity-not-provisioned`. Pins § 2's precondition. |
| `who_am_i_returns_memberships` | After a membership is attached, `WhoAmI` reports it. |
| `who_am_i_seeds_the_bootstrap_admin` | For a configured bootstrap `(issuer, subject)`, one `WhoAmI` call is enough for the `platform_admin` grant to exist. Follows `tests/authz_bootstrap_admin.rs`. |
| `who_am_i_serves_an_api_key_bearer` | With an API key as the bearer: it succeeds, names the service account's principal, and returns empty `issuer`/`subject`. |
| `who_am_i_grpc_http_parity` | `POST /v1/authn/whoami` and the gRPC call, with the same bearer, describe the same principal. Follows `the_grpc_and_http_transports_describe_the_same_build`. |

### 6.3 The negative controls

Two tests pin the mechanism, and each fails for its own reason if somebody adds `WhoAmI` to
`is_exempt`:

- `who_am_i_is_not_exempt` fails on the predicate directly.
- `who_am_i_provisions_a_new_identity` fails because `AuthEnforce` would forward without
  inserting an `AuthContext`, so the handler answers `missing_auth_context()` and the call does
  not succeed.

`who_am_i_without_a_bearer_is_rejected_by_enforcement` is a third control **only because it
asserts the reason**. Revision 1's version of it asserted the gRPC code alone and would have
passed under an exempt `WhoAmI`.

### 6.4 TypeScript

The fake IAM server is the harness for every tier below
(`ts/packages/paigasus-console-core/testing/fake-iam.ts`). Three facts about it, measured, that
revision 1 got wrong:

- `FakeIamMethod` is derived from the proto service descriptors (`fake-iam.ts:64-88`), so
  `authn.whoAmI` appears **automatically** once codegen lands. No hand edit.
- `dispatch` already calls `provisioned.add(token)` for any method absent from `UNENFORCED`
  (`fake-iam.ts:148`, `:274-277`), so `WhoAmI` provisions in the fake **automatically** too. No
  `dispatch` entry.
- The trap: the existing `introspect()` helper keys on `request.token` (`fake-iam.ts:234-249`).
  `WhoAmIRequest` is empty, so a naive delegation reads `undefined` and throws. The `whoAmI`
  default **must key on `context.token`**, and `principalPrnFor(context.token)` must agree with
  the `Introspect` answer for the same token — otherwise the two RPCs name different principals
  for one session.

| File | Change |
|---|---|
| `console-core/testing/fake-iam.ts` | A `defaults()` case for `authn.whoAmI`, keyed on `context.token`. |
| `console-core/testing/dev-world.ts:166-178` | **Scripts `authn.introspect`** with the dev principal's PRN and its two memberships. It needs a matching `authn.whoAmI` handler, or every dev and e2e scenario loses the dev principal's scopes. Revision 1 asked only for a comment edit. |
| `console-core/tests/unit/dev-world.test.ts:48,55` | Asserts `Object.keys(handlers)` excludes `serviceInfo.getServiceInfo`; add the matching assertion for the new method. |
| `console-core/tests/integration/principal.test.ts` | Lines 40, 67, 155 (ordering) **and** 95, 106 (degradation via a throwing/hanging `getServiceInfo`), 131 (a fabricated `serviceInfo` stub), 172, 184 (blank-PRN cases), 199-204. |
| `gateway-console/tests/integration/provisioning.test.ts:41` | "calls GetServiceInfo BEFORE Introspect" becomes "makes one WhoAmI call", keeping the correlation-id assertion. |
| `iam-console/tests/e2e/login.spec.ts:32,49` | The same. |
| `gateway-console/tests/e2e/login.spec.ts:28` | The same. |
| `gateway-console/tests/e2e/call-count.spec.ts:34` | `'authn.introspect': 1` becomes `'authn.whoAmI': 1`. The total does not change: the provisioning call was never in the formula, because a warm session never made it. |
| `iam-console/tests/integration/doubles/fake-iam.test.ts:101-107` | Proves "identity-not-provisioned until the token makes a bearer-enforced call" using `info.getServiceInfo({})`. Re-point it at `whoAmI`, which is now the bearer-enforced call under test. |

A NEW test in `console-core/tests/integration/principal.test.ts` asserts the resolver makes
exactly one IAM call. That is the property this issue delivers, and nothing asserts it today.

## 7. The consoles

`ts/packages/paigasus-console-core/`:

- **`src/principal.ts`** — `introspectWithProvisioning` becomes `whoAmI`. Two calls and a
  conditional retry become one call. The `provisionFirst` option is deleted: there is no
  ordering left to express.
- **`src/principal-resolver.ts`** — the numbered steps 1 and 2 become one call. Step 3's mapping
  is unchanged, `issuer` and `subject` included: the login path always carries an OIDC bearer.
  The degrade-never-fail contract, the two log events and the broad `try` all stay.
- **Both files' parameter types narrow** from `Pick<IamClients, 'authn' | 'serviceInfo'>` to
  `Pick<IamClients, 'authn'>` (`principal.ts:29`, `principal-resolver.ts:31`). Both apps'
  `lib/auth.ts` supply these; they may pass a wider object, so the narrowing is source-compatible,
  but the change is named here because it is part of the public shape of `console-core`.
- **`src/runtime.ts:99`** — `currentPrincipal()` drops `{ provisionFirst: false }`.
- **`src/index.ts:19`** — the renamed export.
- **`src/iam-clients.ts`** — no change. `WhoAmI` is on `AuthnService`, so it is
  `clients.authn.whoAmI({})`. The `serviceInfo` client stays, but not because a probe still uses
  it: the capability-discovery probe uses `fetch` (`src/discovery.ts`,
  `paigasus-discovery/src/probe.ts`), over HTTP, not the Connect client. `IamClients.serviceInfo`
  has no production caller after this change and is read by tests only. Keeping the field is
  harmless and out of scope for this issue; removing it belongs in a follow-up.

**`identity-not-provisioned` does NOT disappear from the console.** Revision 1 deleted the
branch. Per § 2, `WhoAmI` still returns that error for an issuer with JIT off. The live path in
`principal.ts` keeps a branch for it — it can no longer retry, so it reports the error — and the
login path in `principal-resolver.ts` already covers it through degrade-never-fail.
`ts/packages/paigasus-sdk/src/errors/presentation.ts:47` keeps its `from-transport` mapping, and
`tests/presentation.test.ts:60` is unchanged.

The file-head comments in `principal.ts` and `principal-resolver.ts` explain the `GetServiceInfo`
dance with file-and-line references into `rs/`. They are rewritten, not deleted: the new text
says that `WhoAmI` is bearer-enforced and therefore provisions, and points at `is_exempt`.

`ts/apps/iam-console/lib/auth.ts:15` and `ts/apps/gateway-console/lib/auth.ts:15` name the two
calls in a comment about correlation ids. Both are corrected.

`ts/packages/paigasus-sdk/src/iam.ts:8` says the console calls `GetServiceInfo` as its
provisioning call. Corrected. `ts/packages/paigasus-sdk/tests/iam-service-info.test.ts:4` repeats
the claim in a comment; the test itself only checks the generated client and does not change.

## 8. Out of scope

- **`role_grants` (D8).** It stays empty. SMA-633 owns it, and now owns it in two messages
  rather than one — see § 9.
- **`Introspect` (D9).** Unchanged in every respect, including its `debug_assert!` and its DTO.
- **`GetServiceInfo` (D10).** It stays bearer-enforced. It stops being load-bearing for
  provisioning; it does not change. Making it exempt is a separate question with its own
  security argument.
- **The six copies of `actor_context`.** § 5.4 adds a seventh. They are duplicated across a
  module boundary the crate keeps on purpose. Unlike § 5.2's loop, this issue is not otherwise
  writing new paging code, so extending the count is cheaper than the refactor.
- **`IntrospectApiKey`.** Unchanged.
- **A rate limit on provisioning.** `WhoAmI` creates a principal row on first call for any valid
  token from a JIT-enabled issuer. That is unchanged from `GetServiceInfo` today, and unbounded
  principal creation is the existing accepted posture. Making the endpoint's purpose explicit
  invites the question; answering it is a separate issue.

## 9. Recorded limits and inherited work

1. **`status` cannot be proven to vary.** `PrincipalStatus` has only `Active` in M2, and
   `resolve` rejects anything else (`authenticate_token.rs:127-129`). No test asserts a
   non-active status, because none can be produced.
2. **The TypeScript call-count evidence is against a fake.** `call-count.spec.ts` says so in its
   own header. This issue removes one call per cold principal resolution. It does not measure
   latency against a real IAM.
3. **`WhoAmI` reports the bearer's OWN memberships only.** It is not a lookup of another
   principal, and it checks no Cedar action — like `GetServiceInfo`, and for the same reason: a
   caller asking about itself needs no authorization decision. A console that needs another
   principal's memberships uses `TenancyService.ListMemberships`, which is Cedar-checked.
4. **`WhoAmIResponse` carries no credential discriminator** (§ 3.2). A caller distinguishes an
   API-key answer only by empty `issuer`/`subject`.
5. **SMA-633 inherits two messages, and no scope problem.** It must fill `role_grants` in both
   `IntrospectResponse` and `WhoAmIResponse`. It must **not** filter an API-key caller's grants by
   the key's `scope_prn`: a key is a bearer credential for the service account's entire current
   grant set (`application/api_keys.rs:7-22`, D15), and the scope fields are unread v1 metadata
   (`authenticate_api_key.rs:32-34`). Filtering would understate the authority the key wields.
   Revision 2 of this spec claimed the opposite; § 3.2 records the withdrawal and the evidence.
   Because both RPCs read the same `PrincipalContext.role_grants` field, SMA-633 fills both at
   once with no further change here.
6. **One PR is large** (D1). It spans `contracts/`, `rs/` and `ts/`, and the challenge widened it
   with a second proto message and a second mapper. The codegen-drift gate wants the proto and
   its bindings in one commit, and an RPC with no caller proves nothing, so the split stays
   rejected. The review surface is the cost.
7. **D2's drift guard is `buf lint` plus `buf breaking`, not a Rust test.** § 6.1 named a test,
   `who_am_i_response_mirrors_introspect_response`, that was never written. prost's generated
   structs carry no field-name reflection, so a direct field-by-field comparison of
   `IntrospectResponse` and `WhoAmIResponse` at test time would need to parse the `.proto` file
   itself. The guard was deliberately replaced with `buf lint` and `buf breaking`, which run in
   `:lint`. **Residual:** a field added to one message and not the other goes uncaught until
   someone reads both messages side by side. Neither `buf lint` nor `buf breaking` compares one
   message's shape against another's.

## 10. Verification before the PR

The full gate graph, per the root `CLAUDE.md` — this change adds proto messages, so `:breaking`,
`:typecheck` and the codegen-drift gate all have something to say about it:

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

`contracts:lint` is the gate that killed revision 1's D2. It runs inside `:lint`.

Two host facts from `CLAUDE.md` apply to the local run. No single local bash runs every gate, so
the gates that need bash 4+ are re-run directly afterwards. `paigasus-kernel-ts:build` can fail
`moon ci` in GitHub Actions with a `.napi-stage-<random>` path in a `cargo metadata` error; that
is a CI concurrency flake, confirmed by the exact error text, not a defect in this diff.

The IAM integration tests need Docker. Without it they skip quietly, so a green `moon ci` alone
does not prove § 6.2 ran. The PR states whether Docker was reachable.

## 11. What the adversarial challenge changed

Revision 1 was challenged on 2026-09-20. Three findings were blockers, and each was verified
against the code before being accepted.

| Finding | Verified | Change |
|---|---|---|
| BLOCKER: reusing `IntrospectResponse` fails buf lint STANDARD | `contracts/buf.yaml:8-14` excepts only `PACKAGE_DIRECTORY_MATCH` | D2 reversed. § 3.1, § 4 rewritten. Cascades into § 5.3 — for the better. |
| BLOCKER: `Credential::ApiKey.expires_at` is an `Option` | `paigasus-iam-core/src/authn.rs:80-87` | § 3.2, § 4, § 5.3. Uses `AuthnPrincipal::expires_at()`. Revision 1's snippet did not compile. |
| BLOCKER: `AppState` exposes no membership repository | `adapters/http/mod.rs:172`, `:97`, `application/memberships.rs:241-252` | § 5.2 now names the wiring: `AuthenticateToken::context_for`. No `AppState` change. |
| MAJOR: `JitPolicy` gates provisioning and was never mentioned | `authenticate_token.rs:107-109`, `:43-45` | § 2 precondition, § 4 doc, § 6.2 test, § 7 keeps the console branch. |
| MAJOR: `GET /v1/authn/whoami` performs writes | RFC 9110; `auth_middleware.rs:66-68` | D7 is POST. § 3.3. |
| MAJOR: `who_am_i_requires_a_bearer` was vacuous | `convert.rs:90-91` — both paths are `Unauthenticated` | § 6.2 asserts the reason. § 6.3 says why. |
| MAJOR: `dev-world.ts` scripts `authn.introspect` | `dev-world.ts:166-175` | § 6.4 adds the handler. A comment edit was not enough. |
| MAJOR: the fake-IAM change was described wrongly | `fake-iam.ts:64-88`, `:148`, `:234-249` | § 6.4 rewritten. Two named edits do not exist; the `request.token` trap does. |
| MAJOR: the TS blast radius was incomplete | grep | § 6.4 and § 7 extended by six sites. |
| MAJOR: the `actor_context` census was wrong | six copies, not seven; `http/audit.rs` uses `Extension` | § 5.1 corrected and split by mechanism. |
| MAJOR: D2 left no credential discriminator | `iam.proto` | § 3.2 and § 9.4 record it; § 9.5 hands the rest to SMA-633. |
| MINOR ×7 | — | Line references corrected; `AuthContext` and `is_exempt` doc edits named; `kind`'s lack of a reader stated; the HTTP error type fixed; the outer-merge test gap covered in § 5.5 and § 6.1. |

**Not accepted:** nothing. Every finding was either folded in or recorded as a limit. The
challenge's own "what's good" list confirmed § 5.2's duplication claim and
`who_am_i_provisions_a_new_identity`'s non-vacuity; both survive unchanged in substance.
