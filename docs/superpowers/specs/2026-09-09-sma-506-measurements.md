# SMA-506 measurements

Behavioural claims backing the `@paigasus/auth` implementation, measured against the
installed toolchain rather than assumed. See
`docs/superpowers/specs/2026-09-09-sma-506-auth-design.md` § 20 for the full list this
package owes.

## M2 — node-redis v6 API

Measured against the pinned `redis@6.2.1` (`ts/pnpm-workspace.yaml` catalog), by reading
the installed type declarations directly rather than from memory or v5-era examples.

### `SET` with `NX` and `PX`

File: `ts/node_modules/.pnpm/@redis+client@6.2.1/node_modules/@redis/client/dist/lib/commands/SET.d.ts`

```ts
export interface SetOptions {
    expiration?: {
        type: 'EX' | 'PX' | 'EXAT' | 'PXAT';
        value: number;
    } | { type: 'KEEPTTL'; } | 'KEEPTTL';
    /** @deprecated Use `expiration` { type: 'EX', value: number } instead */
    EX?: number;
    /** @deprecated Use `expiration` { type: 'PX', value: number } instead */
    PX?: number;
    condition?: 'NX' | 'XX' | 'IFEQ' | 'IFNE' | 'IFDEQ' | 'IFDNE';
    matchValue?: RedisArgument;
    /** @deprecated Use `{ condition: 'NX' }` instead. */
    NX?: boolean;
    /** @deprecated Use `{ condition: 'XX' }` instead. */
    XX?: boolean;
    GET?: boolean;
}
```

The v6 shape is `{ condition: 'NX', expiration: { type: 'PX', value: number } }`. The
flat `NX`/`PX` boolean/number fields still exist but are marked `@deprecated` in the
type itself — the adapter uses the non-deprecated `condition`/`expiration` shape.

`client.set(key, value, options)` return type (from `SET.d.ts`):
`SimpleStringReply<'OK'> | BlobStringReply | NullReply`, which resolves under the
default (empty) `TYPE_MAPPING` to `'OK' | string | null`. A failed `NX` set returns
`null`; a successful one returns `'OK'`.

### `GETDEL`

File: `ts/node_modules/.pnpm/@redis+client@6.2.1/node_modules/@redis/client/dist/lib/commands/GETDEL.d.ts`

```ts
declare const _default: {
    readonly parseCommand: (this: void, parser: CommandParser, key: RedisArgument) => void;
    readonly transformReply: () => BlobStringReply | NullReply;
};
```

`GETDEL` **is** exposed, as both `GETDEL` and camelCase `getDel` (confirmed at
`ts/node_modules/.pnpm/@redis+client@6.2.1/node_modules/@redis/client/dist/lib/commands/index.d.ts:3138,3146`).
It is atomic get-and-delete natively, no script required.

**Decision: `takeTransaction` still uses the `TAKE_TXN` Lua script, not `getDel`.**
The task-4 brief's own reasoning is followed: all three primitives that need
atomicity beyond a single command (`SET_CAS`, `UNLOCK`, `TAKE_TXN`) go through one
code path (`#evalGuarded`) and therefore one error-handling and one key-building
path, rather than `takeTransaction` alone taking a different route through
`getDel`. `getDel`'s availability was the thing to verify before choosing, and it
is available — the choice not to use it is deliberate, not a fallback.

### `eval` / `evalSha`

File: `ts/node_modules/.pnpm/@redis+client@6.2.1/node_modules/@redis/client/dist/lib/commands/EVAL.d.ts`

```ts
export interface EvalOptions {
    keys?: Array<RedisArgument>;
    arguments?: Array<RedisArgument>;
}
declare const _default: {
    readonly parseCommand: (this: void, parser: CommandParser, script: RedisArgument, options?: EvalOptions) => void;
    readonly transformReply: () => ReplyUnion;
};
```

`client.eval(script, { keys: [...], arguments: [...] })` — KEYS and ARGV are both
passed via one options object, not as separate positional arrays and not as a
flat variadic list. `evalSha` (`EVALSHA.d.ts`) takes the identical `EvalOptions`
shape, with a sha1 in place of the script body. The adapter uses plain `eval`
(not `evalSha`) since there is no script-caching requirement here and it avoids a
`NOSCRIPT` failure mode entirely.

The reply type (`ReplyUnion`) is a large structural union of RESP-typed markers
(`NumberReply`, `BlobStringReply`, `NullReply`, ...) that resolve under the
client's default (empty) `TYPE_MAPPING` to plain JS primitives (`number`,
`string`, `null`, ...) per `ReplyWithTypeMapping` in
`.../dist/lib/RESP/types.d.ts:70`. The adapter asserts the narrower type it
expects from each script with `as` (`as number` for `SET_CAS`/`UNLOCK`, which
`RETURN 0/1` and `DEL`'s integer reply; `as string | null` for `TAKE_TXN`, which
returns the stored value or a nil reply).

### `createClient` reconnection / offline-queueing options

File: `ts/node_modules/.pnpm/@redis+client@6.2.1/node_modules/@redis/client/dist/lib/client/index.d.ts`
and `.../dist/lib/client/socket.d.ts`.

- `disableOfflineQueue?: boolean` — top-level `RedisClientOptions` field (not
  under `socket`). "When `true`, commands are rejected when the client is
  reconnecting. When `false`, commands are queued for execution after
  reconnection." Default is `false`.
- `socket.reconnectStrategy?: false | number | ((retries: number, cause: Error) => false | Error | number)`
  — `false` or a returned `Error` both stop reconnection; a returned `number` is
  a delay in ms before the next attempt.
- `socket.connectTimeout?: number` — "Connection timeout (in milliseconds)".
- `commandOptions?: CommandOptions` at the top level of `RedisClientOptions`
  applies as the *default* for every command issued through the client;
  `CommandOptions.timeout?: number` is "Timeout for the command in
  milliseconds" (`.../dist/lib/client/commands-queue.d.ts:6-17`). This is what
  the adapter uses to honour `commandTimeoutMs` — there is no separate
  per-call timeout argument on `get`/`set`/`eval`/etc.

