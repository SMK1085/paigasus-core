# SMA-715 — a malformed `PAIGASUS_SESSION_REDIS_URL` must not write the Redis password to the log

- Linear: SMA-715 (milestone "Frontend", priority High, labels `area:frontend`, `Bug`)
- Related: SMA-705 (found in its spec challenge, D10 and § 10). SMA-705 is now merged on `main`
  (`83d446fc`, PR 331).
- Status: APPROVED by Sven on 2026-09-27, with the decisions of § 5 (D3 to D6) and the answers in
  § 10. Rebased on `origin/main` at `4051df5e` (see § 12).

## 1. Problem

`PAIGASUS_SESSION_REDIS_URL` is a bare `z.string().optional()`
(`ts/packages/paigasus-auth/src/config.ts:98`). Two sites give the value to node-redis
`createClient({ url })`:

1. `createRedisSessionStore` in `ts/packages/paigasus-auth/src/adapters/redis-store.ts:287`,
   called from `createAuthRuntime` (`src/runtime.ts`) and so from `getAuthRuntime`.
2. `redisDescriptorCache` in `ts/packages/paigasus-console-core/src/discovery.ts:312-321`,
   called from `descriptorCacheFor` and so from `createAppDiscovery`. The issue does not name
   this site. The acceptance item "check the other places" finds it (§ 1.3).

node-redis (`@redis/client` 6.2.1, `dist/lib/client/index.js:156-212`) parses the URL
synchronously inside `createClient`, in `parseOptions` → `parseURL`, with
`new (require('node:url').URL)(url)`.

### 1.1 Measured: which parse errors carry the secret

MEASURED on 2026-09-27 with the repository's own `redis@6.2.1`, by a call of `createClient({ url })`
for each value below. The password is `SECRETPW`.

| URL | Error | Own properties | `input` holds the DSN |
|---|---|---|---|
| `redis//u:SECRETPW@h` (no scheme colon) | `TypeError: Invalid URL` | `code`, `input` | yes |
| `redis://u:SECRETPW@h:99999` (port out of range) | `TypeError: Invalid URL` | `code`, `input` | yes |
| `http://u:SECRETPW@h` | `TypeError: Invalid protocol` | none | no |
| `redis://u:SECRETPW@h/abc` | `TypeError: Invalid pathname` | none | no |
| `redis://u:%zzSECRETPW@h` | `URIError: URI malformed` | none | no |
| `unix://u:SECRETPW@/` | `TypeError: Invalid unix URL` | none | no |
| `unix:///tmp/s?db=x` (non-numeric db) | `TypeError: Invalid db query parameter` | none | no |
| `unix:///tmp/%zz` (bad escape in the `unix:` path) | `URIError: URI malformed` | none | no |

No error had a `cause`. The two `ERR_INVALID_URL` rows leak through `input`. Node sets `input` with a
plain assignment, so `input` is an ENUMERABLE own property (measured:
`Object.getOwnPropertyDescriptor(e, 'input').enumerable === true`). `util.inspect` (and so
`console.error`, and Next's own logging of an unhandled error) prints it with default options.

The last two rows were added after the spec challenge. They hold no secret, but they are the two
remaining throw paths of `parseURL`, so the table now lists every throw path. The tests use them
with a sentinel in a form that the parser still reaches (for example
`unix://u:<sentinel>@/tmp/s?db=x`); the implementer checks each test URL against the real parser
and keeps the error class of this table.

One more throw in `parseOptions` embeds the URL in its MESSAGE: `tls socket option is set to …
which is mismatch with protocol or the URL ${options.url} passed`. It fires only when the caller
passes `socket.tls`. Neither site passes it today. § 3.4 pins that.

### 1.2 Measured: an empty or whitespace-only value (Q4, now in scope)

MEASURED on 2026-09-27 with the same `redis@6.2.1`:

| URL | Result of `createClient({ url })` |
|---|---|
| `''` (empty) | NO throw. `options.url` is falsy, so node-redis skips all parsing. The client targets `localhost:6379`. |
| `'   '` (spaces) | `TypeError: Invalid URL`, `code` `ERR_INVALID_URL`, `input` `'   '` |
| `'\t\n'` | `TypeError: Invalid URL`, `code` `ERR_INVALID_URL`, `input` `'\t\n'` |
| `' redis://127.0.0.1:1 '` (valid URL, outer spaces) | NO throw. `new URL()` strips the outer spaces; host `127.0.0.1`, port 1. |

The empty value passes both `=== undefined` checks (`runtime.ts:101` in `requireRedisUrl`,
`discovery.ts:346`). The chart maps the value from the Secret key `session-redis-url`
(`charts/paigasus/templates/console-deployment.yaml:71-75`), so an empty key gives exactly this
value. It is not a leak, but the pod silently connects to `localhost:6379` and the operator gets no
message that names the variable. A whitespace-only value already throws today, but through the
same leaking `ERR_INVALID_URL` shape (no secret in it, because the value holds none).

The last row is a control: a valid URL with outer whitespace works today, and the empty check of
§ 3.5 must not refuse it.

### 1.3 Where the error goes

- **Auth.** The error leaves `createRedisSessionStore`, then `createAuthRuntime`, then
  `getAuthRuntime`. `getAuthRuntime` deletes its slot on a failure (`runtime.ts:265-277`), so each
  request builds the runtime again and throws again. Next logs the raw error for each auth route
  request and each page that resolves a session. The password is in the log each time.
- **Console-core.** `descriptorCacheFor` sets `current.processCache` only after
  `redisDescriptorCache` returns, so a throw leaves the cache unset and the next request tries
  again. `createAppDiscovery` is inside React `cache()` per request (`src/runtime.ts:108`), so the
  same leak repeats per request.
- **SMA-705 `/readyz` (merged).** `readinessResponse` (`ts/packages/paigasus-auth/src/http/readiness.ts`)
  catches the runtime-build error itself and logs `readiness.runtime_failed { error }`, where
  `error` is the error's `name` only. That route does not cover the user request path, and it does
  not cover console-core. SMA-705 also gave `AuthConfigError` an explicit `name`
  (`core/errors.ts:138-148`, `this.name = 'AuthConfigError'`), so after this fix the route logs
  `AuthConfigError` for a malformed URL, where before it logged `TypeError`.

### 1.4 The other places that parse a URL (acceptance item 3)

Read on `origin/main` at `ad1c44b7`, and checked again at `4051df5e`:

| Site | Value | Secret? | Verdict |
|---|---|---|---|
| `paigasus-auth/src/config.ts:26` `httpsUrl` | `PAIGASUS_OIDC_ISSUER`, `PAIGASUS_PUBLIC_ORIGIN` | no | `URL.canParse(v) && new URL(v)` in one refine: short-circuit, no throw. Zod issues carry no input; next-config renders `<key>: <code>` only. No change. |
| `paigasus-auth/src/config.ts:37` `z.url()` | redirect URI overrides | no | zod, no throw. No change. |
| `paigasus-auth/src/adapters/oidc.ts:351` `new URL(opts.issuer)` | issuer | no | Already validated as https by the shape. The client secret goes in the options object, not in a URL. No change. |
| `paigasus-discovery/src/config.ts:48` | `PAIGASUS_SERVICES` addresses | could hold userinfo | `try { new URL } catch { throw fixed message }`. It refuses credentials. Already safe. No change. |
| `apps/{iam,gateway}-console/lib/config.ts:18-19,42` | `PAIGASUS_IAM_GRPC_URL` | could hold userinfo | `URL.canParse` first, then refuses credentials. Already safe. No change. |
| `paigasus-console-core/src/discovery.ts:313` `createClient` | `PAIGASUS_SESSION_REDIS_URL` | **yes** | **Same defect.** In scope (§ 3.2). |
| `paigasus-auth/src/adapters/redis-store.ts:287` `createClient` | `PAIGASUS_SESSION_REDIS_URL` | **yes** | The issue's site. In scope (§ 3.1). |

The other `new URL(...)` calls in `src/` parse a request URL or a derived redirect URI. They hold no
secret. The `testing/` fakes are test support and out of scope.

## 2. Goals

- G1. A malformed `PAIGASUS_SESSION_REDIS_URL` produces an error that holds no part of the URL, at
  both sites: no message text, no own property (`input` included), no `cause`, no stack text.
- G2. The operator still learns WHICH variable is wrong: the fixed message names
  `PAIGASUS_SESSION_REDIS_URL`.
- G3. The retry-on-failure behaviour of both singletons does not change.
- G4 (added by D5). An empty or whitespace-only `PAIGASUS_SESSION_REDIS_URL` is refused at both
  sites with the same fixed, secret-free error as G1. It never connects to `localhost:6379`.
- G5 (added by D4). The SMA-705 `readiness.runtime_failed` event carries the `code` of an
  `AuthError` (for this defect, `auth_config_invalid`), so the operator can tell a configuration
  refusal from another build failure.

## 3. Design

### 3.1 Auth — `src/adapters/redis-store.ts`

Wrap ONLY the `createClient(...)` call in `createRedisSessionStore`. Put it in a small local
function, so the client keeps the type that the compiler infers today:

```ts
const INVALID_REDIS_URL = 'PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL';

function buildClient(opts: RedisSessionStoreOptions) {
  // SMA-715 G4: node-redis skips all parsing for an empty URL and connects to localhost:6379.
  if (opts.url.trim() === '') throw new AuthConfigError(INVALID_REDIS_URL);
  try {
    return createClient({ url: opts.url, ...unchanged options });
  } catch {
    // SMA-715: node-redis parses the URL here with `new URL()`. Its ERR_INVALID_URL TypeError
    // carries the whole DSN, password included, in `input`. Rethrow a fixed message, no cause.
    throw new AuthConfigError(INVALID_REDIS_URL);
  }
}
const client = buildClient(opts);
```

Do NOT write `let client: ReturnType<typeof createClient>`. The comment at `redis-store.ts:97-102`
records that this type does not typecheck for a client made with `commandOptions`. An IIFE with the
same body is also acceptable. The parameter type name above is illustrative; use the options type
that `createRedisSessionStore` already takes. The empty check (§ 3.5) sits before the try, in the
same function.

- `catch` with NO binding. The original error is never read, so no later edit can copy it into the
  new one by accident.
- `AuthConfigError` (`src/core/errors.ts:138`, `code = 'auth_config_invalid'`, `name =
  'AuthConfigError'` since SMA-705) is the existing class for a bad configuration.
  `createAuthRuntime` already throws it for the missing-URL case. No new class. Its constructor
  takes ONE argument (`message`) since SMA-705, so it cannot take a `cause` option by accident.
- The try block holds ONLY `createClient`. The `client.on('error', …)`, the connect and the
  decorator stay outside it, so a real defect there is not reported as a bad URL.
- The catch changes EVERY synchronous `createClient` error into "not a valid Redis URL". On
  node-redis 6.2.1 only `parseOptions` can throw for these options (`#validateOptions` fires only for
  RESP3 features, and the `RedisSocket` constructor has no synchronous `throw`). A later node-redis
  version that adds option validation would make every URL look invalid. The auth default test tier
  mocks `createClient` in every existing file, so § 6.1 adds a valid-URL control row that builds a
  REAL client with the production options and asserts that no error is thrown.

### 3.2 Console-core — `src/discovery.ts`

Same shape in `redisDescriptorCache`: first the empty check of § 3.5, then wrap only
`createClient(...)`, `catch` with no binding, and throw (here `let client: RedisClientType;` before
the try is valid, because line 313 already uses that type; a local function is also acceptable)
`new Error('PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL')`. A plain `Error`, the same as the
missing-URL throw at `discovery.ts:347`. `watchConnectionLoss`, `state().redisClient` and the rest
stay after the try, so a throw leaves `state()` untouched and the next call tries again (G3).

The two sites stay separate. No shared helper: console-core would need a new public export from
`@paigasus/auth/server` for a few lines of code (§ 5, rejected R2).

### 3.3 README

Two READMEs. The fixed message has no detail (D2), so the operator depends on this text.

- `ts/packages/paigasus-auth/README.md` "Redaction" (line 151): a malformed, empty or
  whitespace-only `PAIGASUS_SESSION_REDIS_URL` fails with the fixed message
  `PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL`, and the node-redis parse error is dropped.
- `ts/packages/paigasus-console-core/README.md` (today it has no redaction section): the same
  sentence for the console descriptor cache, which throws a plain `Error` with the same message.
- In both READMEs, list the usual causes: an empty or whitespace-only value (for example an empty
  `session-redis-url` Secret key), a password that holds `@ : / ? # %` and is not
  percent-encoded, a port above 65535, a scheme other than `redis:`, `rediss:` or `unix:`, and a
  database path or `db` parameter that is not a number.
- `ts/packages/paigasus-auth/README.md` "The readiness route (SMA-705)" (line 260), the
  `readiness.runtime_failed { error }` bullet (about line 290) and the "Known limits" bullet on a
  configuration defect: state the new `code` field (§ 3.6), and replace the text that says a
  malformed Redis URL puts the password into the `input` of the `TypeError`. After this fix the
  route logs `error: 'AuthConfigError', code: 'auth_config_invalid'` for that case.

### 3.4 Pin: no `socket.tls` (auth only)

node-redis puts the URL into the MESSAGE of the tls-mismatch `TypeError` (§ 1.1). The § 3.1 catch
also drops that error, so it cannot leak through this path. The pin is still useful: it states the
assumption. Add one assertion to `paigasus-auth/tests/adapters/redis-client-options.test.ts` that
the captured `socket` has no `tls` key. That test mocks `createClient`, so it records the options
BEFORE node-redis parses them.

No console-core pin. The console-core options test reads the RESOLVED options through
`redisClientFromState()?.options`. node-redis 6.2.1 writes the parsed `tls: false` into the
caller's own `socket` object (`parsed.socket = Object.assign(options.socket, parsed.socket)`,
`@redis/client/dist/lib/client/index.js:163`), so `'tls' in socket` is `true` today on unchanged
code. The § 3.2 catch already drops the tls-mismatch error at that site.