Error classes thrown on failure (`.../dist/lib/errors.d.ts`): `TimeoutError`,
`ConnectionTimeoutError`, `SocketTimeoutError`, `ClientClosedError`,
`ClientOfflineError`, `SocketClosedUnexpectedlyError`, `ReconnectStrategyError`.
The adapter does not discriminate between these — every error raised by a
redis-store method is caught and replaced with a fixed-message
`SessionStoreUnavailable`, never re-thrown and never carrying the original error
(as a `cause` or otherwise), because several of node-redis's own error messages
embed the connection DSN.

### Redis image tag

`redis:8-alpine` (as named in the task-4 brief and in
`tests/containers/redis-store.test.ts`) exists and was used directly — no fallback
to `redis:7-alpine` was needed.

## M1 — openid-client v6 API

Measured against the pinned `openid-client@6.8.8` (`ts/pnpm-workspace.yaml`
catalog), by reading the installed type declarations directly rather than from
memory or v5-era examples. v6 exports **functions that take a `Configuration`
object**, not a `Client` class — every claim below is read from
`ts/node_modules/.pnpm/openid-client@6.8.8/node_modules/openid-client/build/index.d.ts`
(2382 lines) unless a different file is named. The underlying primitive types
(the `[clockTolerance]`/`[clockSkew]` symbols, error classes, `IDToken`) live in
`oauth4webapi`, which openid-client re-exports as `oauth.*`; those are read
from `ts/node_modules/.pnpm/oauth4webapi@3.8.8/node_modules/oauth4webapi/build/index.d.ts`.

### Discovery entry point

```ts
export declare function discovery(
  server: URL,
  clientId: string,
  metadata?: Partial<ClientMetadata> | string,
  clientAuthentication?: ClientAuth,
  options?: DiscoveryRequestOptions,
): Promise<Configuration>;
```

(`index.d.ts:925`.) It fetches `${server}/.well-known/openid-configuration`
(the default `algorithm: 'oidc'`, `index.d.ts:777-793`) and returns a
`Configuration` wrapping both the discovered `ServerMetadata` and the supplied
`ClientMetadata`. `AuthorizationServer.issuer` (`oauth4webapi` `index.d.ts:557`)
is the only REQUIRED server-metadata field; every other field (`token_endpoint`,
`jwks_uri`, `authorization_endpoint`, `revocation_endpoint`,
`end_session_endpoint`, ...) is optional on the type but required in practice
for whichever call needs it — `authorizationCodeGrant`'s own doc comment notes
"URL of the authorization server's token endpoint must be configured."

### How the client's credentials are supplied