### 3.5 The empty-value check (D5, G4)

At each site, before `createClient`: `if (url.trim() === '') throw <the site's fixed error>`.

- The error is the SAME as the parse-failure error of that site: `AuthConfigError` in auth, a plain
  `Error` in console-core, both with the message `PAIGASUS_SESSION_REDIS_URL is not a valid Redis
  URL`. It holds no part of the value.
- The check is at the `createClient` site, not in `runtime.ts` or `config.ts`. So it covers both
  sites with one rule, it does not change the zod shape (D3), and `runtime.ts` does not change.
- The check does not trim the value that goes to `createClient`. A valid URL with outer whitespace
  (§ 1.2, last row) keeps working, as today.
- A whitespace-only value throws already through the § 3.1 parse catch (§ 1.2). The explicit check
  makes the rule independent of node-redis's parser. The consequence for the proof-of-bite is in
  § 6.3 step 6.

### 3.6 SMA-705 readiness event logs `code` (D4, G5)

SMA-705 is on `main`, so this PR makes the change (Q3 answer). In
`ts/packages/paigasus-auth/src/http/readiness.ts`, the runtime-failure branch logs
`readiness.runtime_failed { error, code }`:

- `error` stays the error's `name` (unchanged, `errorName(err)`).
- `code` is present ONLY when the caught value is an `AuthError` (`src/core/errors.ts:4`). Its value
  is that class's literal `code`, for example `auth_config_invalid`. For any other value the field
  is absent.
- Do NOT log `code` from an arbitrary error. A foreign error's `code` is text that this package does
  not control. The node-redis `TypeError` carries `code: 'ERR_INVALID_URL'`, and SMA-705's T11
  fixture (`tests/http/readiness.test.ts:104-107`) has exactly that shape. That fixture must still
  log `{ error: 'TypeError' }` with no `code`.
- Update the comment of `src/ports/logger.ts:18-20` and the file header of `readiness.ts` (THE LOG,
  lines 21-25) to name the new field and its bound.
- `readiness.test.ts:259` (the real cross-field refusal) then expects
  `{ error: 'AuthConfigError', code: 'auth_config_invalid' }`.

The SMA-705 spec § 10 residual (`docs/superpowers/specs/2026-09-27-sma-705-console-oidc-readiness-design.md:431-432`,
"The Redis DSN leak on the user request path (D10). SMA-715 tracks it") gets one added sentence:
closed by SMA-715, with this spec's path. Do not delete the old text.

## 4. Acceptance criteria

- AC1. `createRedisSessionStore` rejects with an `AuthConfigError` whose message is exactly
  `PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL` for every URL in the § 1.1 table.
  A real client built with a valid URL (`redis://127.0.0.1:1`) and the production options does not
  throw (§ 3.1, control against a false "invalid URL").
- AC2. `descriptorCacheFor` with `PAIGASUS_SESSION_STORE: 'redis'` throws an `Error` with the same
  message for every URL in the § 1.1 table. After the throw, `redisClientFromState()` is
  `undefined`. A later call with a VALID Redis URL then puts a new client in state (G3).
- AC3. For every error in AC1, AC2 and AC8, a sentinel password is absent from all of these:
  `message`, `stack`, `String(err)`, every own property name and value
  (`Object.getOwnPropertyNames`, which includes non-enumerable ones),
  `util.inspect(err, { showHidden: true, depth: Infinity })`, and each link of the `cause` chain.
  `err.cause` is `undefined`.
- AC4. `createAuthRuntime` with `PAIGASUS_SESSION_STORE: 'redis'` and the malformed URL
  `redis//u:<sentinel>@h` rejects with an error that meets AC3. (The runtime path is the one Next
  logs.)
- AC5. The auth options pin asserts that the client options carry no `socket.tls` (§ 3.4). There
  is no console-core pin.
- AC6. The README text of § 3.3 exists in both READMEs, including the readiness-route text.
- AC7. The proof-of-bite of § 6.3 is done and recorded in the PR body.
- AC8 (D5). At both sites, `''`, `'   '` and `'\t\n'` are refused with the same error class and
  the same message as AC1 (auth) and AC2 (console-core). No client is built for `''`: in auth the
  `createClient` spy or the real-client path shows no client; in console-core
  `redisClientFromState()` stays `undefined`. `createAuthRuntime` with `PAIGASUS_SESSION_REDIS_URL:
  ''` rejects with the `AuthConfigError`. The control `' redis://127.0.0.1:1 '` is NOT refused at
  either site.