`metadata: Partial<ClientMetadata> | string` — a string is shorthand for
`client_secret` alone (`index.d.ts:914` doc comment: "when a string is passed
it is a shorthand for passing just `ClientMetadata.client_secret`"). The
adapter passes an object instead, so it can also set `[clockTolerance]` in the
same call: `{ client_secret: <secret>, [clockTolerance]: <seconds> }`.
`clientAuthentication` (4th positional arg) is left `undefined` — its default
is `ClientSecretPost` using `ClientMetadata.client_secret`
(`index.d.ts:917-918`), which is exactly the confidential-client shape
§ 6.3 of the design doc requires. `ClientSecretPost`/`ClientSecretBasic`/etc.
(`index.d.ts:71,122`) exist for an explicit override; none is needed here.

### Building the authorization URL — PKCE, `state`, `nonce`

```ts
export declare function buildAuthorizationUrl(
  config: Configuration,
  parameters: URLSearchParams | Record<string, string>,
): URL;
```

(`index.d.ts:2034`, synchronous — no network call.) PKCE, `state` and `nonce`
are all plain string entries in `parameters`, not separate arguments:

```ts
let code_verifier = client.randomPKCECodeVerifier()
let code_challenge = await client.calculatePKCECodeChallenge(code_verifier)
let redirectTo = client.buildAuthorizationUrl(config, {
  redirect_uri, scope, code_challenge, code_challenge_method: 'S256',
  state, nonce,
})
```

(`index.d.ts:2011-2020`, the library's own example.) `randomPKCECodeVerifier`
(`:726`), `calculatePKCECodeChallenge` (`:718`), `randomNonce` (`:734`) and
`randomState` (`:742`) are all exported helpers; the adapter uses
`randomPKCECodeVerifier`/`calculatePKCECodeChallenge`/`randomNonce` and takes
`state` from the caller (task 8's transaction id), since `buildAuthorizationUrl`
treats it as an opaque parameter either way.

### The authorization-code grant call — expected `state`/`nonce`

```ts
export interface AuthorizationCodeGrantChecks {
  expectedNonce?: string;
  expectedState?: string | typeof skipStateCheck;
  idTokenExpected?: boolean;
  maxAge?: number;
  pkceCodeVerifier?: string;
}
export declare function authorizationCodeGrant(
  config: Configuration,
  currentUrl: URL | Request,
  checks?: AuthorizationCodeGrantChecks,
  tokenEndpointParameters?: URLSearchParams | Record<string, string>,
  options?: AuthorizationCodeGrantOptions,
): Promise<oauth.TokenEndpointResponse & TokenEndpointResponseHelpers>;
```

(`index.d.ts:1814-1910`.) `currentUrl` is the full callback URL (or a `Request`);
`code`/`state` are read from it. `checks.expectedState` and
`checks.expectedNonce` are the caller-supplied expectations — "This value must
match exactly" (`:1822-1828`) — so the adapter passes both from the stored
login transaction (`state` = txn id, `nonce` = the value stored alongside
`codeVerifier`). `TokenEndpointResponseHelpers.claims()` (`:1176`) returns the
verified `oauth.IDToken` (`undefined` only if no `id_token` came back), and
`.expiresIn()` (`:1188`) returns seconds until the access token expires. ID
Token claim validation — `iss`, `aud`, `nonce`, `exp`/`iat` (subject to clock
tolerance), and signature/`alg` — happens **inside** this call, which is what
`tests/adapters/oidc.test.ts` exercises for the nine required cases: it is the
one call in the whole API surface that runs verification, so there is no
separate "verify this JWT" function to call instead.

### The refresh-token grant

```ts
export declare function refreshTokenGrant(
  config: Configuration,
  refreshToken: string,
  parameters?: URLSearchParams | Record<string, string>,
  options?: DPoPOptions,
): Promise<oauth.TokenEndpointResponse & TokenEndpointResponseHelpers>;
```

(`index.d.ts:1953`.) Same response shape as the code grant (`access_token`,
optional `refresh_token`, `.expiresIn()`), which is why `oidc.refresh` can
return exactly the `RefreshedTokens` shape `core/single-flight.ts`'s
`ResolveDeps.refresh` already expects.

### Token revocation and the end-session URL

```ts
export declare function tokenRevocation(
  config: Configuration,
  token: string,
  parameters?: URLSearchParams | Record<string, string>,
): Promise<void>;

export declare function buildEndSessionUrl(
  config: Configuration,
  parameters?: URLSearchParams | Record<string, string>,
): URL;
```

(`index.d.ts:2360`, `:2235`.) `tokenRevocation` POSTs to
`ServerMetadata.revocation_endpoint` and resolves `void` — the adapter treats
any thrown error as best-effort-failed, matching design doc § 9.5 ("revoke...
best-effort"). `buildEndSessionUrl` is synchronous, like
`buildAuthorizationUrl` — no network call — and takes `post_logout_redirect_uri`
/ `id_token_hint` / `state` as plain `parameters` entries.

### Where clock tolerance is configured — symbol-keyed, confirmed

```ts
export declare const clockTolerance: typeof oauth.clockTolerance;
export declare const clockSkew: typeof oauth.clockSkew;
```

(`index.d.ts:598,618`, re-exporting `oauth4webapi`'s own symbols.) The
underlying declarations are `unique symbol`, not string keys:

```ts
export declare const clockSkew: unique symbol;        // oauth4webapi index.d.ts:256
export declare const clockTolerance: unique symbol;   // oauth4webapi index.d.ts:274
```

and `oauth.Client` (which `ClientMetadata extends`) declares them as symbol-keyed
optional properties: `[clockSkew]?: number` and `[clockTolerance]?: number`
(`oauth4webapi index.d.ts:985,989`). So tolerance is set as an entry in the
**client metadata object**, computed with the symbol as the key, not a string
option and not a field on `Configuration` itself:

```ts
const metadata: Partial<client.ClientMetadata> = {
  client_secret: clientSecret,
  [client.clockTolerance]: clockToleranceSeconds, // seconds; default 30
};
```

`Configuration` itself (the class returned by `discovery()`) has no
`clockTolerance` property or setter — `ConfigurationProperties`
(`index.d.ts:1070-1083`) exposes only `[customFetch]` and `timeout`. Confirming
this took reading both the `openid-client` re-export and the `oauth4webapi`
declaration site, since `openid-client`'s own file only re-exports the constant
and does not repeat that it is a `unique symbol`.

### Per-request timeout

`Configuration` has a plain (non-symbol) `timeout` accessor, in **seconds**:

```ts
get timeout(): number | undefined;
set timeout(value: number | undefined);
```

(`index.d.ts:1149-1153`, under `ConfigurationProperties`, default `30`.) It is
also settable at discovery time via `DiscoveryRequestOptions.timeout` (seconds),
which then persists onto the resolved `Configuration`: "If this option is used,
then the same timeout value will be assigned to the resolved Configuration
instance for use with **all its future individual HTTP requests**"
(`index.d.ts:836-840`). So `PAIGASUS_OIDC_HTTP_TIMEOUT_MS / 1000` passed once to
`discovery()`'s `options.timeout` covers every later call the adapter makes
through that `Configuration` — discovery, the code grant, refresh, revocation —
with no separate per-call timeout argument to thread through (there is none on
`authorizationCodeGrant`/`refreshTokenGrant`/`tokenRevocation` — their only
`options` shapes are `AuthorizationCodeGrantOptions`/`DPoPOptions`, both DPoP-only).

### Insecure-transport escape hatch (test-only)

`discovery()`'s HTTPS-only restriction can be lifted for the discovery request
itself, and thereafter for the resulting `Configuration`, via
`options.execute: [client.allowInsecureRequests]` (`index.d.ts:800-802,821-824`).
The adapter exposes this as an internal `allowInsecureRequests` option on
`CreateOidcClientOptions`, defaulting to `false` and never set by
`runtime.ts` — it exists solely so `tests/adapters/oidc.test.ts` can point the
adapter at the local plain-`http` JWKS fixture (`tests/fixtures/jwks.ts`).
`authEnvShape.PAIGASUS_OIDC_ISSUER` (`src/config.ts`) is a strict `httpsUrl`, so
no production path can reach an `http://` issuer regardless.

### M1 addendum — `authorizationCodeGrant` does NOT verify the id_token signature by default

**Measured, and load-bearing for the "signed by a different key" test.** With a
plain `discovery()` call (no `execute` array), `authorizationCodeGrant` accepts
an ID token signed with a private key that is never published in the served
JWKS document, under a `kid` that impersonates a real, published key. Verified
directly: minting such a token and running it through
`client.authorizationCodeGrant` against the local fixture returned the parsed
claims with no error — not a rejection.

The reason is intentional, documented behavior, not a bug:
`enableNonRepudiationChecks`'s own doc comment
(`index.d.ts:1474-1524`) states plainly that "Validating signatures of JWTs
received via direct communication between the client and a TLS-secured
endpoint (which it is here) is not mandatory since the TLS server validation
is used to validate the issuer instead of checking the token signature,"
adding "You only need to use this method for non-repudiation purposes" —
**and this applies to the id_token from a normal `authorizationCodeGrant`,
not only to UserInfo/Introspection JWT responses**, confirmed by reading
`oauth4webapi`'s source: `processAuthorizationCodeOpenIDResponse` /
`processGenericAccessTokenResponse` route the id_token through
`validateIdTokenClaims` (claims only: issuer, audience, subject presence,
`exp`/`nbf`/`nonce`/`auth_time` — never a cryptographic check), and the only
call sites of `validateJwsSignature`/`getPublicSigKeyFromIssuerJwksUri` in the
whole library are `validateApplicationLevelSignature` (an opt-in the caller
must invoke manually with the raw `TokenEndpointResponse`) and the JARM/hybrid
response-mode paths, neither of which a standard authorization-code BFF flow
touches.

**This is a real, spec-compliant design choice** — RP↔token-endpoint TLS
already authenticates the issuer — but it means an adapter that only follows
the "recommended" `discovery()` → `authorizationCodeGrant()` path ships with
**no defense-in-depth signature check on the ID token at all**, and the
brief's own explicit requirement ("a token signed by a different key is
rejected") is not satisfiable without opting in.

**Fix:** `createOidcClient` passes `execute: [client.enableNonRepudiationChecks, ...]`
to `discovery()`'s options unconditionally (in addition to
`allowInsecureRequests` when the test-only escape hatch is set). Verified
directly: with `enableNonRepudiationChecks` in `execute`, the same
different-key token now throws `ClientError: invalid response encountered`,
and a well-formed token is still accepted — confirming the fix does not
introduce a false rejection.

### M1 addendum — the brief's literal 45 s/120 s clock-skew figures do not hold against the measured library

The task-7 brief (and design doc § 11.3) specify: "a token 45 s in the future is
accepted under a 30 s clock tolerance; a token 120 s in the future is rejected."
**This exact pairing is not reachable against the measured
`oauth4webapi@3.8.8` behavior for any natural ID Token construction**, and the
adapter is not changed to fake it. The two relevant checks, read directly from
`ts/node_modules/.pnpm/oauth4webapi@3.8.8/node_modules/oauth4webapi/build/index.js:1815-1841`:

```js
if (claims.exp !== undefined) {
  if (claims.exp <= now - clockTolerance) throw /* expired */;
}
if (claims.nbf !== undefined) {
  if (claims.nbf > now + clockTolerance) throw /* not yet valid */;
}
```

Both are strict `>`/`<=` comparisons against **exactly** `clockTolerance`
seconds, not a multiple of it, and there is no third check anywhere in
`openid-client` or `oauth4webapi` that bounds `iat` from the future for a plain
`authorizationCodeGrant` ID Token (the one place `iat` gets a "too far" check —
`index.js:1988-1989`, "too far in the past", a **past**-only, 1-hour **fixed**
bound — belongs to `validateJwtAuthResponse`, the JARM/hybrid-flow path, which
`authorizationCodeGrant` never calls).

Consequences, both verified by direct arithmetic against the code above:

- **`exp` cannot produce a "further future value rejected" case at all.** A
  future `exp` is always `> now - clockTolerance` for any positive offset, so
  it is accepted regardless of magnitude — 45 s and 120 s alike.
- **`nbf` is the only future-direction, tolerance-gated check that exists**,
  but its boundary is exactly `clockTolerance`. Under the specified 30 s
  tolerance, `nbf = now + 45` gives `45 > 30` → **rejected**, not accepted as
  the brief requires; `nbf = now + 120` also rejects. No choice of tolerance
  equal to 30 makes 45 accepted and 120 rejected, because both figures sit on
  the same side of a 30-second boundary.

**Resolution taken (original, review round 1 superseded this — see below):**
`tests/adapters/oidc.test.ts` tested the same underlying property the brief's
case exists to protect — that `PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS` is
actually wired into `[client.clockTolerance]` and enforced by the library —
using `nbf` values that correctly straddle a 30 s tolerance: `now + 20`
(inside tolerance, accepted) and `now + 45` (outside tolerance, rejected).
This is the real, measured mechanism exercised with figures that are actually
true statements about it, in place of the brief's 45 s/120 s pair, which
described a claim comparison the installed library does not implement.
Reported per the task's own instruction to name a requirement that cannot be
met against the real API rather than inventing a workaround that fakes it.

### Review round 1, Important 3 — the original pair still didn't isolate the wiring

The review that followed task 7 found a real hole in the resolution above:
**both tests used `clockToleranceSeconds: 30` — `oauth4webapi`'s own default**
(`getClockTolerance`, `index.js:435-440`, returns `30` whenever
`[clockTolerance]` is absent from the client metadata). So deleting
`[client.clockTolerance]: opts.clockToleranceSeconds` from `oidc.ts` entirely
— i.e. never wiring the configured value at all — left BOTH tests passing
unchanged: `nbf = now + 20` is still `<= 30` and `nbf = now + 45` is still
`> 30`, regardless of whether the number 30 came from configuration or from
the library falling back to its own default. The tests could not distinguish
"we passed 30" from "we passed nothing," which was the one property they
existed to prove.

**Fix:** replaced the pair with one test that mints a SINGLE token
(`nbf = now + 45`) and runs it through TWO clients configured with two
DIFFERENT, both non-default tolerances — `60` (expected: accepted, since
`45 <= 60`) and `30` (expected: rejected, since `45 > 30`). This verdict can
only flip between the two calls if the `clockToleranceSeconds` argument
actually reaches `[client.clockTolerance]`; if the line were deleted, both
calls would fall back to the library's default 30 and BOTH would reject,
failing the "accepted under 60" half.

**Mutation-tested, as this repository's convention requires (see M5 below for
the precedent).** `[client.clockTolerance]: opts.clockToleranceSeconds` was
removed from `src/adapters/oidc.ts`'s `discovery()` call (bounded by
`IMPORTANT-3-MUTATION-START`/`END` markers), and
`pnpm -C ts/packages/paigasus-auth exec vitest run tests/adapters/oidc.test.ts`
was re-run against the mutated file: **1 of 13 tests failed** — exactly the
new test, with:

```
AssertionError: promise rejected "Error: oidc authorization_code_grant fail…" instead of resolving
Caused by: Error: oidc authorization_code_grant failed: ClientError
```

— i.e. the `makeClient(60)` call, expected to ACCEPT `nbf = now + 45` under an
explicit 60 s tolerance, instead rejected it, because with the line removed
`getConfig()` requests no tolerance and the library silently falls back to its
own default of 30 (under which 45 exceeds tolerance regardless of the `60`
argument the test believed it was configuring). All 12 other tests, including
the six other ID-token cases, stayed green — confirming the mutation is
isolated to exactly the property this test exists to check. Restored by
deleting the two marker comments and the disabled clause (never
`git checkout --`), and re-ran: 13 of 13 passed again, and the full suite
(112 tests) was re-run clean afterward.

### Review round 1, Important 1 — the four costs of always-enabled non-repudiation checks

Enabling `client.enableNonRepudiationChecks` unconditionally (the M1 addendum
above) is the right call and stays unconditional — reported here are its
costs, not a reason to revert it.

1. **Latency.** JWKS is cached in a `WeakMap` for 300 s per `AuthorizationServer`
   object (`oauth4webapi` `index.js:1005-1037`), so the added cost is
   approximately one extra round trip per five minutes per process — small.
2. **Availability.** Before this change, an unreachable `jwks_uri` did not
   affect login at all (nothing in the plain `authorizationCodeGrant` path
   reads it). After, with a cold or stale cache, an unreachable `jwks_uri`
   fails EVERY login and EVERY refresh — JWKS moved from "unused" to "a
   dependency of both paths."
3. **Key rotation.** On an unknown `kid`, `getPublicSigKeyFromIssuerJwksUri`
   (`index.js:1025-1096`) only refetches the JWKS if the cached copy is
   already at least 60 s old; otherwise it throws `KEY_SELECTION` immediately.
   A legitimate login can therefore fail for up to 60 s immediately after an
   IdP rotates its signing key.
4. **HS256 becomes fatal.** `validateApplicationLevelSignature` throws
   `UnsupportedOperationError` for any `alg` starting with `HS`
   (`index.js:1206-1209`, `header.alg.startsWith('HS')`). An IdP configured to
   sign ID tokens with a client-secret HMAC would log in successfully before
   this change and cannot after.

**Ruling on point 4, recorded so it is not re-opened blind:** kept
unconditional. ADR-0015 already pins IAM to RS256/ES256 for access tokens, so
an asymmetric ID token is consistent with the deployments this package
actually serves — an HS256 IdP is not a configuration this package is
expected to support, and failing loudly on one is preferable to silently
skipping signature verification for everyone to accommodate it.

## M5 — single-flight refresh: proving AC 2's test can fail

Task 5's Step 8 is mandatory: an AC 2 test that cannot fail proves nothing. The
guard was mutated in `src/core/single-flight.ts` by commenting out
`if (await store.tryAcquireLock(sid, lockToken, lockTtlMs))` and replacing it
with an unconditional `if (true)`, so every caller enters the refresh critical
section instead of contending for the per-session lock. Markers
`M5-MUTATION-START` / `M5-MUTATION-END` bounded the change during the run and
were deleted afterward — never reverted with `git checkout --`, since that
would also have discarded the (uncommitted, at the time) Task 5 implementation.

`pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/single-flight.test.ts`
against the mutated file: **5 of 13 tests failed.**

The two tests this step exists to break both failed on the refresh counter,
which is the AC 2 claim itself:

- `TWO CONCURRENT CALLERS ON AN EXPIRED TOKEN TRIGGER EXACTLY ONE REFRESH` —
  `expected 2 to be 1`. **Observed refresh count: 2.**
- `TWENTY concurrent callers still trigger exactly one refresh` —
  `expected 20 to be 1`. **Observed refresh count: 20** (one call per
  concurrent caller — the lock was providing zero exclusion).

Three further tests failed as a side effect of the same mutation, for a
different reason: `returns null when the record is deleted between the read
and the lock`, `returns the stale-but-live record when the lock cannot be won
in time`, and `returns null when the lock cannot be won and the token is
genuinely expired` all threw `Error: must not refresh`, because their fixtures
rely on losing the lock race and removing the guard makes every caller "win"
it. This is expected collateral damage of this specific mutation, not a
separate defect — it is further evidence the guard is load-bearing, not an
additional AC 2 finding.

After confirming the failure, the guard was restored by deleting the two
marker comments and the `if (true)` line (never `git checkout --`), and
`tests/core/single-flight.test.ts` was re-run: 13 of 13 tests passed again.

### M5, second mutation — invariant 1 (the post-lock double-check)

Review round 1 (F5) found that nothing in the original suite would fail if
the post-lock double-check were deleted: the two "exactly one refresh" tests
resolve a WAITER through the outer loop's re-read, never through the
post-lock double-check, so the first M5 mutation above does not exercise this
invariant at all.

A new deterministic test was added —
`the post-lock double-check stops a second holder from refreshing an
already-refreshed record (F5, invariant 1)` — that overrides `store.get` so
the FIRST call (the pre-lock read) returns a stale, expired copy, and every
later call (in particular the post-lock double-check) returns the record
another holder already refreshed. It asserts `refresh` is called zero times.

The double-check line,

```ts
if (!shouldRefresh(Date.now(), fresh.accessExpiresAt, skewMs)) return fresh; // invariant 1
```

was commented out (bounded by `M5-MUTATION2-START` / `M5-MUTATION2-END`
markers) and the suite re-run:

`pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/single-flight.test.ts`
against the mutated file: **1 of 20 tests failed** — exactly the new F5 test,
with `TypeError: Cannot read properties of undefined (reading 'expiresIn')`
(the mocked `refresh` in that test is a bare `vi.fn()` with no return value
configured, since the test's whole point is that it must never be called).
All 19 other tests, including both original "exactly one refresh" tests,
stayed green — confirming they do not exercise this invariant, exactly as
the review predicted.

Restored by deleting the `M5-MUTATION2-START`/`M5-MUTATION2-END` markers and
the commented-out line (never `git checkout --`), and re-ran: 20 of 20
tests passed again.

## M3 — `NODE_EXTRA_CA_CERTS` + `ignoreHTTPSErrors` suffice for the self-signed cert on both sides

Task 13's E2E tier has TWO parties that must trust Keycloak's runtime-generated self-signed
certificate (SAN `127.0.0.1`/`localhost`, minted by `tests/e2e/tls-fixture.ts` via `openssl`):

- The **browser** (Chromium via Playwright), which navigates to Keycloak's own hosted pages
  (the authorization endpoint, the login form, the end-session endpoint) directly.
- The **fixture server** (`tests/e2e/fixture-server.ts`), a plain `node` process that talks to
  Keycloak itself for OIDC discovery, the token exchange, and (best-effort) revocation.

**Measured: both assumptions hold, but they are two DIFFERENT mechanisms, not one.**
`playwright.config.ts`'s `use.ignoreHTTPSErrors: true` covers the browser side — verified by the
browser successfully rendering and submitting Keycloak's login form over `https://127.0.0.1:<port>`
with no certificate-warning interstitial blocking it. `NODE_EXTRA_CA_CERTS`, set in
`playwright.config.ts`'s `webServer.env` to the cert file `tls-fixture.ts` generates, covers the
fixture server side — verified by `roundtrip.spec.ts`'s AC 1 test completing a real token exchange
(`POST /protocol/openid-connect/token`) against that same self-signed listener. Before this was
wired correctly, the fixture server's own `authorizationCodeGrant` call failed with a TLS
certificate-verification error; setting `NODE_EXTRA_CA_CERTS` to the exact cert file Keycloak was
started with (not merely "a" self-signed cert) resolved it. The two settings do not substitute for
each other — dropping either one reproduces the corresponding party's own TLS failure.

## M6 — the Keycloak realm JSON for a confidential code-flow client on tag 26.4

**Renumbered from an earlier draft slot.** This is the design doc's own M6 (§ 20's table); see the
M13 section below for the collision this resolves.

Started from `rs/crates/services/paigasus-iam/tests/fixtures/keycloak-realm.json`. Its one client,
`paigasus-cli`, cannot be reused for an authorization-code round trip: `publicClient: true` and
`standardFlowEnabled: false` make it a password-grant-only client with no secret. Added a second,
CONFIDENTIAL client, `paigasus-e2e-rp` (`publicClient: false`, `standardFlowEnabled: true`, a fixed
`secret`, `redirectUris`/`webOrigins` pointed at the fixed fixture-server port), plus
`offline_access` promoted from Keycloak's default OPTIONAL client scope to a DEFAULT one. Working
file: `ts/packages/paigasus-auth/tests/e2e/keycloak-realm.json`.

**Three things Keycloak 26.4's realm importer rejected, measured directly against the running
container's logs (`docker logs <id>`) — the next person will hit these too if they hand-edit this
file:**

1. **Any unrecognised field on a `ClientRepresentation` hard-fails the ENTIRE realm import**, not
   just that client. A `_comment` string field (an attempt to document the client inline, since
   JSON has no comment syntax) produced:
   `ERROR: Unrecognized field "_comment" (class org.keycloak.representations.idm.ClientRepresentation), not marked as ignorable`,
   followed by `ERROR: Failed to run import` / `ERROR: Failed to start server in (development)
   mode` — the container's realm never loaded and Keycloak's discovery endpoint never came up.
   Fix: no inline JSON comments; the client's rationale lives in
   `ts/packages/paigasus-auth/tests/e2e/constants.ts`'s doc comment instead.
2. **`postLogoutRedirectUris` is not a top-level `ClientRepresentation` field** in this schema
   version either — same class of error
   (`Unrecognized field "postLogoutRedirectUris"`, with the same 44-property list Keycloak reports
   as everything it DOES recognise). The correct location is the client ATTRIBUTE
   `post.logout.redirect.uris`, nested under `"attributes": {...}`.
3. **`offline_access` in `defaultClientScopes` alone does not make a refresh token issuable.**
   Once the realm imported cleanly, the token exchange itself failed with a 400:
   `error: 'not_allowed', error_description: 'Offline tokens not allowed for the user or client'`
   (captured via a temporary `console.error` in `src/adapters/oidc.ts`'s
   `authorizationCodeGrant` catch block, reverted after diagnosis). Keycloak auto-creates the
   `offline_access` REALM ROLE for every realm, but does not auto-assign it to a user declared in
   a realm-import JSON — the fix was adding `"realmRoles": ["offline_access"]` to `alice`'s user
   object.

After all three fixes, `waitForKeycloakReady`'s discovery poll (`tests/e2e/global-setup.ts`)
succeeds and the full round trip in `roundtrip.spec.ts` completes — see M3's own successful
token-exchange observation above, and the M6-part-3 result is now also directly verified by AC 1's
`expect(debugBody.refreshToken).not.toBeNull()` assertion (fix round 1, Important 3).

## M8 — does the fixture server start under `--conditions=react-server`?

**No — measured, not assumed, and the brief's own plan (mirrored from the design doc's original
M8 assumption) does not survive task 10.** `src/server.ts` opens with `import 'server-only'`,
whose exports map resolves to a no-op only under the `react-server` condition — the design doc's
M8 assumed setting that condition via `NODE_OPTIONS=--conditions=react-server` would be enough to
start the fixture server, which mounts `createAuthRouteHandler` (`src/server.ts`).

**First measurement (reproducing the brief's plan).** Ran, from
`ts/packages/paigasus-auth`, with the shared `.js`-to-`.ts` loader
(`tests/fixtures/ts-esm-loader.mjs`) and a probe script importing `./src/server.js`:

```
NODE_OPTIONS="--conditions=react-server" node --experimental-transform-types \
  --import "file://.../tests/fixtures/ts-esm-loader.mjs" probe.mjs
```

Result: `IMPORT_FAILED`, `Error [ERR_MODULE_NOT_FOUND]: Cannot find module
'.../node_modules/next/headers' imported from '.../src/next/get-session.ts'`. This is a DIFFERENT
concrete symptom than vitest.config.ts's own comment predicts (a `React.createContext` crash at
module scope in `next/navigation`'s react-server build) — but the same underlying incompatibility:
setting `react-server` breaks the resolution of `next/headers`/`next/navigation`, which
`src/server.ts` re-exports through `src/next/get-session.ts`, exactly as vitest.config.ts's header
comment already documented for the vitest case.

**Second measurement (isolating the cause).** Removing `--conditions=react-server` entirely did
**not** fix the import — the identical `next/headers` resolution error reproduced with no
conditions flag at all. Isolated with a minimal repro (`node --experimental-transform-types
--input-type=module -e "import('next/headers')..."`, run from the package directory, no custom
loaders): same `ERR_MODULE_NOT_FOUND`, with Node's own hint `Did you mean to import
"next/headers.js"?`. Confirmed `next`'s `package.json` has no `exports` field at all
(`grep -n '"exports"' node_modules/next/package.json` — no match), so Node's ESM resolver falls
back to legacy extensionless resolution for a bare deep import and — unlike Vite's resolver, which
is what makes this work under vitest — does NOT probe `.js`/`.mjs`/`index.js` on its own.
`import('next/headers.js')` (the literal suggested fix) resolved successfully in the same probe.
This second problem is unrelated to `server-only`/`react-server` and reproduces identically with
either condition-list setting.

**Fix, mirroring task 10's actual solution rather than the design doc's original plan:** a Node
module customization hook, `tests/e2e/e2e-loader.mjs`, that (1) redirects the bare specifier
`server-only` to `tests/support/server-only-stub.ts` (the same permanent alias vitest.config.ts
uses, applied here via `module.register`'s `resolve` hook instead of Vite's `resolve.alias`), and
(2) retries any OTHER `ERR_MODULE_NOT_FOUND` bare-specifier resolution with a literal `.js`
extension appended before giving up. No `--conditions` flag is set at all. Verified working: the
fixture server starts and completes a full login (`roundtrip.spec.ts`'s AC 1), which requires
`src/server.ts`'s entire import graph — `next/headers`, `next/navigation`, `server-only` included —
to resolve and execute successfully.

## M13 — PKCE: proving Task 8's callback tests actually witness the exchange

**Renumbered from M6 to M13 (task 13, fix round 1).** The design doc's § 20 measurement table
(`docs/superpowers/specs/2026-09-09-sma-506-auth-design.md`) is the authority on M-numbers, and it
already defines M6 as "the Keycloak realm JSON for a confidential code-flow client on tag 26.4" —
task 13's own measurement. This section (Task 8's PKCE proof) took the M6 label first, before that
collision was noticed, and is renumbered here so the design doc's table and this file agree. Any
earlier reference to "M6" meaning PKCE should now read M13.

Task 8's review round 1 (Important 2) found that `tests/fixtures/jwks.ts`'s
`/token` handler drained the token request body without inspecting it, so
neither of these was reachable by any test:

- Replacing `codeVerifier: tx.codeVerifier` in `src/http/routes.ts` with any
  constant.
- Omitting `pkceCodeVerifier` from the `authorizationCodeGrant` call in
  `src/adapters/oidc.ts` entirely.

ADR-0017 decision 4 delegates PKCE to `openid-client` specifically so it
cannot be got wrong — a guard nothing tests is not delegated, it is assumed.

**Fix.** The fixture now exposes `setNextCodeChallenge(challenge)` and
`s256CodeChallenge(codeVerifier)` (both exported from `tests/fixtures/jwks.ts`).
When a challenge is armed, `/token` recomputes `BASE64URL(SHA256(code_verifier))`
from the request body and 400s (`invalid_grant`) on a mismatch or an absent
verifier; when no challenge is armed (every OTHER caller of the fixture —
`tests/adapters/oidc.test.ts`, the containers suites), the check is skipped
entirely, so nothing else changed behaviour. `tests/http/callback.test.ts`
arms a FRESH `codeVerifier` (`crypto.randomUUID()`) per test in `beforeEach`,
so no single hardcoded mutation constant could coincidentally satisfy it.

**Mutation 1 — `src/adapters/oidc.ts`, Important 2's own repro.**
`pkceCodeVerifier: params.codeVerifier` was deleted from the
`client.authorizationCodeGrant` call (bounded by `M6-MUTATION-START`/`-END`
markers).

`pnpm -C ts/packages/paigasus-auth exec vitest run tests/http/callback.test.ts tests/adapters/oidc.test.ts`
against the mutated file: **7 of 17 `callback.test.ts` tests failed** — all
six success-path tests, plus the replay test (whose first attempt is itself a
success). Every failure was `Error: callback rejected: code_exchange_failed`,
thrown from `routes.ts`'s `reject`, because the fixture's `/token` handler
received no `code_verifier` at all and 400'd. `tests/adapters/oidc.test.ts`'s
13 tests stayed green, unaffected — confirming it does not, and did not
before this fix, exercise PKCE either (it never calls
`setNextCodeChallenge`).

Restored by deleting the `M6-MUTATION-START`/`-END` markers and the omitted
line, and re-ran: 17 of 17 `callback.test.ts` tests passed again.

**Mutation 2 — `src/http/routes.ts`, the brief's own repro.**
`codeVerifier: tx.codeVerifier` was replaced with
`codeVerifier: 'a-constant-verifier'` (bounded by the same marker names).

`pnpm -C ts/packages/paigasus-auth exec vitest run tests/http/callback.test.ts`
against the mutated file: **the same 7 of 17 tests failed**, same reason —
the constant verifier's S256 hash cannot match a challenge computed from a
fresh `crypto.randomUUID()` generated after the mutation was made.

Restored by reverting to `codeVerifier: tx.codeVerifier` and re-ran: 17 of 17
passed again. Both mutations were reverted with `Edit`, never `git checkout
--`, and the full suite (`pnpm -C ts/packages/paigasus-auth exec vitest run`)
was re-run clean afterward: 161 of 161.

## M12 — does Keycloak honour `client_id`-only end-session, with no `id_token_hint`?

Task 9's `src/http/routes.ts` deliberately omits `id_token_hint` from the
end-session redirect (the raw ID token JWT is never stored in
`SessionRecord`) and relies on `openid-client@6.8.8` appending `client_id`
unconditionally instead. The design doc and the task 9 report both name this
a real, untested-until-now question: does an actual identity provider accept
that combination, or does it interpose a confirmation page first? Task 9's
ruling that omitting `id_token_hint` was acceptable rested on the assumption
that Keycloak would honour `client_id` alone — this measurement is what is
supposed to convert that ruling from reasoning into evidence.

**CORRECTED (fix round 1, Important 1).** The first version of this
measurement asserted only that `page.getByTestId('public-heading')` becomes
visible after clicking logout, and argued that a confirmation interstitial
would have timed that assertion out. True for the interstitial hypothesis,
but insufficient in general: `routes.ts`'s `handleLogout` has a DEGRADED arm
— if `buildEndSessionUrl` throws (discovery failing, or no
`end_session_endpoint` advertised), it redirects straight to
`runtime.postLogoutRedirectUri` WITHOUT contacting Keycloak at all, and
`runtime.ts` defaults that URI to `${PAIGASUS_PUBLIC_ORIGIN}${basePath}/` —
the public page. `public-heading` becoming visible is therefore consistent
with BOTH "Keycloak was reached and honoured the redirect" and "Keycloak was
never contacted at all"; the original assertion could not tell those apart.
There was also a narrower gap even setting that aside: nothing in the
original version observed the end-session request's QUERY STRING directly —
that `client_id` is present and `id_token_hint` absent rested on reading
`oidc.ts` plus `tests/adapters/oidc.test.ts`, an inferred link inside a
measurement whose entire point is to replace inference with observation.

**Fix: capture the navigation to Keycloak's own end-session endpoint and
assert on it directly.** `logout.spec.ts` now does:

```ts
const [endSessionRequest] = await Promise.all([
  page.waitForRequest((req) => req.url().includes('/protocol/openid-connect/logout')),
  page.getByTestId('logout-button').click(),
]);
const endSessionUrl = new URL(endSessionRequest.url());
expect(endSessionUrl.searchParams.get('client_id')).toBe(KEYCLOAK_CLIENT_ID);
expect(endSessionUrl.searchParams.has('id_token_hint')).toBe(false);
```

This rests on the wire: `page.waitForRequest` observes the actual outgoing
HTTP request the browser makes, which can only happen if Keycloak's
end-session endpoint was actually reached — the degraded no-Keycloak-contact
path in `handleLogout` would never produce this request at all, so this
assertion (unlike the original one) DOES distinguish the measured case from
the never-happened case. Only after this passes does the test go on to
assert `public-heading` becomes visible, which now genuinely means "Keycloak
was reached AND honoured the redirect", not "the redirect landed on the
public page for some reason".

**Observed, captured directly from the request (task 13, fix round 1):**

```
GET /realms/paigasus-test/protocol/openid-connect/logout
    ?post_logout_redirect_uri=http%3A%2F%2F127.0.0.1%3A4319%2Fe2e%2F
    &state=JodFdrSoRDpw
    &client_id=paigasus-e2e-rp
```

(line-wrapped here for readability; the real request is one query string).
`client_id=paigasus-e2e-rp` is present, `id_token_hint` is absent, and the
browser's subsequent navigation lands on `public-heading` with no
confirmation interstitial.

**Result: Keycloak 26.4 completes the redirect immediately, with `client_id`
alone.** This confirms task 9's design decision holds for Keycloak, now on
the wire rather than by inference; it says nothing about Okta or any other
provider that may mandate `id_token_hint` (see routes.ts's own "NAMED
RESIDUAL" comment on `handleLogout`).

**Mutation proof that the fix actually catches what the original version could not.**
`src/http/routes.ts`'s `handleLogout` was mutated (bounded by
`M12-MUTATION-START`/`-END` markers) to unconditionally throw before calling
`runtime.oidc.buildEndSessionUrl`, forcing the degraded, never-contacts-Keycloak
arm on every logout. `pnpm -C ts/packages/paigasus-auth exec playwright test
logout.spec.ts` against the mutated file: **the test failed** —
`page.waitForRequest: Test timeout of 30000ms exceeded`, because the
end-session request this assertion waits for never happens when Keycloak is
never contacted. This is exactly the case the original `public-heading`-only
assertion could NOT distinguish from a genuine success (both would have
shown the public page — the mutated one via the degraded redirect, the real
one via the real end-session flow). Restored by deleting the two marker
lines and the injected `throw` (never `git checkout --`, though in this case
the edit and its revert left `git diff` on `routes.ts` empty); re-ran
`logout.spec.ts` afterward: 1 of 1 passed again.