- AC9 (D4). `readiness.runtime_failed` carries `code: 'auth_config_invalid'` for an
  `AuthConfigError`, and carries no `code` for the node-redis-shaped `TypeError` of T11 (which has
  its own `code: 'ERR_INVALID_URL'`).
- AC10 (D4). The SMA-705 spec § 10 residual names SMA-715 as closed (§ 3.6).

## 5. Decisions and rejected options

- D1. Catch at the `createClient` call, not higher. The issue asks for this. A catch in
  `getAuthRuntime` would also redact other errors that are not about the URL.
- D2. A fixed message with no detail. The node-redis messages `Invalid protocol` and so on are safe
  literals, but the tls-mismatch message holds the URL. A copy of "the message when it is safe"
  needs an allow-list that a node-redis bump can make wrong. The operator gets the variable name,
  and reads the README for the form.
- D3 (Sven, Q1). No early zod refine on `PAIGASUS_SESSION_REDIS_URL`. It can refuse valid
  node-redis URL forms. No follow-up issue.
- D4 (Sven, Q3). This PR updates the SMA-705 residual text, and the SMA-705 `/readyz` catch logs
  `code` (`auth_config_invalid`). SMA-705 is on `main`, so the code change is in this PR (§ 3.6).
- D5 (Sven, Q4, CHANGED from the spec default). Fold the empty-value fix in. An empty or
  whitespace-only `PAIGASUS_SESSION_REDIS_URL` is refused at both sites with the same fixed,
  secret-free error, and never connects to `localhost:6379` (§ 3.5). The tests (empty string,
  whitespace, and a valid-URL control) must fail when the check is removed (§ 6.3 step 6).
  Interpretation recorded by the unattended setup run: "the same error" means the same class and
  the same message as the parse-failure error of that site. A separate "is empty" message was not
  chosen.
- D6 (Sven, Q2). The console-core `discovery.ts` `createClient` site is in this PR.
- R1. REJECTED: a zod refine on `PAIGASUS_SESSION_REDIS_URL` in `authEnvShape`. It would fail
  earlier and render as `<key>: <code>`. But it must copy node-redis's grammar (`redis:`, `rediss:`,
  the `unix:` form, a numeric db path, percent-decoding). A copy that is too strict refuses a valid
  URL and breaks a working deployment. The catch in § 3.1 is enough for G1. Confirmed by D3.
- R2. REJECTED: one shared helper exported from `@paigasus/auth/server` for console-core. It adds a
  public API for a few lines, and the two sites throw different error classes on purpose (§ 3.2).
- R3. REJECTED: keep the original error as `cause`. The issue forbids it, and `cause` is what
  `util.inspect` prints.
- R4. REJECTED: treat `''` like `undefined` in `runtime.ts` / `discovery.ts:346`. That gives the
  "is required" message, but it needs a second change per site, and `requireRedisUrl` would then
  differ from the `createClient`-site rule. One check at the `createClient` site covers both.
- R5. REJECTED: log any error's `code` in `readiness.runtime_failed`. A foreign `code` is
  uncontrolled text (§ 3.6).

## 6. Tests

### 6.1 Auth — new `ts/packages/paigasus-auth/tests/adapters/redis-store-url.test.ts`

- No `vi.mock('redis')`. The test must use the REAL node-redis parser, because the defect is in its
  error shape. `createClient` throws before any I/O, so no Redis server is needed. Note: the three
  existing files `redis-client-options.test.ts`, `redis-store-connect.test.ts` and
  `redis-store-parse.test.ts` in the same directory DO mock `redis`; do not copy their setup.
- A shared table of the eight URLs of § 1.1. Each has the sentinel `SMA715SENTINELPW` in the
  password position.
- A helper `collectText(err)` walks the error per AC3 and returns every string it finds. The test
  asserts that no string contains the sentinel.
- Helper controls, one per channel. Each is a synthetic `Error` with the sentinel ONLY in:
  (a) a non-enumerable own property, set with `Object.defineProperty`;
  (b) `cause`;
  (c) a nested `cause` (`cause.cause`);
  (d) an enumerable own property.
  The helper must find the sentinel in each one. One more row applies the helper to the RAW
  node-redis error for `redis//u:<sentinel>@h` and must find the sentinel (that is channel (d) in a
  real error).
- A valid-URL control row: `createClient` with `redis://127.0.0.1:1` and the production options
  (through `createRedisSessionStore` with the real client, or a direct call with the same options)
  does not throw. Close or discard the client without I/O.
- One case per AC4 through `createAuthRuntime`. `tests/runtime.test.ts`'s `BASE` is not exported.
  Copy the `CONFIG` fixture of `tests/runtime-redis-wiring.test.ts:27-45`, which already has the
  redis store selected, and set the malformed URL. Do NOT copy that file's `vi.mock('redis')`.
- AC8 rows (D5): `''`, `'   '`, `'\t\n'` → `AuthConfigError` with the fixed message, and AC3 holds.
  The `''` row must prove that no client was built and no connect was tried: `createRedisSessionStore`
  rejects before any I/O (for example, it rejects synchronously-fast, well under the connect
  timeout, and no socket to `localhost:6379` is opened). One `createAuthRuntime` row with `''`. The
  control `' redis://127.0.0.1:1 '` does not throw at `createClient` (same shape as the valid-URL
  control row).

### 6.2 Console-core — new case in `tests/unit/discovery-redis-client.test.ts`

The same table, the same helper (a local copy: the packages do not share test support). For G3:
after the throw, assert that `redisClientFromState()` is `undefined`. Then call
`descriptorCacheFor` with a VALID Redis URL (`redis://127.0.0.1:1`, as the existing options pin
does) and assert that `redisClientFromState()` now holds a client. `resetDiscoveryForTest()` in
`afterEach` already exists.

AC8 rows (D5): `''`, `'   '`, `'\t\n'` → a plain `Error` with the fixed message, AC3 holds, and
`redisClientFromState()` stays `undefined`. Without the check, `''` puts a client in state (this is
what makes the row red, § 6.3 step 6). The control `' redis://127.0.0.1:1 '` puts a client in state.

### 6.3 Readiness — `ts/packages/paigasus-auth/tests/http/readiness.test.ts` (D4)

- The existing real cross-field test (line 255-260) expects
  `{ error: 'AuthConfigError', code: 'auth_config_invalid' }`.
- T11 and T11b (lines 217-239) keep `{ error: 'TypeError' }` with NO `code` key, although the
  fixture error has `code: 'ERR_INVALID_URL'` (`toEqual` already fails on an extra key).
- A new row: the getter rejects with the real SMA-715 error (a `createAuthRuntime` with
  `PAIGASUS_SESSION_REDIS_URL: 'redis//u:<sentinel>@h'`) → one event
  `{ error: 'AuthConfigError', code: 'auth_config_invalid' }`, and the sentinel is in no body,
  header or logged field.

### 6.4 Proof that the tests bite

Per the memory notes "red-first is not proof — delete the feature" and "a mutation must compile":

1. Remove the try/catch in `redis-store.ts` (keep a compiling file). The § 6.1 table must red on the
   two `ERR_INVALID_URL` rows at least. Restore by deleting the mutation, not by `git checkout`.
2. Put the raw error back into the thrown error. `AuthConfigError`'s constructor takes only a
   message since SMA-705, so `new AuthConfigError('…', { cause: e })` does not compile. Use a
   compiling form, for example `catch (e) { throw Object.assign(new AuthConfigError('…'), { cause: e }) }`.
   AC3 must red on the `cause` walk.
3. Remove the try/catch in `discovery.ts`. § 6.2 must red.
4. Remove one channel at a time from the helper, and record which § 6.1 helper control fails:
   - skip non-enumerable properties (for example `Object.keys` in place of
     `Object.getOwnPropertyNames`, and no `showHidden`): control (a) must red;
   - skip `cause`: controls (b) and (c) must red;
   - walk only one level of `cause`: control (c) must red;
   - skip own property values: control (d) and the raw node-redis row must red.
   Do not expect the raw node-redis row to red when `showHidden` is removed: `input` is enumerable
   (§ 1.1), so the default `util.inspect` still prints it.
5. Make the auth catch unconditional on the valid URL too (for example, throw from the try after
   `createClient`). The valid-URL control row must red.
6. (D5) Remove the empty check at each site, one site at a time. The `''` row of that site must
   red (auth: no rejection, or no `AuthConfigError`; console-core: no throw and a client in state).
   EXPECTED and measured-by-design: the `'   '` and `'\t\n'` rows stay GREEN with only the check
   removed, because node-redis throws `ERR_INVALID_URL` for them and the parse catch maps that to
   the same fixed error (§ 1.2). They red only when BOTH the check and the parse catch are removed.
   Record that combined mutation too. Then make the check refuse any value with outer whitespace
   (for example `url !== url.trim() || url === ''`): the `' redis://127.0.0.1:1 '` control must red.
7. (D4) Remove the `code` field from `readiness.ts`: the cross-field test and the new SMA-715 row
   must red. Log `code` from any error with a string `code`: T11 and T11b must red.

Record each result in the PR body.

## 7. Files

- `ts/packages/paigasus-auth/src/adapters/redis-store.ts` — the empty check and the catch (§ 3.1, § 3.5).
- `ts/packages/paigasus-auth/src/http/readiness.ts` — the `code` field (§ 3.6).
- `ts/packages/paigasus-auth/src/ports/logger.ts` — the comment on `readiness.runtime_failed` (§ 3.6).
- `ts/packages/paigasus-auth/tests/adapters/redis-store-url.test.ts` — new (§ 6.1).
- `ts/packages/paigasus-auth/tests/adapters/redis-client-options.test.ts` — the tls pin (§ 3.4).
- `ts/packages/paigasus-auth/tests/http/readiness.test.ts` — the `code` expectations (§ 6.3).
- `ts/packages/paigasus-auth/README.md` — Redaction and readiness-route text (§ 3.3).
- `ts/packages/paigasus-console-core/src/discovery.ts` — the empty check and the catch (§ 3.2, § 3.5).
- `ts/packages/paigasus-console-core/tests/unit/discovery-redis-client.test.ts` — new cases (§ 6.2).
- `ts/packages/paigasus-console-core/README.md` — the same text (§ 3.3).
- `docs/superpowers/specs/2026-09-27-sma-705-console-oidc-readiness-design.md` — one sentence in
  § 10 (§ 3.6).

No change to `config.ts`, `runtime.ts`, the chart or the apps.

## 8. Verification

- `moon run paigasus-auth-ts:test paigasus-console-core-ts:test` (check the exact project ids with
  `moon query projects`), plus `:lint`, `:typecheck` and `ts:fmt`.
- Before the push: the full gate graph of the root `CLAUDE.md`.
- Docker is necessary. A `src/` edit in console-core selects `paigasus-console-core-ts:test-e2e`,
  which needs Docker and has no skip hatch (`ts/CLAUDE.md`). Start Docker before the full graph.

## 9. Size

Small to medium: two sites of about ten lines each, a two-line change in `readiness.ts`, three test
files, README text in two packages, one sentence in the SMA-705 spec.

## 10. Open questions (all answered by Sven on 2026-09-27)

- Q1. Does Sven also want the early zod refine (R1) on `PAIGASUS_SESSION_REDIS_URL`, as a separate
  issue? It fails the config parse at startup with `<key>: <code>`, but it risks refusing a valid
  node-redis URL form. This spec does not do it.
  **ANSWER: No refine, and no follow-up issue (D3).**
- Q2. The issue names only `redis-store.ts`. This spec adds the console-core site (§ 1.4), which
  has the same leak from the same variable. The assumption is that Sven wants both fixed in one PR.
  If he wants a separate issue for console-core, § 3.2 and § 6.2 move out.
  **ANSWER: The console-core site is in this PR (D6).**
- Q3. SMA-705 is not merged. When it merges, its `/readyz` catch and this fix overlap on the auth
  path. No code conflict is expected (different files), but the SMA-705 spec § 10 residual that
  names SMA-715 can then be closed. Which PR updates that text depends on merge order.
  Added by the challenge: `AuthConfigError` does not set `name`, so its `name` is `Error`. The
  SMA-705 `/readyz` catch logs only `name`, so after this fix it logs `Error` where before it
  logged `TypeError`. Should SMA-705 log `code` (`auth_config_invalid`) instead? This spec does not
  change `AuthConfigError` or SMA-705.
  **ANSWER: This PR updates the SMA-705 residual text, and the `/readyz` catch logs `code` (D4).**
  SMA-705 is now merged on `main` (`83d446fc`), so the code change is in this PR (§ 3.6). The
  challenge's `name` premise is stale: SMA-705 made `AuthConfigError` set
  `name = 'AuthConfigError'`, so the route logs `AuthConfigError`, not `Error`.
- Q4. `PAIGASUS_SESSION_REDIS_URL=""` passes both `=== undefined` checks. node-redis skips all
  parsing when `options.url` is falsy, so the client connects to `localhost:6379` with no error.
  The chart maps the value from a Secret key, so an empty key gives exactly this value. Is this in
  scope, or a separate issue? This spec assumed a separate issue.
  **ANSWER (CHANGED): In scope. Refuse an empty or whitespace-only value at both sites with the
  same fixed, secret-free error, with tests that fail when the check is removed (D5, § 3.5,
  AC8, § 6.4 step 6).**

## 11. Challenge changelog

Spec-challenger verdict: **APPROVE WITH CHANGES** (no BLOCKER, two MAJOR, seven MINOR, three
QUESTIONS). Each finding was checked against `main` at `ad1c44b7` and the installed
`@redis/client` 6.2.1.

Folded:

- MAJOR, proof-of-bite step 4 cannot red. Confirmed: `input` is enumerable
  (`getOwnPropertyDescriptor(...).enumerable === true`, measured). § 6.1 now has one synthetic
  helper control per channel (non-enumerable, `cause`, nested `cause`, enumerable). § 6.4 step 4 now
  removes one channel at a time and names the control that must red. § 1.1 records that `input` is
  enumerable.
- MAJOR, console-core `tls` pin fails on unchanged code. Confirmed at `index.js:163`
  (`Object.assign(options.socket, parsed.socket)`) and in `tests/support/discovery-state.ts`, which
  reads the resolved options. § 3.4 and AC5 keep the pin in the auth test only.
- MINOR, `let ... ReturnType<typeof createClient>` does not compile. Confirmed by the comment at
  `redis-store.ts:97-102`. § 3.1 now specifies a local `buildClient` function. § 3.2 uses
  `RedisClientType`, which line 313 already uses.
- MINOR, the catch gives the wrong cause for any other constructor error. Folded as a valid-URL
  control row with a REAL client (§ 6.1, AC1, § 6.4 step 5). The fixed message stays as it is (D2).
- MINOR, the G3 check cannot fail. § 6.2 and AC2 now assert that `redisClientFromState()` is
  `undefined` after the throw, and that a valid URL then puts a client in state.
- MINOR, two throw paths missing. Measured: `Invalid db query parameter` and `URIError: URI
  malformed` from a `unix:` path. Both rows are in § 1.1, so AC1/AC2 cover them.
- MINOR, README in the wrong package and gives no fix. § 3.3 now edits both READMEs and lists the
  usual causes.
- MINOR, § 8 does not say that Docker is necessary. Added.
- MINOR, AC4 depends on a fixture that is not exported. § 6.1 now copies the `CONFIG` fixture of
  `tests/runtime-redis-wiring.test.ts`, without its `vi.mock('redis')`.
- QUESTIONS: the `name` question is added to Q3. The empty-string question is Q4. The console-core
  scope question is Q2, unchanged.

Rejected: none.

## 12. Changes at approval and rebase (2026-09-27)

Approval decisions applied (§ 5 D3 to D6, § 10 answers):

- Q1 → D3: no zod refine, no follow-up issue.
- Q2 → D6: the console-core site stays in this PR.
- Q3 → D4: new § 3.6 (the readiness event logs an `AuthError`'s `code`), G5, AC9, AC10, § 6.3,
  § 6.4 step 7, the SMA-705 spec § 10 sentence, and the readiness-route README text in § 3.3.
- Q4 → D5 (scope extended): new § 1.2 (measured), § 3.5, G4, AC8, R4, the AC8 rows of § 6.1 and
  § 6.2, and § 6.4 step 6. The Linear issue description got a "Scope extended on 2026-09-27"
  section.

Checked against `origin/main` at `4051df5e` (the spec was written at `ad1c44b7`; SMA-705 PR 331
and SMA-695 PR 332 merged in between). SMA-695 (optional chart Ingress) touches nothing here.
Stale items fixed:

- SMA-705 is merged, not an open branch. Header, § 1.3 and Q3 updated.
- `AuthConfigError` now sets `name = 'AuthConfigError'` and has a one-argument constructor
  (`core/errors.ts:138-148`). The Q3 challenge premise ("its `name` is `Error`") is stale, and the
  old § 6.3 step 2 mutation `new AuthConfigError('…', { cause: e })` no longer compiles. § 6.4
  step 2 now uses `Object.assign(..., { cause: e })`.
- `runtime.ts` moved by two lines: `getAuthRuntime`'s slot delete is at `runtime.ts:265-277`
  (was `263-274`), and the `=== undefined` check is at `runtime.ts:101` (was `99`).
- `oidc.ts` `new URL(opts.issuer)` is at line 351 (was 336).
- `config.ts` `z.url()` override is at line 37 (was 36).
- `apps/{iam,gateway}-console/lib/config.ts`: `URL.canParse` is at line 18, `new URL` at 19 and 42
  (was cited as 19, 42).
- `tests/runtime-redis-wiring.test.ts`'s `CONFIG` fixture is at lines 27-45 (was 25-44).
- The auth README "Redaction" heading is at line 151 (was 150). The README now also has the SMA-705
  readiness section (line 260), which names the `TypeError`/`input` leak. § 3.3 now updates it.
- The old § 1.3 (other URL parse sites) is now § 1.4, and the old § 6.3 (proof-of-bite) is now
  § 6.4, because § 1.2 and § 6.3 are new.

Unchanged and re-verified: `config.ts:98`, `redis-store.ts:97-102` and `:287`, `discovery.ts:312-321`,
`:346-347`, console-core `src/runtime.ts:108`, `core/errors.ts:138`, `paigasus-discovery/src/config.ts:48`.
