# SMA-677: a rate limit and a token budget for gateway chat completions

- Linear: SMA-677 (milestone "Gateway", labels `area:gateway`, Feature)
- Date: 2026-09-27
- Packages: `rs/crates/libs/paigasus-redis` (new), `rs/crates/libs/paigasus-test-docker` (new),
  `rs/crates/services/paigasus-gateway`, `rs/crates/services/paigasus-iam`,
  `rs/crates/libs/paigasus-observability`, `rs/crates/libs/paigasus-proto`,
  `contracts/proto/paigasus/common/v1/error.proto` (and its generated bindings),
  `ts/packages/paigasus-sdk`, `ts/packages/paigasus-proto`, `ts/apps/gateway-console`,
  `ts/apps/iam-console`, `ci/error-registry`, `ci/affected-graph`, the root `moon.yml`,
  `rs/.config/nextest.toml`, `ops/observability/prometheus`, `charts/paigasus`, `docs/ops`
- Related: SMA-635 (interactive-user auth, spec
  `docs/superpowers/specs/2026-09-23-sma-635-gateway-user-auth-design.md` § 9 risk "Spend"),
  ADR-0023 (interactive-user auth on the gateway chat surface; it names SMA-677 as the owner of the
  limit), the Notion scoping page "AI Gateway" (§ 6.3, § 6.4, § 11 "Rate-limit algorithm"),
  SMA-504 (the canonical error model), SMA-446 (the metric-name registry), SMA-473 and SMA-476
  (the IAM Redis connection budget and circuit breaker that this issue moves into a lib crate),
  SMA-538 (the Docker-skip policy that this issue moves into a lib crate)
- Revision 5 (2026-10-02): the spec-challenger's round 2 findings on revision 4 are folded (verdict
  APPROVE WITH CHANGES; the coordinator accepted every finding). The main changes: the budget key
  gets a relative TTL (D18); the domain error no longer depends on `redis`, and the application
  service moves to `src/application/` (D10, § 4.2-§ 4.3a); Redis durability and eviction rules
  (D21); fail-open alert rules (§ 4.8); a shared Docker test crate (D22); the config keys are
  renamed to `tokens_per_period` and `budget_period`. See "Challenge changelog — round 2
  (revision 4)". Revision 4 folded Sven's decisions of 2026-10-02 (see "Revision 4 changelog").
  All file:line citations were checked on `main` at 4face4af.
- **Approved by Sven on 2026-10-02 (GATE 1)**, together with the § 11 recommendations for Q14 to
  Q20. Sven did not answer Q3, Q5, Q8, Q10, Q11 and Q12 one by one. The plan uses their § 11
  recommendations and names each one, so Sven can change it at plan review.

## 1. The problem

`POST /v1/chat/completions` has no rate limit and no spend limit. The config example says so
(`rs/crates/services/paigasus-gateway/gateway.toml.example:4-6`): "only deploy this behind a hard
OpenAI spend cap". SMA-635 opened the chat surface to every person who holds `gateway_user`,
through the console playground. Today only an org admin's out-of-band role grant controls who can
spend model tokens.

One caller can therefore:

- send requests as fast as the upstream accepts them, and use the shared OpenAI rate limit of
  every other tenant;
- spend tokens with no ceiling. The operator sees the cost only on the OpenAI invoice.

The request path today (`src/adapters/http/chat.rs`): the auth middleware attaches a
`CallerContext { principal_prn, scope_prn, credential }`. The handler parses a copy of the body to
read `model` and `stream`, checks the streaming toggle, and forwards the raw bytes. No state
survives the request.

## 2. Acceptance

- A1. With `[limits]` configured, a principal whose D3 estimate for the current 60 s window is at
  or above `principal_requests_per_minute` gets `429` with `code: "rate-limited"`, a `Retry-After`
  header, `paigasus-retryable: true` and `x-should-retry: true`. No upstream call is made. (The
  D3 estimate prevents an instant 2x burst at a window edge. It does not guarantee that every
  rolling 60 s span holds at most the limit; see D3.)
- A2. The same holds per organization with `org_requests_per_minute`, summed over every principal
  of that org.
- A3. An org whose token use in the current budget period is at or above `tokens_per_period` gets
  `429` with `code: "budget-exhausted"`, `paigasus-retryable: false` and `x-should-retry: false`.
  No upstream call is made.
- A4. Every admitted request is charged at most once, by one charge guard (§ 4.6), on both the
  stream and the non-stream path:
  - a `2xx` answer with a usage record → its `usage.total_tokens` (`source="reported"`);
  - a `2xx` answer without usage, a timeout, a transport failure after the request was sent, or a
    client disconnect → the D14 estimate (`source="estimated"`);
  - a connect failure (`OpenAiError::Connect`) and a non-`2xx` upstream answer → zero.
  - A request admitted under fail-open (A12) has no ticket and is not charged. A charge that the
    store cannot record is counted (§ 4.8) and is not retried (D10).
- A5. The response body and the SSE bytes that the client receives are byte-identical to the
  bytes without limits. The limit code reads a copy; it never changes what is forwarded. (Only the
  two refusals add headers, and only on the refusal response.)
- A6. Without a `[limits]` table, the gateway behaves exactly as today: the same statuses, headers,
  bodies, metrics and log lines. It opens no Redis connection. Existing tests change only where
  they build `AppState` by struct literal (they gain `limits: None`); no assertion changes.
- A7. Metrics (§ 4.8) count every refusal by reason, every charged token by source, every store
  failure by operation and kind, every dropped charge by reason, and the gateway's Redis breaker
  state. The names are in `paigasus_observability::names::ALL`, and each series is primed at zero
  when limits are on, by a library function that `main.rs` and the tests both call. Two alert rules
  (§ 4.8) fire on a fail-open outage, and `promtool` tests prove them.
- A8. Both new codes are declared in `error.proto`, resolve through
  `ErrorReason::from_wire_reason`, are covered by `ci/error-registry/check.py`'s `MANIFEST`, and
  have an entry in the TS SDK's `PRESENTATION` table. `budget-exhausted` maps to the new
  `quota-exhausted` presentation (§ 4.9).
- A9. `gateway.toml.example` documents every new key, the Redis requirements (D21), and the M0
  note at lines 4-6 says what the new limit does and does not cover (§ 6).
- A10. The console playground sends `stream_options: {"include_usage": true}`, so its streams are
  charged from the reported usage.
- A11. With `backend = "redis"`, every gateway replica that points at the same Redis shares one set
  of counts, and a gateway restart does not reset them. The same store contract suite (§ 5.2)
  passes against the memory adapter and against a real Redis.
- A12. With `backend = "redis"`, when Redis does not answer, answers with an error, or the breaker
  is open, the request is admitted (fail-open). The gateway skips the charge, logs (at the D10 levels,
  at most one line per 10 s per operation and kind), and increments
  `gateway_limit_store_unavailable_total{op,kind}`. `/readyz` does not check Redis.
- A13. IAM behaves exactly as before the move to `paigasus-redis` and `paigasus-test-docker`.
  Every existing IAM test passes with no assertion change, and `iam_redis_breaker_state` and
  `iam_redis_breaker_transitions_total` keep their names, labels and values byte-identical.
- A14. `repo:redis-connect-single-site` and `repo:iam-docker-policy-single-site` keep their ids.
  Their single allowed sites are in `paigasus-redis` and `paigasus-test-docker`, and they scan IAM,
  the gateway and the lib. A banned constructor, or a hand-rolled Docker-skip policy, in the
  gateway makes the matching gate red.

## 3. Decisions

D1. **The store is a port with two adapters: memory and Redis.** A `LimitStore` trait lives in the
domain layer. This issue ships `MemoryLimitStore` and `RedisLimitStore`. `[limits] backend`
selects one (D17). The memory adapter is for development and a single replica. The Redis adapter
is for every deployment with more than one replica, or that needs the counts to survive a gateway
restart. Sven decided on 2026-10-02 that the Redis store is in this issue (answered Q1).

Three approaches were considered:

| Approach | For | Against |
|---|---|---|
| A. Port + in-process store only | No new infrastructure. Small. | The limits are per replica: with `gateway.backend.replicas: 2` (`charts/paigasus/values.yaml:107`) the effective limit is two times the configured one. A restart resets the budget. Rejected by Sven (Q1). |
| **B. Port + memory and Redis adapters, with a shared Redis lib (chosen)** | Shared across replicas; survives a gateway restart. IAM's tuned connection and breaker are reused, not copied (D15). | IAM's Redis code and Docker test policy move into lib crates, so this issue touches IAM and two CI gates. The issue is large; see § 12 "Delivery shape". |
| C. Ask IAM to keep the counters | One owner of tenancy state. | A new gRPC RPC on every chat request, and the Notion "IAM" page says spend and budget logic stays in the gateway. |

D2. **The org of a request.** For an OIDC caller, `scope_prn` is already the resolved org PRN
(SMA-635). For an API key, `scope_prn` is the key's own scope. `ApiKey.scope` is a
`TenancyNodeRef` (`paigasus-iam-core/src/api_key.rs:84`), which is only `Organization`, `Team`
or `Project` (`tenancy.rs:141-145`). So a valid request always has an org. The org is found with
the same rule that `domain::resolve_org` uses today (`org_of`). `org_of` stays in
`src/domain/mod.rs` as `pub(crate)`, so `resolve_org` does not depend on the limits module.

A scope that names no org is therefore a parse mismatch or an IAM defect, not a normal case. The
gateway logs it at `warn` and counts it in `gateway_limit_unscoped_requests_total`. When the
policy has an org rate or a token budget configured, it refuses the request with `500 internal`
(fail-closed, Q11). When only a principal rate is configured, it applies that limit and admits the
request. This fail-closed rule is about a malformed request, not about the store; D10's fail-open
rule does not change it.

D3. **The rate-limit algorithm is a sliding-window counter over 60 s.** It keeps two fixed-window
counts (the current and the previous minute) per key. The estimate is
`previous × (1 − elapsed_in_current / 60) + current`. A request is admitted when
`estimate + 1 ≤ limit`. It uses O(1) memory per key. It prevents the instant 2x burst at a window
edge that a plain fixed window allows. It does not bound every rolling 60 s span to the limit: if
the previous window's requests cluster at its end, a rolling minute can hold nearly 2x the limit.
The estimator, not a rolling count, is the contract (A1).

Both adapters compute the estimate in the same integer form, so they agree to the request:
`admit ⇔ previous × (60000 − elapsed_ms) + current × 60000 + 60000 ≤ limit × 60000`, with
`elapsed_ms` in `0..60000`. `SlidingWindow::admits` in the domain and the Lua script (D18) both use
this form. The contract suite (§ 5.2) catches a difference between them. D23 bounds every count,
so every product stays below 2^53 and Lua's double-precision numbers hold it exactly.

A token bucket (GCRA) was considered. On Redis both algorithms need one Lua script for D4's
"check every key, then commit all or nothing", so the Redis cost does not decide. The sliding
window stays because its state (two integers per key) is simple to reason about and to test with
fixed instants. Sven decided on 2026-10-02 to keep the sliding window (answered Q2). The root
`CLAUDE.md` says significant choices get a Notion ADR before code. The ADR (to be written, before
the plan) records the algorithm, the Redis store and the fail-open policy. Q18 recommends that it
also records the `paigasus-redis` extraction.

D4. **The principal limit, the org limit and the budget are checked together, and then
committed.** If any check fails, no count grows. So a request refused by the org limit does not
use the principal's quota, and the reverse. In the memory store, `SlidingWindow` exposes
`check(now, limit) -> Result<(), RetryAfterSecs>` (pure) and `commit(now)` (mutates), called under
one lock. In the Redis store, one Lua script reads every key, decides, and increments the rate keys
only when every check passes (D18). Redis runs a script atomically, so two replicas cannot both
take the last slot.

D5. **The budget is in tokens, not in currency.** A cost needs a price table per model, and the
gateway has one upstream and no price data. `usage.total_tokens` is what OpenAI reports and what
an operator can compare with the OpenAI usage page. Other gateways also count tokens (Kong's
ai-rate-limiting-advanced, Envoy AI Gateway; Portkey offers tokens or currency). A cost budget is
out of scope (§ 10, Q3).

D6. **The budget period is a UTC calendar period: `daily`, `weekly` or `monthly`. The default is
`monthly`.** Sven decided on 2026-10-02 (answered Q9). The config key is `budget_period`, and the
budget value is `tokens_per_period`. (Revision 4 called them `budget_window` and
`tokens_per_window`. "Window" is the 60 s rate window of D3, so the budget keys now say "period".)

- `daily` starts at 00:00:00 UTC. Its key is the date, for example `d2026-10-02`.
- `weekly` is the ISO 8601 week. It starts on Monday at 00:00:00 UTC. Its key is the ISO
  week-year and week, for example `w2026-W40`. (The ISO week-year differs from the calendar year
  near 1 January, so the key uses `chrono`'s `iso_week()`, not the calendar year.)
- `monthly` starts on the first day of the month at 00:00:00 UTC. Its key is the month, for
  example `m2026-10`.

The period key comes from the request instant alone, so every replica agrees on it with no
coordination. On Redis, each org and period has its own key with a TTL (D18). A new period is a
new key that starts at zero, so no reset job runs and a reset cannot lag. (LiteLLM resets budgets
with a background job that runs about every 10 minutes, so its resets can lag; this design has no
such job.) `monthly` is the default because it matches the period of an OpenAI invoice and of
OpenAI's own spend limit. With the memory backend, a restart still resets the use, and a
`monthly` budget is then not an invoice cap (§ 6). A per-org time zone is a follow-up (§ 10).

D7. **The budget is checked before the request and charged after it.** The gateway cannot know the
token count of a request before the upstream answers. So a request is admitted when the used
tokens are below the budget, and its real use is added when the charge guard drops. Concurrent
requests can therefore overshoot the budget by the use of the requests in flight. The overshoot is
bounded by about "org request rate × longest request duration × tokens per request". This is a
documented soft edge (§ 6). Envoy AI Gateway charges after the response in the same way.
`max_tokens` reservation was considered and rejected: most requests do not send it, and a
reservation of the model maximum would refuse legal requests near the end of a period. A per-org
cap on requests in flight would bound the overshoot more tightly; Q12 asks for it.

A charge whose ticket's budget period has already ended (a request that started at 23:59:59 and
ended after midnight) is added to the period in its ticket, not to the current one. On Redis, the
ticket carries the budget key itself, and that key lives until one day after its period ends
(D18), so a late charge still lands. A charge that arrives more than one day after its period ends
is skipped and counted (D18). In the memory store, if the sweep already evicted that period, the
charge is dropped and still counted in `gateway_tokens_charged_total`. This keeps a period's total
stable after it ends.

D8. **Status codes and codes.**

| Case | Status | `type` | `code` | `param` | `paigasus-retryable` | Extra headers |
|---|---|---|---|---|---|---|
| Principal or org request rate | 429 | `requests` | `rate-limited` | none | `true` | `Retry-After: <seconds>`, `x-should-retry: true` |
| Org token budget | 429 | `insufficient_quota` | `budget-exhausted` | none | `false` | `x-should-retry: false` |

429 is what the OpenAI SDKs map to `RateLimitError`, so an unchanged SDK shows a sensible error.
The OpenAI Python and Node SDKs retry a 429 twice by default unless the response has
`x-should-retry: false`, and they ignore `paigasus-retryable`. So the budget refusal sends
`x-should-retry: false`: a retry in seconds cannot succeed, and each retry costs 2-3 IAM RPCs
before the refusal. The rate refusal sends `x-should-retry: true`, which only confirms the SDK
default. OpenAI answers a quota exhaustion with `429` and `type: "insufficient_quota"`, so the
budget case uses that `type`. OpenAI's own 429 rate bodies use `type: "requests"` (or `"tokens"`)
with `code: "rate_limit_exceeded"`. This spec uses `type: "requests"`. The `code` stays the
Paigasus registry code. The SDKs choose the exception class from the status, so the `type` has low
impact. The implementer confirms the current OpenAI `type` value against the OpenAI API reference
before the code is written (Q8). (LiteLLM answers a budget refusal with 400, 401 or 422, depending
on the document. This spec does not copy that.)

The message says which period is used up and when it resets (for example "The organization token
budget for 2026-10 is used up. It resets at 2026-11-01T00:00:00Z."). The message carries no count
and no org id. `GatewayError::BudgetExhausted` carries `resets_at_unix: i64` and
`period: BudgetPeriod` (`Copy + Default`, D6). The label ("2026-10-02", "2026-W40" or "2026-10") is
formatted from the period kind and the instant one second before the reset. Both codes are gateway
codes, so they take the next numbers in the gateway range of `error.proto`:
`ERROR_REASON_RATE_LIMITED = 311`, `ERROR_REASON_BUDGET_EXHAUSTED = 312`.

**Precedence.** When more than one check fails, the refusal reason is chosen in this order: the org
budget, then the principal rate, then the org rate. The budget comes first because a
`retryable: true` answer to a request that cannot succeed for hours misleads the client. The
metric label follows the chosen reason, and only one refusal is counted. `Retry-After` is
`max(ceil(retry_after))` over every failed rate check, with a minimum of 1 second. The Lua script
returns every failed check with its counts (D18), and the application service applies this
precedence in Rust for both adapters.

D9. **The limit runs in the handler, after the body is parsed and after the streaming check, just
before egress.** The streaming check is `chat.rs:109-111` and the egress call is `chat.rs:115`. A
request refused for a bad body or for `stream` does not use quota, which matches the rule in
`chat.rs:106-108` ("a refused request never reaches the upstream or its rate limit"). It runs after
authentication, so an unauthenticated flood cannot fill the per-principal state (§ 6 has the
residual). It is not a tower layer: it needs `CallerContext`, and the handler already owns the
order of the refusals. The handler calls one injected application service (§ 4.3a); it does not
run the org resolution, the policy and the store itself.

D10. **Store failure policy: fail-open.** Sven decided on 2026-10-02 (answered Q4). The port
returns `Result<_, LimitStoreError>`. The domain error does not name `redis`:

```rust
pub enum LimitStoreError { Unavailable { kind: UnavailableKind } }
pub enum UnavailableKind { Io, Server, Decode }   // Copy; bounded metric label
```

`src/adapters/limits/redis.rs` maps a `redis::RedisError` to it. `Io`: `err.is_io_error()`, which
includes the breaker's short-circuit (an `ErrorKind::Io` error, `redis_conn.rs:494-496`) and every
connection failure. `Decode`: a reply of the wrong type (`UnexpectedReturnType`, `Parse`). `Server`:
every other error, for example an `OOM` refusal under `noeviction` (D21) or a script error. The
memory adapter never returns an error.

- **Check fails.** The application service admits the request with no ticket. The charge guard
  then charges nothing. The service increments
  `gateway_limit_store_unavailable_total{op="check",kind}` and logs.
- **Charge fails.** The charge task (D16) increments
  `gateway_limit_store_unavailable_total{op="charge",kind}` and logs. It does not retry. A retry
  would likely fail again while Redis is down, and the breaker short-circuits it anyway. A retry
  after an ambiguous failure (the `INCRBY` ran but the answer was lost to the 500 ms response
  timeout, `redis_conn.rs:19-20`) would also charge twice. One lost charge under-counts; a double
  charge refuses a paying org early. Under-counting is the smaller fault.
- **Log levels.** `kind = Io` logs at `warn`: Redis is down, and fail-open is the designed answer.
  `kind = Server` and `kind = Decode` log at `error`: Redis answered, so the fault is a
  misconfiguration (for example a full `noeviction` instance) or a defect, and fail-open hides it
  unless someone looks. Both are rate-limited: at most one line per (operation, kind) per 10 s,
  the interval of IAM's `LOG_RATE_LIMIT_INTERVAL`
  (`paigasus-iam/src/application/log_rate_limit.rs:11`). IAM's `LogRateLimiter` is `pub(crate)`,
  so the gateway holds its own small limiter with the same policy: a suppressed event is still
  counted by the metric. The line carries the kind and the error text, never the Redis URL.
  (`BREAKER_OPEN_MESSAGE` is free of any URL, `redis_conn.rs:199-202`.)
- **The spend risk** is bounded by the outage. While Redis is down, no rate limit and no budget
  applies, and the tokens used in that time are not counted against any budget. The worst case is
  "the upstream's own rate limit × the outage duration". § 6 states this. The two alert rules in
  § 4.8 page the operator.

Fail-closed (`503 limits-unavailable`) was considered and rejected by Sven: it turns a Redis outage
into a chat outage. The gateway's `/readyz` does not check Redis (A12). It checks IAM only
(`src/adapters/http/mod.rs:143-155`). A Redis check there would take every replica out of the
balancer during a Redis outage, which is the fail-closed outcome that D10 rejects. IAM also does
not check Redis for readiness: its `/readyz` pings the database only
(`paigasus-iam/src/adapters/http/mod.rs:1016-1041`).

`MemoryLimitStore` must not stop the service when an edge case occurs. Rules:

- A poisoned `std::sync::Mutex` is recovered with `PoisonError::into_inner`. The state is still
  consistent, because every update is a whole-integer write.
- All counter arithmetic uses saturating operations. No `u64` overflow can panic.
- A backward clock step makes the elapsed time negative. It is clamped to zero
  (`duration_since(..).unwrap_or(Duration::ZERO)`).
- `Drop` of the charge guard never calls `unwrap` or `expect`, and never panics. A panic during
  unwinding aborts the process.

D11. **Limits are off unless configured.** No `[limits]` table means no limit (A6). An existing
deployment keeps its behavior. A `[limits]` table with a key unset means "no limit on that
dimension". A value of `0` is rejected by `GatewayConfig::validate` (it would refuse every request,
which is never what an operator means; use the role grant to stop a principal). Q5 asks whether the
default should instead be "on, with conservative values".

An org is exempt from a table default when its `[[limits.org]]` entry sets `exempt = true`. An
exempt org gets no org rate and no budget; the principal rate still applies. `exempt = true`
together with a value for `org_requests_per_minute` or `tokens_per_period` is a validation error.

**An empty policy skips the store.** The resolved `LimitPolicy` of one request is empty when it has
no principal rate, no org rate and no budget. An exempt org with no principal rate gives one. The
application service then returns a guard with no ticket and makes no store call. When the config
can only give empty policies (no table default set and no `[[limits.org]]` entry sets a value),
`main.rs` builds no store and opens no Redis connection, and it logs one `info` line that says so.

D12. **Per-org overrides by org UUID.** `[[limits.org]]` entries set `org_requests_per_minute`,
`tokens_per_period` and `exempt` for one org id. An unlisted org gets the table defaults. The
override list is read once at boot. A change needs a restart, the same as every other gateway
setting today. The overrides are config, not store state: with `backend = "redis"`, every replica
must carry the same `[[limits.org]]` list, or two replicas apply different limits to one shared
count. figment's environment provider cannot express an array of tables, so `[[limits.org]]` is
set in the TOML file only. The scalar `[limits]` keys work through `GATEWAY_LIMITS__*` environment
variables in the usual way. The example config says this. A runtime API to set a budget is out of
scope (§ 10).

D13. **Memory bound.** In the memory store, the principal map and the org map evict entries whose
last touch is older than two rate windows (principal) or older than the current budget period
(org), in a periodic sweep on the check path (every 1024 checks, not a background task). The
number of principals is bounded by authenticated callers, so this bound is enough for one process.
In the Redis store, every key has a TTL (D18), so Redis evicts them with no sweep.

D14. **Stream estimate.** The estimate is `request_tokens_estimate + completion_tokens_estimate`:

- `request_tokens_estimate = ceil(text_bytes / 4)`, where `text_bytes` is the summed UTF-8 length
  of every string `content` value and every `text` part in `messages`, read from the body copy
  that the handler already parses. Image parts, audio parts and other non-text parts count a fixed
  85 tokens each (OpenAI's low-detail image cost), not their base64 length. When the body does not
  have this shape, the fallback is `ceil(body_bytes / 4)`, capped at 32768.
- `completion_tokens_estimate` for a stream = the number of complete `data:` records forwarded,
  excluding `[DONE]` and excluding a record that holds a usage object. An OpenAI chunk usually
  carries one token, so one record counts as one token.
- `completion_tokens_estimate` for a non-stream `2xx` body without usage = `ceil(len / 4)` of the
  summed `choices[].message.content` strings and tool-call `arguments` strings.
- For a timeout, a transport failure after send, or a disconnect before any answer, the completion
  estimate is zero, so only the request estimate is charged.

Revision 1 used `ceil(forwarded_stream_bytes / 8)`. A real OpenAI chunk is about 200-260 bytes and
usually carries one token, so that formula charged each token about 25-32 times. It is withdrawn.

D15. **A new lib crate `paigasus-redis` owns every Redis connection.** Sven decided on 2026-10-02.
IAM's `src/adapters/redis_conn.rs` (1250 lines with its tests; `pub(crate) mod redis_conn` at
`adapters/mod.rs:17`) moves to `rs/crates/libs/paigasus-redis/src/`. IAM and the gateway both use
it. What moves:

- `connection_manager_config()` (`redis_conn.rs:47`), with `CONNECT_RETRIES = 1` (`:34`) and
  `RETRY_MAX_DELAY = 500 ms` (`:40`);
- `RedisHandle` (`:77`) and its `ConnectionLike` impl (`:82-112`);
- `connect` (`:130`), which is eager (`:117-129`);
- the SMA-476 breaker (`:164-497`): `FAILURE_THRESHOLD = 3` (`:167`), `OPEN_DURATION = 2 s`
  (`:193`), `HALF_OPEN_DEADLINE = 5 s` (`:197`), `BREAKER_OPEN_MESSAGE` (`:202`), `Breaker`,
  `Admission`, `ProbePermit` (`:299-329`) and `counts_as_failure` (`:487-489`);
- the breaker's unit tests (`:498-1120`);
- the test-only constructors `new_lazy_for_tests` (`:144`), `with_open_breaker_for_tests`
  (`:153`) and `Breaker::force_open_for_tests` (`:467`), and the `test_support` blackhole listener
  (`:1122-1250`).

What stays in IAM: `RedisRole { Authz, ApiKeys, Jwks }` (`:207-221`), because the roles are IAM's.

**Metric names come from the caller.** Today the breaker emits `names::IAM_REDIS_BREAKER_STATE`
and `names::IAM_REDIS_BREAKER_TRANSITIONS_TOTAL` itself (`redis_conn.rs:339`, `:462-463`). The lib
cannot name IAM's metrics. So the lib defines
`BreakerMetrics { state: &'static str, transitions: &'static str, role: &'static str }`, and
`connect`, `new_lazy_for_tests` and `with_open_breaker_for_tests` take `impl Into<BreakerMetrics>`.
IAM adds `impl From<RedisRole> for BreakerMetrics`, which uses `names::IAM_REDIS_BREAKER_STATE`
(`names.rs:88`), `names::IAM_REDIS_BREAKER_TRANSITIONS_TOTAL` (`names.rs:96`) and today's
`RedisRole::as_label` (`:214-220`). So every IAM call site that passes `RedisRole::X` keeps its
argument and changes only its import. The lib keeps the label keys `role` and `to` and the label
values `closed`, `half_open` and `open`. So the series that the IAM dashboard
(`ops/observability/grafana/dashboards/iam.json:329`) and the three IAM alert rules
(`ops/observability/prometheus/rules/iam.rules.yml:267`, `:279`, `:289`) read stay byte-identical
(A13). The `'static` lifetime keeps the label set bounded, as `RedisRole` did (SMA-476 D10,
`:205-206`). The lib therefore does not depend on `paigasus-observability`.

**Visibility.** The lib's public surface is `connect`, `RedisHandle`, `BreakerMetrics`,
`BREAKER_OPEN_MESSAGE` and `connection_manager_config`. `Breaker`, `Admission`, `ProbePermit`,
`counts_as_failure` and the three tuning constants are `pub(crate)`; the moved unit tests reach
them from inside the crate. The test-only items (`new_lazy_for_tests`,
`with_open_breaker_for_tests`, `test_support`) are `pub` under
`#[cfg(any(test, feature = "test-support"))]`, the pattern of `paigasus-logging`
(`rs/crates/libs/paigasus-logging/Cargo.toml:14-16`). IAM and the gateway turn the feature on from
`[dev-dependencies]` only, the same way they turn on `paigasus-logging/test-support`
(`paigasus-iam/Cargo.toml:177-180`).

**IAM call sites.** 14 production lines in 6 files name the module or pass a `RedisRole` to it:
`api_keys/cache.rs:46`, `:210`; `oidc/redis_cache.rs:16`, `:41`; `http/mod.rs:70`, `:357`, `:690`,
`:873`; `authz/generation.rs:33`, `:368`; `authz/entity_cache.rs:41`, `:67`;
`authz/decision_cache.rs:31`, `:128` (all under `rs/crates/services/paigasus-iam/src/adapters/`).
Each changes only its import path. Their tests that use the test-only constructors and the
blackhole: `api_keys/cache.rs:390`, `:407-408`; `oidc/redis_cache.rs:101-102`;
`authz/generation.rs:557-558`, `:646`, `:676`, `:716`, `:737`, `:762`, `:788`, `:820`, `:917`;
`authz/entity_cache.rs:255`, `:302-303`; `authz/decision_cache.rs:313-314`. `adapters/mod.rs:17`
loses its `redis_conn` line, and `RedisRole` moves to a small IAM module.

IAM's `breaker_transitions_emit_the_gauge_and_the_counter` test (`redis_conn.rs:1083-1118`) splits
in two. The lib keeps a generic copy that uses test names and drives `Breaker` directly. IAM gets a
name-pin test: for each `RedisRole`, it calls `with_open_breaker_for_tests(url, role)` inside
`metrics::with_local_recorder` and asserts the IAM names and the `role` label. Forcing the breaker
open is a transition, so it emits both series. So a regression of A13 reds an IAM test, and IAM
needs no access to the `pub(crate)` `Breaker`.

Rejected: a second copy of `redis_conn.rs` in the gateway. It duplicates the SMA-473 budget and the
SMA-476 breaker, and the single-site gate cannot then name one site. Rejected: the gateway without
the breaker. A blackholed Redis then adds about 2.1 s to every chat request
(`redis_conn.rs:22-23`).

D16. **The port is async. `charge` is a non-blocking call that the Redis adapter spawns.** This
answers Q6. The port uses `#[async_trait]`, the gateway's existing pattern for a port consumed as
`dyn` (`src/adapters/iam/client.rs:52-53`; the workspace note at `rs/Cargo.toml:102-104` says to
use `async-trait` only when `dyn Trait` is required, which is the case here).

```rust
#[async_trait]
pub trait LimitStore: Send + Sync {
    async fn check_and_admit(&self, principal: &str, org: Option<Uuid>, policy: &LimitPolicy,
                             now: SystemTime) -> Result<LimitDecision, LimitStoreError>;
    /// Must not block and must not panic. An adapter that needs I/O spawns it.
    fn charge(&self, ticket: LimitTicket, tokens: u64, now: SystemTime);
}
```

`charge` stays synchronous because the charge guard calls it from `Drop`, and `Drop` cannot await.
It takes `now` from the guard's injected clock, so the budget TTL (D18) comes from the same clock
as the period key.

- `MemoryLimitStore::charge` updates the map under its lock and returns.
- `RedisLimitStore` has an inherent `async fn apply_charge(&self, ticket, tokens, now) ->
  Result<(), LimitStoreError>` that runs the charge command (D18). `charge` calls
  `tokio::runtime::Handle::try_current()`. With a runtime, it spawns `apply_charge` on that handle
  through a `tokio_util::task::TaskTracker` (`spawn_on`), and the task records a failure per D10.
  With no runtime (a drop in a plain thread), it increments
  `gateway_limit_charges_dropped_total{reason="no_runtime"}` and returns. No repo code spawns from
  `Drop` today, so a test proves this path (§ 5.8).
- The spawned task holds a clone of `RedisHandle`, which is `Clone` and shares one breaker
  (`redis_conn.rs:76-80`, `:259-261`). It does not hold the guard, the stream, or any request
  state, so reqwest's cancel-on-drop (`chat.rs:26-29`) is not changed.
- **Shutdown drain.** `runtime::supervise` (`src/runtime.rs:33`) gains an optional `TaskTracker`.
  After the servers drain, it calls `close()` and awaits `wait()` for at most 5 s. Charges still
  running after 5 s are lost, and their number (`len()`) is added to
  `gateway_limit_charges_dropped_total{reason="shutdown"}`. `tokio-util` 0.7.19 is in
  `rs/Cargo.lock:5560-5561` as a transitive dependency but is not a workspace dependency. The
  gateway adds `tokio-util = { version = "0.7", features = ["rt"] }`. `TaskTracker` needs the `rt`
  feature (`tokio-util-0.7.19/Cargo.toml:76-80`), which adds only `tokio/rt`, `tokio/sync` and
  `futures-util`, all already in the lock. So `repo:deny` and `repo:osv` see no new crate from it.

Rejected: an async `charge` awaited by the guard. `Drop` cannot await. Moving the charge out of
`Drop` would lose the disconnect case, which is the reason the guard exists (A4). Rejected:
spawning in the memory adapter too. It gains nothing.

D17. **`[limits] backend = "memory" | "redis"`, the same pattern as IAM's caches.** IAM's three
caches each have a `backend` enum with `#[serde(rename_all = "lowercase")]`
(`paigasus-iam/src/config.rs:168-174`), an `Option` `redis_url` (`:165`, `:224`, `:345`), and a
validation error when `backend = "redis"` has no URL (`:1056`, `:1091`, `:1152`). The gateway copies
that shape:

- `backend` defaults to `memory`.
- `redis_url` is required when `backend = "redis"` and rejected when `backend = "memory"`. The
  messages: `limits.backend = "redis" requires limits.redis_url` and
  `limits.redis_url is set but limits.backend is "memory"`. The second rule is stricter than IAM's.
  It stops an operator who sets only `GATEWAY_LIMITS__REDIS_URL` from believing the counts are
  shared.
- `rediss://` is recommended. The workspace `redis` dependency already has
  `tokio-rustls-comp` and `tls-rustls-webpki-roots` (`rs/Cargo.toml:210-212`). A plain
  `redis://` URL is accepted, because the kind Redis has no TLS and no password
  (`ci/kind/manifests/redis.yaml:3-4`), but the example config says it is for a private network
  only.
- The URL is redacted in `Debug` and in `Serialize`, because `GatewayConfig` derives both
  (`src/config.rs:17`) and a Redis URL can carry a password. IAM's `RedactedUrl`
  (`paigasus-iam/src/config.rs:63`) does this. The gateway uses its own existing pattern instead:
  `secrecy::SecretString` with `#[serde(skip_serializing)]`, as for `upstream.openai.api_key`
  (`src/config.rs:99-108`). The redaction property is the same. Rejected: moving `RedactedUrl` into
  `paigasus-redis`. IAM also uses it for its Postgres and NATS URLs (`paigasus-iam/src/config.rs:21`,
  `:498`, `:574`), so it does not belong in a Redis crate. Rejected: a third copy of `RedactedUrl`
  in the gateway. This deviates from the wording of Sven's decision 5; Q15 asks him to confirm.
- Environment: `GATEWAY_LIMITS__BACKEND` and `GATEWAY_LIMITS__REDIS_URL`, through the existing
  `Env::prefixed("GATEWAY_").split("__")` (`src/config.rs:245-247`).

**Boot.** With `backend = "redis"` and a non-empty config (D11), `main.rs` calls
`paigasus_redis::connect`, which is eager: a Redis that is down at boot fails the boot
(`redis_conn.rs:117-122`). So a wrong URL or a wrong password is found at deploy time, not as a
silent fail-open at run time. A Redis outage after boot is fail-open (D10). **Order:**
`prime_metrics` and `RedisLimitStore::connect` run after `paigasus_observability::init`
(`src/main.rs:68`). The breaker sets its gauge in its constructor (`redis_conn.rs:336-340`), so a
connect before the recorder exists loses that sample. Rejected: a lazy connect at boot. It would
turn a typo in the URL into a gateway that runs with no limits and only a metric to show it. The
cost: during a Redis outage, a new replica cannot start, so a scale-up or a rollout stalls. IAM
accepted the same trade (SMA-473 D10, `redis_conn.rs:119-122`). Q14 asks Sven to confirm.

D18. **The Redis keys and the Lua script.**

**Keys.** All keys start with `paigasus:gateway:limits:v1:`. The `v1` lets a later format change
use new keys with no migration.

| Key | Value | TTL |
|---|---|---|
| `…:rate:p:<principal PRN>:<w>` | principal request count in window `w` | `EXPIRE 180` on every increment |
| `…:rate:o:<org UUID>:<w>` | org request count in window `w` | `EXPIRE 180` on every increment |
| `…:budget:<org UUID>:<period key>` | org tokens used in the period (D6 key) | `EXPIRE <ttl_secs>` on every charge |

`w` is the window index `floor(unix_ms / 60000)`. A rate key is read as "current" in window `w`
and as "previous" in window `w + 1`, so it must live until the end of `w + 1`. A TTL of 180 s from
the last increment covers that (two windows) plus one window of margin for clock skew between
replicas.

**The budget TTL is relative.** `ttl_secs = period_end + 86400 − now`, in whole seconds. The domain
computes it (`BudgetPeriod::charge_ttl(ticket_period, now) -> Option<u32>`) from the injected clock
(§ 4.3a), the same clock that chose the period key. The charge sends `EXPIRE <key> <ttl_secs>`, not
`EXPIREAT`. Reason: `EXPIREAT` is an absolute instant on the Redis server's clock, so the gateway's
clock and the Redis clock would both decide one key's life. A relative TTL depends on the gateway's
clock only (D18 "Clock"). When `ttl_secs < 1`, the charge arrives more than one day after its
period ended. The domain returns `None`, the adapter sends nothing, and it increments
`gateway_limit_charges_dropped_total{reason="period_expired"}`. Rejected: a clamp to 1 s. It writes
a key that Redis deletes at once, which hides the event and costs a round trip. A ticket older than
a day cannot come from a normal request, so the drop path is for a defect or a stopped clock.

The budget key lives one day past its period end, so a late charge on a rolled-over ticket (D7)
still lands, and then Redis evicts it. No key needs a reset job (D6). The script uses no command
newer than Redis 5.0 (no `EXPIRE NX`, which is Redis 7.0). The stated minimum Redis version is 6.2
(D22); the script does not rely on that, so it also runs on the older default image of IAM's test
helper (`redis:5.0`, `testcontainers-modules` 0.15, `src/redis/standalone.rs:4`).

**Redis topology.** The gateway needs a URL that always reaches the current primary: a single
node, or a primary behind a service address that moves on failover. A replica answers a write with
`READONLY`, so a URL that can reach a replica turns every check into a `Server` failure and
fail-open. Redis Cluster is not supported. The workspace `redis` dependency has no `cluster`
feature (`rs/Cargo.toml:210-212`), and IAM has the same limit today. Rejected: a `{<org>}` hash tag
on every key, with the principal keys under the org tag. In a cluster that puts one script's keys
in one slot. But it changes the principal limit from "per principal" to "per principal and org",
and it buys nothing without a cluster client. Q16 asks Sven to confirm.

**The admission script.** One script per request, loaded with `redis::Script` (D19).

- `KEYS` = principal current, principal previous, org current, org previous, budget. A dimension
  with no limit passes an empty limit, and the script skips it.
- `ARGV` = `elapsed_ms` in the current window, the principal limit, the org limit, the budget
  limit, and the rate TTL (180). The budget key is only read, so the script needs no budget TTL.
- The script reads every count with `GET` (a missing key is 0). It evaluates the D3 integer form
  for each rate and `used < budget` for the budget. If every check passes, it runs `INCR` and
  `EXPIRE` on the two current rate keys. If any check fails, it writes nothing (D4).
- It returns integers only (a Lua number is truncated to an integer reply): an admit flag, and for
  each dimension a pass flag plus its counts. The application service computes `Retry-After` and
  the precedence (D8) in Rust, from the same domain functions that the memory adapter uses.
- The script does not write the budget key on admission. A request adds tokens only when it is
  charged.

**The charge.** `MULTI`, `INCRBY <budget key> <tokens>`, `EXPIRE <budget key> <ttl_secs>`, `EXEC`,
through `redis::pipe().atomic()`. `RedisHandle` implements `req_packed_commands`
(`redis_conn.rs:95-107`), so a pipeline passes through the breaker. A charge of zero tokens sends
nothing. `tokens` is clamped per D23.

**Clock.** The gateway's clock is the source, passed in through `now` and the injected `Clock`
(§ 4.3a). The script does not call Redis `TIME`, and no command takes an absolute instant. Reasons:

- The domain computes the window index, `elapsed_ms`, the period key, the period end and the budget
  TTL once, for both adapters. One set of pure functions is tested with fixed instants (§ 5.1).
- The contract suite (§ 5.2) runs on both adapters with a fixed clock. With `TIME`, the Redis run
  could not test a window edge or a period edge.
- Skew between replicas is bounded by NTP, normally milliseconds. Near a window edge, a skewed
  replica can write to the next window's key early or the previous one late. The estimate then
  moves by about `skew / 60 s` of one window's count. Near a period edge, a request can count in
  the neighbouring period. Both errors are bounded by the skew and are stated in § 6.

Rejected: Redis `TIME` in the script. It removes the skew, but it splits the window arithmetic
between Rust and Lua, and the period key (an ISO week, a month) would need calendar code in Lua.

D19. **Script loading.** The adapter builds one `redis::Script` at construction.
`Script::invoke_async` sends `EVALSHA`. On a `NOSCRIPT` answer it sends `SCRIPT LOAD` and then
`EVALSHA` again (`redis-1.7.1/src/script.rs:195-215`). This covers a Redis restart, a failover to a
replica that never saw the script, and `SCRIPT FLUSH`. `redis::Script` needs the `redis` crate's
`script` feature, which the workspace turned off on purpose (`rs/Cargo.toml:206-209`: "Default
features (… script …) are all unneeded for a plain SET/GET cache"). This issue adds `script` in
the `paigasus-redis` crate's own dependency line, not in the workspace table, so the comment stays
true for IAM. Cargo unions features, so IAM's build gets the feature too; that changes no IAM
behavior. The feature pulls in one new crate, `sha1_smol` (`redis-1.7.1/Cargo.toml:101`; it is not
in `rs/Cargo.lock` today). `repo:deny` checks its licence against `rs/deny.toml:23-28`, and
`repo:osv` checks its advisories. The plan records both results.

Rejected: a hand-written `EVALSHA` with `SCRIPT LOAD`. `redis::Script` already handles `NOSCRIPT`.

D20. **Hot-path latency when Redis is down.** The gateway reuses the lib's tuned config and
breaker, so these bounds come from the moved code, not from new code:

- **Redis stopped** (connection refused): a command fails in about 100-200 ms
  (`docs/ops/RUNBOOK-observability.md:1785-1786`).
- **Redis blackholed**, breaker closed: a command waits for `connection_timeout` (1 s) twice, about
  2.1 s (`redis_conn.rs:22-23`, `:1129-1137`). The first three failures open the breaker
  (`FAILURE_THRESHOLD`, `:167`). Every request that is in flight before the breaker opens pays this.
- **Breaker open**: `admit` returns `ShortCircuit` with no I/O (`:370-373`), for 2 s
  (`OPEN_DURATION`, `:193`). The request is admitted at once under D10.
- **Half-open**: one probe request pays the dial again (up to about 2.1 s, or less if it joins a
  dial that is already in flight, `:169-186`). Every other request short-circuits (`:377-380`).
- **Connected, but Redis stops answering**: `response_timeout` is 500 ms (`:19-20`).

So under a blackhole, a small number of requests per 2 s window pay about 2.1 s, and the rest pay
nothing. Rejected: a gateway-side `tokio::time::timeout` (for example 250 ms) around
`check_and_admit`. A timeout drops the command future, and a dropped `ProbePermit` in the closed
state records nothing (`:323-329`, `:441-453`). So the breaker would never open, and every request
would pay the full timeout for the whole outage. The plan must not add one without a breaker
change.

D21. **Redis durability and eviction.** Redis can lose or evict a key with no error to the
gateway. A lost budget key reads as zero, so a lost key silently resets an org's budget. The
requirements:

- **A dedicated instance or logical database for limits is recommended.** IAM's caches set TTLs
  and are safe to evict; the limit keys are not caches. Sharing one `maxmemory` pool lets cache
  pressure evict a budget. A logical database (`redis://host:6379/2`) separates the keys but not
  the memory pool, so a dedicated instance is the stronger choice. Q17 asks Sven.
- **`maxmemory-policy noeviction`**, or enough headroom that eviction never runs. Under
  `noeviction`, a full instance refuses the write with an `OOM` error. That error is
  `kind = Server` (D10): the request fails open, the metric moves, and the log line is at `error`.
  That is visible. Under an `allkeys-*` or `volatile-*` policy, Redis evicts a budget key silently
  instead (every limit key has a TTL, so `volatile-*` policies also select them).
- **AOF persistence** (`appendonly yes`) if a budget must survive a Redis restart. With no
  persistence, a Redis restart resets every budget and every rate count. With RDB snapshots only,
  a restart loses the charges since the last snapshot.
- **Replication is asynchronous.** A failover to a replica can lose the charges of the last
  moments before it. This under-counts.

The gateway cannot check these settings at run time without `CONFIG GET`, which a managed Redis
often refuses. So they are documented requirements: in § 6, in the `[limits]` comments of
`gateway.toml.example`, and in a new RUNBOOK section (§ 7).

D22. **A new dev-only lib crate `paigasus-test-docker` owns the Docker-skip policy.** IAM's
`tests/support/docker.rs` (290 lines) holds the SMA-538 policy: `skip_docker` (`:98-100`),
`require_docker` (`:104-106`), `is_daemon_unreachable` (`:156`), `start_or_skip` (`:257`) and
`start_redis_or_skip` (`:286-290`). The gateway's Redis tests need the same policy. This issue moves
the policy into `rs/crates/libs/paigasus-test-docker`, and IAM and the gateway both use it as a
dev-dependency.

- The crate is `publish = false`, version `0.0.0`, with `testcontainers = "0.27"` and
  `testcontainers-modules = { version = "0.15", features = ["redis"] }` as normal dependencies.
  Both are already in `rs/Cargo.lock` through IAM (`paigasus-iam/Cargo.toml:139`, `:144`). No
  crate lists it under `[dependencies]`; only `[dev-dependencies]`.
- It moves the policy code and its two Docker-free test binaries,
  `paigasus-iam/tests/support_docker_policy.rs` and `tests/support_docker_retry.rs`, into its own
  `tests/`. IAM's `tests/support/docker.rs` keeps only IAM-specific helpers, or becomes a
  re-export. IAM's Postgres helpers (`start_migrated_postgres`, `start_raw_postgres`,
  `tests/support/mod.rs:6-9`) stay in IAM and call the crate's `start_or_skip`.
- `start_redis_or_skip(what)` keeps `Redis::default()` (`docker.rs:287`), so IAM's tests keep the
  image they use today (A13). A new `start_redis_image_or_skip(tag, what)` takes an explicit tag.
  The gateway pins its tags with it (below).
- IAM's binary counts change: two test binaries leave IAM. The canary doc
  (`paigasus-iam/tests/docker_preflight.rs:14-25`), `rs/CLAUDE.md:26-30` and
  `docs/dev-setup.md:67` restate those counts, and the plan re-derives them with the commands in
  `docker_preflight.rs:20-25`.

**`repo:iam-docker-policy-single-site`** keeps its id, although its name says "iam". `ci.yml`'s
`T=(…)` array and the root `CLAUDE.md` `ci-targets` block are byte-gated against each other, so a
rename touches both and the root memory file for no behavior gain. The description says that the
gate now covers every crate. The single site becomes `rs/crates/libs/paigasus-test-docker/src/`.
The gate scans that directory, IAM `tests/` and gateway `tests/`, with the same terms
(`moon.yml:543`'s `var_os("CI")`, `var("CI")`, `option_env!("CI")` and the `AsyncRunner`
import). Its control requires at least one hit in the crate's `src/`; every other hit is an
offender. Its `inputs` gain the crate's `src/**/*` and `paigasus-gateway/tests/**/*`.

**The gateway's Docker tests.**

- A canary `paigasus-gateway/tests/docker_preflight.rs`, in the form of IAM's, so a Docker-less
  run of the gateway reds once instead of passing every Redis test silently.
- `rs/.config/nextest.toml` gains two overrides in the form of IAM's (`:79-82` and `:99-107`): one
  for `package(=paigasus-gateway) and binary(docker_preflight)` with `retries = 1`, and one that
  names each gateway Redis test binary (`limits_store_redis`, `limits_redis_e2e`) with the same
  exponential retries and `test-group = 'docker-containers'` (`:52-53`, 8 threads). The gateway's
  pure unit tests keep `retries = 0`.
- `paigasus-gateway-rs:test` gets `options.mutex: 'heavy-integration'`, as
  `paigasus-iam-rs:test` has (`paigasus-iam/moon.yml:52-57`). Reason: its Redis containers run on
  the same CI runner as the console e2e tasks, and SMA-718 showed that overlap starves the browser.
  The cost is that the gateway's test task waits for IAM's; it is short, so the wall time grows a
  little.

**Redis versions.** The stated minimum Redis version is 6.2. The contract suite runs on two pinned
images: `redis:6.2-alpine` (the minimum) and `redis:7.4-alpine` (the version kind runs,
`ci/kind/manifests/redis.yaml:25`). Each image is one container per test binary, and the suite
cases share it with separate keys (§ 5.2), so the second image costs one container start, a few
seconds. Reasons for 6.2 and not 5.0: Redis 5.0 and 6.0 are past their upstream support, and a
stated minimum that is tested is worth more than an untested claim of 5.0 support. The script still
avoids 7.0-only commands (D18), so 6.2 is a tested floor, not a hard need.

Rejected: the policy in `paigasus-redis`'s `test-support` feature. The policy also serves IAM's
Postgres and NATS suites, so it does not belong in a Redis crate. Rejected: a gateway copy of the
policy with a widened gate (revision 4's choice). It leaves two copies of a policy whose whole
point is one copy (SMA-538).

D23. **Numeric bounds.** `validate` rejects `principal_requests_per_minute` and
`org_requests_per_minute` above 10^9, and `tokens_per_period` above 10^12. Each charge clamps
`tokens` to at most 10^9 (no real chat completion uses that many; a larger value is a bad usage
record). The arithmetic bound: a rate count only grows on an admission, so a count is at most its
limit, 10^9. The largest D3 product is `10^9 × 60000 = 6 × 10^13`, below 2^53 (about
9 × 10^15), so Lua's doubles and the Rust `u64` arithmetic both hold it exactly. A budget key can
pass its limit by the D7 overshoot, but `10^12 + 10^9 × concurrent requests` stays far below both
2^53 and Redis's signed 64-bit `INCRBY` range.

## 4. Design

### 4.1 Config (`src/config.rs`)

```toml
[limits]
backend                       = "redis"   # "memory" | "redis"; default "memory"
# redis_url: set GATEWAY_LIMITS__REDIS_URL, not this file (it can carry a password).
# Required when backend = "redis". Use rediss:// outside a private network.
# Redis 6.2 or later, a URL that always reaches the primary, no Redis Cluster.
# Use a dedicated instance (or at least a logical database) for limits, set
# maxmemory-policy noeviction, and turn on AOF if budgets must survive a Redis restart.
principal_requests_per_minute = 60        # unset: no principal rate limit; max 1000000000
org_requests_per_minute       = 600       # unset: no org rate limit; max 1000000000
tokens_per_period             = 5000000   # unset: no token budget; max 1000000000000
budget_period                 = "monthly" # "daily" | "weekly" | "monthly"; default "monthly"

# TOML only: environment variables cannot set an array of tables.
# With backend = "redis", give every replica the same list (D12).
[[limits.org]]
id                      = "0190a100-0000-7000-8000-0000000000a1"
org_requests_per_minute = 1200
tokens_per_period       = 20000000

[[limits.org]]
id     = "0190a100-0000-7000-8000-0000000000b2"
exempt = true
```

`GatewayConfig` gains `limits: Option<LimitsConfig>`. `LimitsConfig` gains
`backend: LimitsBackend` (`#[serde(rename_all = "lowercase")]`, default `Memory`) and
`redis_url: Option<SecretString>` with `#[serde(skip_serializing)]` (D17). `validate` rejects: a `0`
value (D11); a value above its D23 cap; an `org.id` that is not a 36-character UUID (the same rule
as `domain::parse_org_uuid`); a duplicate `org.id`; `exempt = true` with a limit value (D11);
`backend = "redis"` with no or an empty `redis_url`; `redis_url` with `backend = "memory"` (D17).
Each rejection names the key, in the style of the existing messages. `budget_period` is an enum,
so an unknown value fails deserialization. The `Defaults` struct (`src/config.rs:148-160`) gains no
`limits` field, because the table is absent by default (A6). Environment variables:
`GATEWAY_LIMITS__BACKEND`, `GATEWAY_LIMITS__REDIS_URL`,
`GATEWAY_LIMITS__PRINCIPAL_REQUESTS_PER_MINUTE`, `GATEWAY_LIMITS__ORG_REQUESTS_PER_MINUTE`,
`GATEWAY_LIMITS__TOKENS_PER_PERIOD`, `GATEWAY_LIMITS__BUDGET_PERIOD`.

### 4.2 Domain (`src/domain/limits.rs`, new; `src/domain.rs` becomes `src/domain/mod.rs`)

The domain has no dependency on `redis`, `tokio`, `metrics` or the config types.

- `LimitPolicy` — the resolved numbers for one request (principal rate, org rate, org budget, each
  `Option<NonZeroU64>`), and `is_empty()` (D11). Pure. It is built from a domain-owned
  `LimitRules` value (the defaults plus the override map). `LimitsConfig` converts into
  `LimitRules` at boot.
- `SlidingWindow` — the D3 arithmetic with the D4 split: `check(now, limit)` (pure) and
  `commit(now)`. Clock passed in. Saturating arithmetic (D10). A pure function
  `admits(previous, current, elapsed_ms, limit) -> bool` holds the D3 integer form, and
  `retry_after(previous, current, elapsed_ms, limit) -> u32` computes the wait. The Redis adapter
  calls `retry_after` on the counts that the script returns.
- `WindowIndex` — `floor(unix_ms / 60000)` and `elapsed_ms`, from `now`. Pure.
- `BudgetPeriod` — `Daily | Weekly | Monthly` (`Copy + Default`, default `Monthly`). For `now` it
  returns the period key (D6), the reset instant, and `charge_ttl(key, now) -> Option<u32>` (D18).
  Pure. The key is carried as `Copy` values (a day number, an ISO `(year, week)`, or a
  `(year, month)`), and formatted to a string only by the Redis adapter and the refusal message.
- `LimitDecision` — `Admit(Option<LimitTicket>)` | `Refused(Vec<FailedCheck>)`, where a
  `FailedCheck` is a principal rate, an org rate (each with its counts and `elapsed_ms`) or the
  budget (with its period and reset instant).
- `LimitTicket` — carries the org UUID and the budget period key. It exists only when a budget
  applies, and it is given back to `charge` exactly once, by the charge guard.
- `trait LimitStore` — async, as in D16.
- `LimitStoreError` and `UnavailableKind` — as in D10.
- `trait Clock` — one method, `now() -> SystemTime`.

### 4.3 Adapters (`src/adapters/limits/{mod,memory,redis}.rs`, new)

`src/adapters/limits/` holds the two store adapters only.

**`MemoryLimitStore`** holds one `std::sync::Mutex<State>` with two `HashMap`s (principal PRN →
window counts; org UUID → window counts plus budget use per period key). The lock is held for the
arithmetic only, never across an `.await`, and `check_and_admit` has no `.await` inside it. D4
holds because every key is checked and then committed under one lock. D13 eviction runs inside
`check_and_admit`. The lock is recovered on poison (D10).

**`RedisLimitStore`** holds a `paigasus_redis::RedisHandle`, the `redis::Script` (D19) and a
`TaskTracker` (D16). It builds the D18 keys, runs the script, maps a `RedisError` to
`LimitStoreError::Unavailable { kind }` (D10), and records the store-unavailable metric for a failed
charge. It opens its connection with
`BreakerMetrics { state: names::GATEWAY_REDIS_BREAKER_STATE, transitions:
names::GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL, role: "limits" }`. It exposes `apply_charge` (D16)
and `tracker()` for the shutdown drain.

Neither adapter has a test-only read accessor. The contract suite (§ 5.2) observes the store
through the port and `apply_charge` only. A test that needs a raw count reads Redis through
`redis-cli` in the container (§ 5.3), as IAM's tests do
(`paigasus-iam/tests/authz_generations_redis.rs:62-82`).

### 4.3a Application service (`src/application/{mod,limits,charge_guard}.rs`, new)

The gateway has no `application` module today (`src/lib.rs:10-14`). It gains one, laid out like
IAM's: flat files under `src/application/` with a `mod.rs` (`paigasus-iam/src/application/`
holds files such as `authorize.rs` and `log_rate_limit.rs`, and no subdirectories).

- `limits.rs`: `Limits` holds the `LimitRules`, an `Arc<dyn LimitStore>` and an `Arc<dyn Clock>`
  (`SystemClock` in production, a fixed clock in tests). It exposes
  `async fn admit(&self, caller: &CallerContext) -> Result<ChargeGuard, LimitRefusal>`. It
  resolves the org (D2), builds the policy, skips the store for an empty policy (D11), calls the
  store, applies the D8 precedence, records the refusal metric, and returns a guard. On
  `Err(LimitStoreError::Unavailable { kind })` it records the D10 metric and log, and returns a
  guard with no ticket. The file also holds `prime_metrics` and the D10 log limiter.
- `charge_guard.rs`: `ChargeGuard` and the A4 charge rules (§ 4.6).

The handler maps `LimitRefusal` to a `GatewayError`. `CallerContext` is a domain type
(`src/domain.rs:20`), so `admit` can take it and the application layer does not depend on the HTTP
adapter.

### 4.4 Wiring (`src/adapters/http/mod.rs`, `src/main.rs`, `src/lib.rs`, `src/runtime.rs`)

`src/lib.rs` gains `pub mod application;`. `AppState` (`src/adapters/http/mod.rs:43`) gains
`limits: Option<Arc<Limits>>`. `None` when `[limits]` is absent. Every place that builds
`AppState` by struct literal gains `limits: None` (`src/adapters/http/mod.rs:245-252`,
`tests/metrics.rs`, `tests/chat_proxy.rs`, and the others the compiler lists).

`main.rs`, after `paigasus_observability::init` (`src/main.rs:68`, D17 "Order"): it calls
`paigasus_gateway::application::limits::prime_metrics(backend)`, then builds the store from
`backend` (`MemoryLimitStore::new()`, or `RedisLimitStore::connect(url).await?`, eager), unless D11
says every policy is empty. `prime_metrics` registers and primes the § 4.8 series at zero, and the
two breaker series only for `backend = "redis"`. The metric tests call the same function.
`describe_gateway_metrics` (`main.rs:174-202`) gains the seven new families, and its doc comment
changes from "7 metric families" (`main.rs:174`) to "14". `main.rs` passes the Redis store's
`TaskTracker` to `runtime::supervise` (D16).

### 4.5 Handler (`src/adapters/http/chat.rs`)

After the streaming check (`chat.rs:109-111`) and before `state.openai.chat_completion`
(`chat.rs:115`):

1. If `state.limits` is `None`, skip to egress (A6).
2. Call `limits.admit(&caller).await`.
3. `Err(refusal)` → return `GatewayError::RateLimited { retry_after_secs }` or
   `GatewayError::BudgetExhausted { resets_at_unix, period }`. Log one `debug` line with principal,
   scope and the reason (never the prompt). The refusal metric is the operator's signal. A refused
   request still costs the 2-3 IAM RPCs of authentication (§ 6).
4. `Ok(guard)` → give the guard the request estimate (D14) and the request and correlation ids,
   which the handler reads from `current_ids()` here, where they exist. Mark the guard `sent`
   just before the egress call.
   - On `Err(OpenAiError::Connect)`: mark the guard `no_charge`.
   - On any other `Err` (timeout, transport): leave it; its drop charges the request estimate.
   - On a non-`2xx` answer: mark it `no_charge`.
   - On a `2xx` `ChatResponse::Full`: parse a copy of the body for `usage.total_tokens`. If
     found, set `reported`. If not, set the D14 completion estimate. The guard drops at the end of
     the handler.
   - On a `2xx` stream: move the guard into the stream adapter (§ 4.6).

If the client disconnects before the upstream answers, axum drops the handler future. The guard is
a local of that future, so its drop charges the request estimate (A4). If the client disconnects
while `admit` awaits Redis, the future drops before a guard exists. The script may then have
counted the request against the rate keys with no upstream call. That is one rate slot, not a
token charge, and it is accepted.

`GatewayError` is `Copy` and derives `strum::EnumIter` in tests (`error.rs:51-52`). So the two new
variants carry `Copy + Default` fields: `retry_after_secs: u32`, `resets_at_unix: i64` and
`period: BudgetPeriod`. The message formats the reset instant as RFC 3339 with `chrono`
(`chrono = { workspace = true }` joins the gateway crate's dependencies; it is already a workspace
dependency, `rs/Cargo.toml:66`). `parts()` today returns only `&'static str`, so `into_response`
adds `Retry-After` and `x-should-retry` for these two variants, and `BudgetExhausted` builds its
message with `format!`. The exhaustive `strum` registry test (SMA-504 AC 6) covers both variants.
The retryability table test (`error.rs:354-366`) gains `GatewayError::RateLimited { .. } =>
Retryable::Yes`; its `_` arm already gives `BudgetExhausted` `No`, which is correct, but the plan
lists it explicitly.

### 4.6 The charge guard and stream usage (`src/application/charge_guard.rs`, `chat.rs`)

`ChargeGuard` owns `Arc<Limits>`, an `Option<LimitTicket>`, the request estimate, a state
(`admitted | sent | no_charge | reported(u64) | estimated(u64)`), the org, an `outcome` reason for
a missing ticket (`no_budget` or `store_unavailable`), and the request and correlation ids. Its
`Drop` computes the tokens per A4. When it has a ticket and the tokens are above zero, it calls
`store.charge(ticket, tokens, clock.now())` once (D16) and increments
`gateway_tokens_charged_total{source}`. `Drop` never panics (D10).

It emits one line `chat completion metered` with `org`, `tokens`, `source`, `outcome`,
`request_id` and `correlation_id`. `outcome` is one of `charged`, `store_unavailable` (fail-open,
no ticket), `no_budget` (no budget applies to this org) and `zero` (no tokens: a connect failure or
a non-`2xx` answer). The level is `info` when `tokens > 0` and `debug` when `tokens = 0`. Reasons:
the line is the per-org spend record for both paths, so every request that used tokens must be in
it, also the `store_unavailable` ones, which are the only record of fail-open spend. A zero-token
line records no spend, so it goes to `debug`. Revision 4's name `chat completion charged` was wrong
for a line that also records requests with no charge. The ids are explicit fields, so the line works
inside the stream, where `current_ids()` is `None` (SMA-504 § 4.3 situation 3, `chat.rs:135-141`).

`gateway_tokens_charged_total` counts the tokens the guard sent to the store. A Redis charge
that fails after that is counted in `gateway_limit_store_unavailable_total{op="charge"}` (D10).
So the difference between the two is visible, but it is a count of failed charges, not of tokens.

The stream adapter does not put `Drop` on `StreamState`: the unfold closure destructures the state
by move (`chat.rs:226`), and a type with `Drop` cannot be destructured (E0509). Instead, the
unfold state becomes a struct `{ phase: StreamState, scan: UsageScanner, guard: Option<ChargeGuard> }`
(or the guard is a separate field that moves through each step). When the stream ends, fails, or
is dropped, the guard drops once. The adapter is still returned directly, never spawned
(`chat.rs:142`), so reqwest's cancel-on-drop stays. `UsageScanner` parses SSE, so it lives in the
HTTP adapter (`src/adapters/http/usage.rs`, new).

`UsageScanner`:

- It keeps the tail of the current partial SSE record, bounded to 64 KiB. A record larger than
  that is skipped, not buffered, and still counts as one record.
- It splits records on a blank line with any of the three line endings: `\n\n`, `\r\n\r\n`,
  `\r\r` (self-hosted vLLM and LiteLLM upstreams are supported, SMA-558; the console parser does
  the same, `ts/apps/gateway-console/lib/chat-stream.ts:32`).
- On each complete `data:` record that is not `[DONE]`, it looks for the byte string `"usage":{`
  (with optional whitespace after the colon). With `include_usage`, OpenAI sends `"usage":null`
  in every chunk, so a match on `"usage"` alone would parse every record. Only a matching record
  is parsed as JSON. The last non-null `usage.total_tokens` seen wins, and the guard state
  becomes `reported`.
- Every other complete `data:` record adds one to the completion estimate (D14).

OpenAI puts `usage` into a stream only when the client sends
`stream_options: {"include_usage": true}`. The gateway must not change the request (A5), so it
cannot turn this on itself. The console playground turns it on (A10, § 4.9).

### 4.7 Errors (`contracts/proto/paigasus/common/v1/error.proto`, `src/adapters/http/error.rs`)

- Add `ERROR_REASON_RATE_LIMITED = 311` ("rate-limited") and `ERROR_REASON_BUDGET_EXHAUSTED =
  312` ("budget-exhausted") with comments in the style of 308-310. Run `buf format -w` and
  `contracts:generate` (the codegen drift gate checks the Rust, Python and TS bindings).
- Add the two variants to `GatewayError` with the D8 rows. `retryable()`: `RateLimited` →
  `Yes`, `BudgetExhausted` → `No`.
- `ci/error-registry/check.py`: the gateway's `error.rs` is already an `emits` site
  (`check.py:90-91`), guarded by `every_gateway_code_is_declared_in_the_canonical_registry`, so it
  needs no new row. The gate scans every `src/**/*.rs` under `rs/crates` (`check.py:77-78`). Every
  other source file that spells `"rate-limited"` or `"budget-exhausted"` (for example a unit test
  in `chat.rs` or in `src/application/limits.rs`) needs an `asserts` row, in the form of
  `check.py:110-111`. The plan lists the files after the code exists.
- `paigasus-proto/src/error.rs`: add both to the gateway list, and change the count assertion
  (`rs/crates/libs/paigasus-proto/src/error.rs:239`) from 65 to 67.
- `ts/packages/paigasus-proto/src/error.test.ts:60`: `toHaveLength(65)` becomes
  `toHaveLength(67)`.

### 4.8 Metrics and alerts

**Metrics** (`rs/crates/libs/paigasus-observability/src/names.rs`):

| Name | Type | Labels | Meaning |
|---|---|---|---|
| `gateway_limit_refusals_total` | counter | `reason` = `principal_rate` \| `org_rate` \| `org_budget` | Requests refused before egress, one per request (D8 precedence). |
| `gateway_tokens_charged_total` | counter | `source` = `reported` \| `estimated` | Tokens the guard sent to the store (§ 4.6). |
| `gateway_limit_unscoped_requests_total` | counter | none | Requests whose scope names no org (D2). |
| `gateway_limit_store_unavailable_total` | counter | `op` = `check` \| `charge`; `kind` = `io` \| `server` \| `decode` | Store operations that failed or met an open breaker (D10). A `check` here is a request admitted with no limit. |
| `gateway_limit_charges_dropped_total` | counter | `reason` = `no_runtime` \| `shutdown` \| `period_expired` | Charges that were not sent (D16, D18). |
| `gateway_redis_breaker_state` | gauge | `role` = `limits` | 0 closed, 1 half-open, 2 open. Emitted by `paigasus-redis` with the gateway's names (D15). |
| `gateway_redis_breaker_transitions_total` | counter | `role` = `limits`, `to` = `closed` \| `half_open` \| `open` | One per breaker transition. Emitted by `paigasus-redis`. |

No label carries a principal or an org id: that would be an unbounded label set. Per-org use is
in the `chat completion metered` log line (§ 4.6). Every series and every label value is primed at
zero by `prime_metrics()` when `[limits]` is present (memory
`prometheus-new-counter-first-sample-trap`). The two breaker series are primed only for
`backend = "redis"`; the gauge is also set to 0 by the breaker's constructor, as IAM's is
(`redis_conn.rs:336-340`). The names join `ALL` (`names.rs:245-294`), so the drift test covers any
dashboard or rule that uses them. The breaker names copy the IAM aggregation rule:
`max by (job, role)`, never `sum` (`names.rs:87`). `docs/ops/RUNBOOK-observability.md` § 2.3 gains
the seven families.

**Alerts** (`ops/observability/prometheus/rules/gateway.rules.yml`, which holds three gateway
alerts today, `:5`, `:10`, `:15`; tests in `rules/tests/gateway.test.yml`). Two rules, in the form
of the IAM breaker rules (`iam.rules.yml:267-289`), in PR 2:

- `GatewayLimitsRedisBreakerOpen`: `max by (job, role) (gateway_redis_breaker_state) != 0`,
  `for: 2m`, severity `warning`. The description says that limits are not applied (fail-open) and
  names the RUNBOOK section (§ 7).
- `GatewayLimitStoreUnavailable`:
  `sum by (job, op, kind) (increase(gateway_limit_store_unavailable_total[10m])) > 0`, `for: 0m`,
  severity `warning`. It also fires on a short outage that the breaker gauge misses between
  scrapes, and on `server` and `decode` errors, which never open the breaker
  (`counts_as_failure`, `redis_conn.rs:487-489`). The prime at zero makes `increase()` see the first
  event.

Each rule gets a `promtool` test with a firing case and a quiet case in `gateway.test.yml`.
`repo:promtool` runs them (`moon.yml` task `promtool`).

### 4.9 The TS SDK and the consoles

Sven decided on 2026-10-02 that `budget-exhausted` gets a new presentation `quota-exhausted`
(answered Q13). Adding a union member touches these places:

- `ts/packages/paigasus-sdk/src/errors/types.ts:22`: the `Presentation` union gains
  `'quota-exhausted'`. It has nine members today, and `'rate-limited'` is one of them.
- `ts/packages/paigasus-sdk/src/errors/presentation.ts:20`: `PRESENTATION` is a total
  `Record<Exclude<ErrorReason, UNSPECIFIED>, …>`, so the regenerated enum is a TS2741 compile
  error until both entries exist. `RATE_LIMITED` → `'from-transport'` (the transport table maps
  429 to the `rate-limited` presentation, `transport-status.ts:60`, which fits "try again soon").
  `BUDGET_EXHAUSTED` → `'quota-exhausted'`, not retryable. The header comment's "66 keys rather
  than 65" (`presentation.ts:12`) becomes "68 keys rather than 67".
- `ts/apps/gateway-console/app/_components/error-copy.ts:19-28` and
  `ts/apps/iam-console/app/_components/error-copy.ts:15`: each `PRESENTATION_COPY` is a total
  `Record<Presentation, …>`, so each fails the type check until it has a `quota-exhausted` row. The
  copy: title "Usage limit reached", body "Your organization used its token budget for this
  period. Ask an administrator, or wait until the budget resets." The body does not say "try
  again". The comment "a tenth presentation fails the type-check"
  (`gateway-console/.../error-copy.ts:12`, `iam-console/.../error-copy.ts:7`) becomes "an
  eleventh" in both files.
- `ts/packages/paigasus-sdk/src/errors/transport-status.ts:14-19`: the comment says nothing in
  this repository emits 429, and that a future quota needs no tenth presentation value. Both
  statements become false. The comment changes: the gateway now emits 429 with a registry reason,
  `budget-exhausted` has its own presentation, and an upstream 429 without a reason still takes
  the transport table's answer.
- `ts/packages/paigasus-sdk/tests/transport-status.test.ts:8`: the hard-coded `PRESENTATIONS`
  list gains `'quota-exhausted'`.
- `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`: `toHaveLength(65)` becomes
  `toHaveLength(67)`.
- `ts/packages/paigasus-sdk/tests/map-error.test.ts:152-156`: the comment "Nothing in this
  repository emits 429 itself" changes. The assertion stays: that fixture is an upstream 429 with
  no reason.
- `ts/apps/gateway-console/tests/unit/error-views.test.tsx:61` and
  `ts/apps/iam-console/tests/unit/error-views.test.tsx:63`: each `it.each` list gains
  `'quota-exhausted'`, so the new copy row renders with the correlation id.
- The gateway console's other `Presentation` readers (`page-error.tsx:22-33`,
  `section-error.tsx:14-23`, `form-error.tsx:23-26`) branch only on `forbidden`, `not-found`,
  `disabled` and `relogin`, and render every other value through `PRESENTATION_COPY`. So they need
  no change. The plan checks both consoles again with the compiler.
- `ts/apps/gateway-console/lib/chat-route.ts:206`: the request becomes
  `{ model, messages, stream: true, stream_options: { include_usage: true } }`. The console stream
  parser already ignores a record whose `choices` array is empty (`chat-stream.ts:55-64`), so the
  final usage record passes through with no output. A test asserts this.

**Rollout order.** A console with an older SDK sees an unknown reason, and `scrub()` then rewrites
the message to "The model provider rejected the request." (`chat-route.ts:110-112`). So the
console and the SDK ship in the same release as, or before, a gateway that has `[limits]` set. In
one monorepo PR this holds by construction; the operator note in § 9 says it.

### 4.10 The `paigasus-redis` and `paigasus-test-docker` crates

- Paths `rs/crates/libs/paigasus-redis` and `rs/crates/libs/paigasus-test-docker`. Each
  `Cargo.toml` uses `edition.workspace`, `rust-version.workspace` (2024 and 1.95,
  `rs/Cargo.toml:37-41`), `license.workspace`, `authors.workspace`, `publish = false` and
  `version = "0.0.0"`, like `paigasus-logging` (`rs/crates/libs/paigasus-logging/Cargo.toml:1-9`).
  `[lints] workspace = true`.
- `paigasus-redis` dependencies: `redis = { workspace = true, features = ["script"] }` (D19),
  `metrics`, `tokio` (for the test-support blackhole and the `#[tokio::test]` tests), and
  `metrics-util` as a dev-dependency for the breaker metric test (IAM has it today,
  `paigasus-iam/Cargo.toml:162`). No in-tree dependency. `[features] test-support = []` (D15).
- `paigasus-test-docker` dependencies: D22.
- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- `rs/Cargo.toml` `members` gains two literal lines, `"crates/libs/paigasus-redis"` and
  `"crates/libs/paigasus-test-docker"` (`rs/Cargo.toml:20-34`). A glob is forbidden:
  `repo:affected-smoke`'s A11 reds on a glob character, and A9 reds on a crate directory that is
  not listed (`rs/Cargo.toml:1-10`). `[workspace.dependencies]` gains a `path` entry with
  `version = "0.0.0"` for each, after the `paigasus-observability` entry (`rs/Cargo.toml:184`). The
  version is required, because `rs/deny.toml` sets `wildcards = "deny"`
  (`paigasus-gateway/Cargo.toml` comment on its `paigasus-logging` dev-dependency).
- `moon.yml` for each: `id: 'paigasus-redis-rs'` or `'paigasus-test-docker-rs'`,
  `layer: 'library'`, `language: 'rust'`, and `fileGroups.upstreams: []`, as `paigasus-logging`'s
  (`rs/crates/libs/paigasus-logging/moon.yml`).
- The IAM `moon.yml` gains both crates under `dependsOn` and their `src/**/*` and `Cargo.toml`
  lines under `fileGroups.upstreams` (`paigasus-iam/moon.yml:7-34`). The gateway `moon.yml` does
  the same (`paigasus-gateway/moon.yml:7-34`). `repo:affected-smoke`'s A6 asserts strict equality
  of that group. The `ts/apps/gateway-console/moon.yml:441-452` input list, which mirrors the
  gateway's upstreams for the playground e2e tier, gains the `paigasus-redis` lines. (The e2e tier
  runs the gateway binary, which does not link `paigasus-test-docker`; the plan confirms whether
  A6 or the e2e harness wants those lines too.)
- `ci/affected-graph/run.sh:391` and `:397` (the `lockfile->all-lint` cases) list every Rust
  crate's `lint` task, and gain `paigasus-redis-rs:lint` and `paigasus-test-docker-rs:lint`. The
  plan adds a `redis->services` case in the form of `service-info->services` (`run.sh:316-317`).
- `rs/release-plz.toml` gains a `[[package]]` entry for each, with `release = false`,
  `publish = false` and `git_release_enable = false`, like `paigasus-logging`
  (`rs/release-plz.toml:223-227`). Neither crate is in `repo:publish-metadata`'s
  `EXPECTED_PUBLISHABLE` (`ci/publish-metadata/run.sh:104`), so that gate needs no change.
  `repo:version-lockstep` writes only the `publish = false` binding crates
  (`ci/version-lockstep/README.md:12-15`), so it needs no change either. The plan runs both gates
  to confirm.
- `.github/CODEOWNERS` is generated by Moon; the plan regenerates it and does not edit it by hand.
- No crate README. The crate doc in `paigasus-redis/src/lib.rs` carries the module doc of
  `redis_conn.rs` (`:3-23`), which explains the budget.

### 4.11 `repo:redis-connect-single-site` (root `moon.yml:420-495`)

The id does not change: `ci.yml`'s `T=(…)` array and the root `CLAUDE.md` `ci-targets` block are
byte-gated against each other. The script changes:

- It runs from `rs/crates` and scans `libs/paigasus-redis/src`, `services/paigasus-iam/src`,
  `services/paigasus-iam/tests`, `services/paigasus-gateway/src` and
  `services/paigasus-gateway/tests`. The terms do not change: `ConnectionManager`,
  `.get_connection_manager`, `.get_multiplexed_async_connection`, and `.get_connection\b` with the
  `/persistence/migration/` exclusion.
- `expected` (the control) must hold at least one hit in `libs/paigasus-redis/src/`. If it is
  empty, the gate exits 2, as today.
- `offenders` is every hit outside `libs/paigasus-redis/src/`. The message names
  `paigasus_redis::connect`.
- `inputs` gain `rs/crates/libs/paigasus-redis/src/**/*`,
  `rs/crates/services/paigasus-gateway/src/**/*` and
  `rs/crates/services/paigasus-gateway/tests/**/*`. Each glob matches tracked files in the same PR,
  so `repo:input-liveness` (`moon.yml:560`) stays green. The IAM inputs stay.
- The description and the comment block name the new site.

`repo:iam-docker-policy-single-site` (`moon.yml:497-558`) changes per D22.

## 5. Tests

### 5.1 Domain unit tests (`src/domain/limits.rs`)

- `SlidingWindow`: a table over (previous count, current count, position in the window, limit) →
  admit or `Retry-After`. Include the window boundary, the first request ever, a limit of 1, a gap
  of more than two windows (the previous count must read as zero), a backward clock step (no
  panic; clamped), and `u64::MAX` counts (no panic; saturating). A check that fails does not
  change the state (D4 split).
- `admits` and `retry_after` (the D3 integer form) agree with `SlidingWindow::check` on every row,
  and at the D23 maximum limit.
- `BudgetPeriod`: daily, weekly and monthly keys and reset instants at 23:59:59.999 and 00:00:00
  UTC, at a month end, at a year end, on 29 February 2028, on a Sunday 23:59:59 and a Monday
  00:00:00 (the ISO week edge), and on 2026-12-31 (ISO week 2026-W53) and 2027-01-01 (still
  2026-W53).
- `charge_ttl`: one second into a period → period length + 86400 − 1; at the period end → 86400;
  86399 s after the period end → 1; 86400 s after → `None`.
- `LimitPolicy`: an override org, an exempt org, an unlisted org, `org = None`, and `is_empty()`.
- D8 precedence: budget and rate both over → `BudgetExhausted`; principal and org rate both over
  → `RateLimited` with the larger `Retry-After`, minimum 1.
- `org_of`: the existing `resolve_org_table` rows still pass.

### 5.2 Store contract suite (`tests/support/limits_contract.rs`)

One suite, generic over a small harness trait with two methods: `store() -> Arc<dyn LimitStore>`
and `async fn charge_now(ticket, tokens, now)`. The memory harness calls `charge`, which is
synchronous. The Redis harness awaits `RedisLimitStore::apply_charge` (D16). So no case polls or
sleeps. The suite uses a fixed clock and the crate's public API only, so
`tests/limits_store_memory.rs` and `tests/limits_store_redis.rs` both run it. The Redis binary
runs the suite once on `redis:6.2-alpine` and once on `redis:7.4-alpine` (D22), with one container
per image, and gives every case its own principal and org ids, so the cases do not share keys.

- D4: the org limit refuses; the principal count did not grow. And the reverse. A budget refusal
  grows neither rate count.
- Budget: use 10 of 10 → refused; use 9 of 10 → admitted, and then a charge of 50 is recorded
  (the D7 overshoot is allowed and visible).
- A new budget period starts at zero, for each of `daily`, `weekly` and `monthly`. A charge whose
  ticket names the previous period goes to that period, not the current one.
- The window edge: requests at 59.999 s and at 60.000 s give the D3 estimate, the same on both
  adapters.
- Concurrency: 64 tasks call `check_and_admit` for one principal with limit 10 at one fixed
  instant; exactly 10 are admitted. On Redis this proves the script's atomicity across
  connections: the 64 tasks use 4 separate `RedisLimitStore` values, which model 4 replicas.

`charge` itself (the spawn) is covered by § 5.8, not by the suite.

### 5.3 Adapter-specific tests

Memory (`src/adapters/limits/memory.rs`, unit tests):

- Eviction (D13): after the sweep, an idle principal's entry is gone and an active one stays.
- Poison: a thread panics while it holds the lock; the next `check_and_admit` succeeds.

Redis (`tests/limits_store_redis.rs`, testcontainers; and `src/adapters/limits/redis.rs` unit
tests for the parts that need no Docker):

- Rate TTL: after one admission, `redis-cli TTL` on both current rate keys reads between 175 and
  180.
- Budget TTL: a charge at a fixed `now` with a known `ttl_secs`; `redis-cli TTL` on the budget key
  reads `ttl_secs − 5 ≤ TTL ≤ ttl_secs`. Both bounds are asserted: the lower bound catches a missing
  `+ 86400`, and the upper bound catches a TTL that does not come from `now`. A charge with no
  `EXPIRE` gives `TTL = -1`, which fails the lower bound.
- Period expired: a charge 86400 s after the ticket's period end writes no key and increments
  `gateway_limit_charges_dropped_total{reason="period_expired"}`.
- Key format: after one admission and one charge, `redis-cli KEYS 'paigasus:gateway:limits:v1:*'`
  lists exactly the expected keys (D18). The raw reads exec `redis-cli` in the container, as IAM
  does (`authz_generations_redis.rs:62-82`).
- `NOSCRIPT` recovery: admit once, run `redis-cli SCRIPT FLUSH`, admit again; the second admission
  succeeds and the store-unavailable metric does not move.
- `OOM` under `noeviction`: a container started with `--maxmemory 1mb --maxmemory-policy
  noeviction`, filled with `redis-cli`; the next check fails open with `kind="server"`.
- Error mapping (unit, no Docker): an `Io` error, the breaker's short-circuit error, a
  `Server` error and an `UnexpectedReturnType` error map to `Io`, `Io`, `Server` and `Decode`.
- Fail-open on an open breaker: a `RedisLimitStore` built on `with_open_breaker_for_tests` and the
  `test_support` blackhole admits every request, issues no ticket, accepts 0 connections
  (`Blackhole::accepted() == 0`, the SMA-702 form), and increments `op="check",kind="io"`. No
  Docker.
- Fail-open on a blackhole with a closed breaker: the first check fails after about 2.1 s and is
  admitted; after three failures the breaker opens and the next check returns at once (it accepts
  no new connection). No Docker.
- A charge against an open breaker increments `op="charge",kind="io"` and does not panic.

### 5.4 HTTP integration (`tests/chat_proxy.rs`, `tests/support/mod.rs`)

The harness already runs the router with a fake IAM and a local fake upstream. Add:

- a `[limits]` builder in the support module, with the memory backend;
- a request counter on `MockOpenAi` (it stores only the last request today,
  `tests/support/mod.rs:64-67`);
- canonical PRN fixtures for org, team and project scopes (the existing
  `CALLER_SCOPE = "prn:paigasus:iam:default:scope/team-a"` does not parse as a tenancy PRN);
- a real-shaped OpenAI chunk fixture: `data: {"id":…,"object":"chat.completion.chunk","created":…,"model":…,"system_fingerprint":…,"choices":[{"index":0,"delta":{"content":" the"},"finish_reason":null}]}`
  with and without a trailing `"usage":null`, and a final usage chunk with empty `choices`;
- a recording `LimitStore` that wraps the memory store and records every `charge` call, and a
  failing `LimitStore` that returns `Unavailable { kind }` for a chosen kind.

Tests:

- A1, A2: `limit + 1` requests; the last is `429 rate-limited`, has `Retry-After ≥ 1`, has
  `paigasus-retryable: true` and `x-should-retry: true`, and the upstream counter reads `limit`.
- A3: a budget of 1 token and an upstream that reports `usage.total_tokens: 5`; the first request
  succeeds, the second is `429 budget-exhausted`, `paigasus-retryable: false`,
  `x-should-retry: false`, the message names the period, and the upstream counter reads 1.
- A4 non-stream with usage, non-stream `2xx` without usage, non-stream timeout (the mock delays
  past `first_byte_timeout_secs`, set to 1 s in the test), stream with a usage record, stream
  without one (N real-shaped chunks, with and without `"usage":null`), CRLF record delimiters,
  client disconnect mid-stream, a mid-stream upstream error (exactly one charge), a connect
  failure and a `429` from the mock upstream (no charge). Each asserts the charged count through
  the recording store.
- A5: the body and the SSE bytes are byte-identical with and without `[limits]`.
- A6: the existing tests run with `limits: None`; no assertion changes.
- A12: with the failing store, a request is admitted, reaches the upstream, causes no `charge`
  call, and logs `outcome = "store_unavailable"`.
- D11: an exempt org with no principal rate makes no store call.
- D9: a `400 invalid-request-body` and a `400 streaming-disabled` do not use quota.
- D2: an API key whose scope is a project PRN is counted against the project's org. An
  unparsable scope with a budget configured is `500 internal` and increments
  `gateway_limit_unscoped_requests_total`.
- A11 end to end (`tests/limits_redis_e2e.rs`, testcontainers): two routers (two `AppState`s, as
  two replicas) each with their own `RedisLimitStore` on one Redis. With an org limit of 3,
  requests alternate between them; the fourth is refused.

### 5.5 Metrics (`tests/limits_metrics.rs`, a new test binary)

The metrics recorder is process-global, so tests in one binary share it
(`tests/metrics.rs:8-12`). The numeric limit tests therefore live in their own binary, and each
test reads a delta (after minus before), not an absolute value, except the one "zero after
`prime_metrics()`" test, which runs in a separate binary. Parse the numeric value of each series
from `/metrics` (memory `prometheus-type-line-vacuous-assertion`):

- zero after `prime_metrics(Redis)`, for every series and label value of § 4.8;
- after `prime_metrics(Memory)`, the two breaker series are absent;
- +1 on `gateway_limit_refusals_total{reason="org_budget"}` after a budget refusal;
- the charged count on `gateway_tokens_charged_total{source="reported"}`;
- +1 on `gateway_limit_store_unavailable_total{op="check",kind="server"}` after a fail-open
  admission with the failing store.

"Absent without `[limits]`" is asserted in a separate binary that never calls `prime_metrics()`.

### 5.6 Config (`src/config.rs` tests, figment `Jail`)

A full table, an empty table, a `0` value, a value above each D23 cap, a bad `org.id`, a duplicate
`org.id`, `exempt` with a limit, an unknown `budget_period`, `weekly`, `backend = "redis"` with no
URL, `redis_url` with `backend = "memory"`, and every `GATEWAY_LIMITS__*` variable of § 4.1 from
the environment. One test serializes a config with a `redis_url` and asserts that the URL is
absent from the output, and that `format!("{cfg:?}")` does not contain it.

### 5.7 `paigasus-redis`, `paigasus-test-docker` and IAM regression

- The lib runs the moved breaker unit tests (`redis_conn.rs:498-1120`) unchanged, except that
  `Breaker::new` takes a `BreakerMetrics`. The generic metric test asserts the caller's names and
  the `role` and `to` labels.
- `paigasus-test-docker` runs the moved `support_docker_policy.rs` and `support_docker_retry.rs`
  unchanged except for their import paths.
- IAM: every existing test passes with no assertion change (A13). IAM gains the name-pin test of
  D15.
- `repo:observability-drift` stays green with no dashboard edit.

### 5.8 The charge from `Drop`

- With a runtime: a guard with a ticket drops inside `#[tokio::test]`; after the `TaskTracker`
  is closed and awaited, the Redis budget key holds the tokens. (No sleep: the test awaits the
  tracker.)
- With no runtime: a guard with a ticket for a `RedisLimitStore` drops on a plain `std::thread`
  with no runtime; `gateway_limit_charges_dropped_total{reason="no_runtime"}` increments, and
  nothing panics.
- Shutdown: a charge task that blocks on a paused listener; `supervise` returns within the 5 s
  bound and adds 1 to `reason="shutdown"`.
- Memory: a guard drops on a plain thread; the charge is applied at once (no spawn).

### 5.9 The gates and the alerts

Run by hand before the push, and record each result in the PR body:

- `repo:redis-connect-single-site` passes on each PR.
- Negative control: add `let _ = redis::Client::open(u)?.get_multiplexed_async_connection();` to a
  gateway `src` file; the gate exits 1 and names that file. Remove it.
- Control: move the file that holds `connect` out of `libs/paigasus-redis/src/`; the gate exits 2.
  Restore it.
- `repo:iam-docker-policy-single-site`: a `var_os("CI")` line added to a gateway test, and one
  added to an IAM test, each red it. Moving the crate's policy file out reds it with exit 2. Remove
  each change.
- `repo:promtool`: both new rules have a firing case and a quiet case in `gateway.test.yml`.

### 5.10 TS tests

- `ts/packages/paigasus-sdk`: the presentation count (67), the `PRESENTATIONS` list (10 members),
  and a `map-error` case for each new reason (`budget-exhausted` → `quota-exhausted`, not
  retryable; `rate-limited` → `rate-limited`).
- `ts/packages/paigasus-proto`: the reason count (67).
- `ts/apps/gateway-console`: `chat-route` sends `stream_options.include_usage: true`; the stream
  relay emits nothing for a usage record with empty `choices`; the error view renders the
  `quota-exhausted` copy with the correlation id.
- `ts/apps/iam-console`: the error view renders the `quota-exhausted` copy.

### 5.11 Proof that the tests bite

Delete or change each of these one at a time and run the suite with `--no-fail-fast` (memory
`mutation-must-compile-to-prove-anything`: each mutation must compile). Each must red at least one
named test:

1. The `admit` call in the handler (→ A1/A2/A3).
2. The principal half of D4 (→ the "reverse" row in § 5.2, on both adapters).
3. The guard's `Drop` charge (→ the disconnect test and the non-stream timeout test).
4. The `usage` scan in the stream (→ "with a usage record", which then sees `estimated`).
5. The `Retry-After` header (→ A1).
6. The `x-should-retry: false` header (→ A3).
7. The `x-should-retry: true` header (→ A1).
8. The `prime_metrics()` call (→ the "zero after boot" test).
9. The D8 precedence (swap the order → the precedence unit test).
10. The poison recovery (→ the poison test).
11. The script's "write nothing on a refusal" branch: increment before the check (→ the D4 rows on
    the Redis run).
12. The `EXPIRE` on the rate keys (→ the rate TTL test).
13. The `EXPIRE` on the budget key (→ the budget TTL test: `TTL = -1`).
14. The `+ 86400` in `charge_ttl` (→ the budget TTL test's lower bound, and the `charge_ttl` unit
    rows).
15. The fail-open arm: return the store error as `500` instead (→ A12 and the open-breaker test).
16. "No ticket → no charge": give the fail-open guard a ticket (→ the A12 test, which sees a
    `charge` call).
17. The D2 refusal: admit an unscoped request when a budget is configured (→ the D2 `500` test).
18. The `Handle::try_current` check: call `tokio::spawn` directly (→ the no-runtime test, which
    then panics).
19. The caller-supplied metric names: hard-code the gateway names in the lib (→ the IAM name-pin
    test, § 5.7).
20. The `EVALSHA` fallback: replace `Script::invoke_async` with a bare `EVALSHA` (→ the `NOSCRIPT`
    test).
21. The error mapping: map every error to `Io` (→ the mapping unit test and the `OOM` test).
22. Each new alert rule: delete it (→ its `promtool` firing case).

Record the result of each in the PR body.

## 6. Known limits

- **Per replica, memory backend only.** With `backend = "memory"`, every replica keeps its own
  counts. With N replicas behind a round-robin balancer, the effective limits are about N times
  the configured values, and a restart or a rollout sets every budget use to zero. A `monthly`
  budget (the default, D6) is then not an invoice cap. The chart's `replicas` comment
  (`charts/paigasus/values.yaml:105-106`, "stateless and horizontally scalable") changes to say
  that limits need `backend = "redis"` for more than one replica.
- **Fail-open spend (D10).** With `backend = "redis"`, while Redis is down, answers with an error,
  or the breaker is open, no rate limit and no budget applies, and those tokens are not charged.
  The exposure is "the upstream's own rate limit × the outage duration". The two alert rules
  (§ 4.8) page the operator; the `chat completion metered` line with
  `outcome = "store_unavailable"` records the unmetered spend per org.
- **Redis durability and eviction (D21).** An evicted or lost budget key resets that org's budget
  silently. The requirements: a dedicated instance or logical database, `noeviction` or enough
  headroom, AOF if budgets must survive a Redis restart. An asynchronous-replica failover can lose
  the last charges.
- **Lost charges.** A charge that fails is not retried (D10). A charge that runs past the 5 s
  shutdown bound, or that has no runtime, is dropped and counted (D16). All of these under-count;
  none over-counts.
- **Clock skew (D18).** Replicas use their own clocks. Near a window edge, skew moves the estimate
  by about `skew / 60 s` of one window's count. Near a period edge, a request can count in the
  neighbouring period.
- **Overrides are per replica config (D12).** Two replicas with different `[[limits.org]]` lists
  apply different limits to one shared count.
- **Topology (D18).** The Redis URL must always reach the primary. Redis Cluster is not supported.
- **Boot needs Redis (D17).** With `backend = "redis"`, a replica does not start while Redis is
  down, so a scale-up or a rollout stalls during a Redis outage (Q14).
- **Soft edge (D7).** Requests in flight when the budget runs out still complete and are charged.
  The overshoot is about "org rate × request duration × tokens per request" (Q12).
- **Estimates (D14).** A client that does not send `include_usage` is charged an estimate. The
  record count matches the token count for OpenAI's one-token chunks; an upstream that batches
  tokens per chunk is under-charged by its batch factor. A timeout or disconnect before any
  answer charges only the request estimate, although the upstream may have generated tokens.
- **The console always sends `stream_options`.** After A10, every playground stream request has
  `stream_options: {"include_usage": true}`. An OpenAI-compatible upstream (SMA-558) that rejects
  an unknown field then refuses every playground stream. vLLM and LiteLLM accept the field. Q19
  asks whether the console needs a switch.
- **One member can use the whole org budget.** There is no per-principal token cap (Q10).
- **Refused requests still cost IAM calls.** Authentication runs before the limit, so a refused
  request still costs 2-3 IAM RPCs. A rate-limited client still loads IAM at its full rate. The
  refusal log line is at `debug` to keep the logs small.
- **Unauthenticated floods are not limited here.** A flood of bad credentials still reaches IAM.
  That is an ingress concern, not this issue.
- **Hot-path latency during a Redis blackhole (D20).** A few requests per 2 s window wait about
  2.1 s.

## 7. Documentation changes

- `gateway.toml.example`: the `[limits]` section (with `backend`, the `GATEWAY_LIMITS__REDIS_URL`
  note, the D21 Redis requirements, the minimum Redis 6.2, the TOML-only note for
  `[[limits.org]]`, and the "same list on every replica" note) and a rewrite of the M0 note at
  lines 4-6.
- The crate has no `README.md` (checked 2026-09-27). The crate doc in `src/lib.rs` and the
  `[limits]` comments in `gateway.toml.example` carry the two codes, the backend choice, the
  fail-open policy, and the `include_usage` advice.
- `docs/ops/RUNBOOK-observability.md`:
  - § 2.3: the seven metric families and the `chat completion metered` log line.
  - Every reference to IAM's `redis_conn` module changes to `paigasus-redis`: `:1782`, `:1860`,
    `:1872`, `:1967`, `:2076`, `:2240`, `:2704`, `:2715`. (`:1860` names the test
    `redis_conn::tests::a_blackholed_backend_costs_seconds_per_command_until_the_breaker_opens`,
    which moves to the lib with its new path.)
  - A new section, "Gateway limits: fail-open and Redis requirements": what fail-open means for
    spend; how to read the store metric (by `kind`), the breaker gauge and the two alerts; the D21
    requirements (dedicated instance or logical database, `noeviction`, AOF, failover loss); and
    how to find unmetered spend from the log line.
- `rs/CLAUDE.md:26-30` and `docs/dev-setup.md:67`: the IAM Docker binary counts (D22), and a note
  that the Docker-skip policy now lives in `paigasus-test-docker`.
- `charts/paigasus/values.yaml`: the `replicas` comment (§ 6).
- `CHANGELOG.md` files are written by release-plz from the commits, not by hand.

## 8. Files expected to change

| File | Change |
|---|---|
| `rs/crates/libs/paigasus-redis/{Cargo.toml,moon.yml,src/**}` | new crate (D15, § 4.10) |
| `rs/crates/libs/paigasus-test-docker/{Cargo.toml,moon.yml,src/**,tests/**}` | new crate (D22) |
| `rs/Cargo.toml` | two literal `members` lines; two workspace dependencies |
| `rs/Cargo.lock` | the two crates; `sha1_smol` |
| `rs/release-plz.toml` | two `[[package]]` entries |
| `rs/.config/nextest.toml` | the gateway preflight and Redis-binary overrides (D22) |
| `rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs` | deleted; `RedisRole` and `From<RedisRole> for BreakerMetrics` move to a small IAM module |
| `rs/crates/services/paigasus-iam/src/adapters/{mod.rs,api_keys/cache.rs,oidc/redis_cache.rs,http/mod.rs,authz/generation.rs,authz/entity_cache.rs,authz/decision_cache.rs}` | import paths; the name-pin test |
| `rs/crates/services/paigasus-iam/tests/support/docker.rs`, `tests/support_docker_{policy,retry}.rs`, `tests/docker_preflight.rs` | policy moved out; two binaries moved; canary counts |
| `rs/crates/services/paigasus-iam/{Cargo.toml,moon.yml}` | both crates as dependencies or dev-dependencies; `dependsOn`, `upstreams` |
| `contracts/proto/paigasus/common/v1/error.proto` | two reasons |
| `rs/crates/libs/paigasus-proto/src/generated/**`, `py/packages/paigasus-proto/**/generated/**`, `ts/packages/paigasus-proto/src/generated/**` | regenerated |
| `rs/crates/libs/paigasus-proto/src/error.rs` | gateway code list; count 65 → 67 |
| `ts/packages/paigasus-proto/src/error.test.ts` | count 65 → 67 (`:60`) |
| `rs/crates/libs/paigasus-observability/src/names.rs` | seven names, `ALL` |
| `rs/crates/services/paigasus-gateway/{Cargo.toml,moon.yml}` | `chrono`, `paigasus-redis`, `redis`, `tokio-util` (`rt`); dev: `paigasus-test-docker`, `testcontainers`, `paigasus-redis/test-support`; `dependsOn`, `upstreams`, `mutex` |
| `rs/crates/services/paigasus-gateway/src/config.rs` | `LimitsConfig`, backend, validation, D23 caps |
| `rs/crates/services/paigasus-gateway/src/domain.rs` → `src/domain/mod.rs`, `src/domain/limits.rs` | rules, policy, windows, periods, TTL, port, error, clock |
| `rs/crates/services/paigasus-gateway/src/application/{mod,limits,charge_guard}.rs` | service, guard, `prime_metrics`, log limiter |
| `rs/crates/services/paigasus-gateway/src/adapters/limits/{mod,memory,redis}.rs` | the two stores |
| `rs/crates/services/paigasus-gateway/src/adapters/mod.rs`, `src/lib.rs`, `src/runtime.rs` | modules, crate doc, shutdown drain |
| `rs/crates/services/paigasus-gateway/src/adapters/http/{mod,chat,error,usage}.rs` | state, handler, errors, `UsageScanner` |
| `rs/crates/services/paigasus-gateway/src/main.rs` | boot order, build the store, `prime_metrics`, describe 7 more families |
| `rs/crates/services/paigasus-gateway/tests/{chat_proxy,metrics,docker_preflight}.rs`, `tests/limits_*.rs`, `tests/support/{mod,limits_contract}.rs` | tests, `limits: None` |
| `rs/crates/services/paigasus-gateway/gateway.toml.example` | docs |
| `moon.yml` (root) | the two single-site gates (§ 4.11, D22) |
| `ci/affected-graph/run.sh` | `:391`, `:397` lint lists; a `redis->services` case |
| `ci/error-registry/check.py` | `asserts` rows for new files that spell the codes (§ 4.7) |
| `ops/observability/prometheus/rules/gateway.rules.yml`, `rules/tests/gateway.test.yml` | two alerts and their tests |
| `ts/apps/gateway-console/moon.yml` | the e2e input list (§ 4.10) |
| `ts/packages/paigasus-sdk/src/errors/{types,presentation,transport-status}.ts` | `quota-exhausted`, two entries, comments |
| `ts/packages/paigasus-sdk/tests/{presentation,map-error,transport-status}.test.ts` | count, list, comments, cases |
| `ts/apps/{gateway-console,iam-console}/app/_components/error-copy.ts`, `tests/unit/error-views.test.tsx` | the copy row, its test |
| `ts/apps/gateway-console/lib/chat-route.ts` (+ its test) | `include_usage` |
| `charts/paigasus/values.yaml` | `replicas` comment |
| `docs/ops/RUNBOOK-observability.md`, `rs/CLAUDE.md`, `docs/dev-setup.md` | § 7 |

**The chart.** No template change. The chart does not deploy the gateway backend
(`charts/paigasus/values.yaml:95`, `deploy: false`), and its gateway branch passes only
`GATEWAY_HTTP_ADDR` (`charts/paigasus/templates/backend-deployment.yaml:183-186`). The chart has no
Redis for any backend: `redis: {}` (`values.yaml:211`) is the console session store. Wiring
`GATEWAY_LIMITS__*` and a Redis secret into the chart belongs with the issue that sets
`deploy: true` for the gateway (§ 10). The goldens in `charts/paigasus/tests/golden/` change only
if the `replicas` comment reaches the rendered output; `:helm-render` decides.

**kind.** No change. kind runs an nginx stub, not the gateway (`ci/kind/manifests/gateway-stub.yaml`,
`ci/kind/README.md:37`). The Redis tests run with testcontainers.

**Dependencies.** New crates in `rs/Cargo.lock`: `paigasus-redis` and `paigasus-test-docker`
(in-tree) and `sha1_smol` (D19). `tokio-util` (D16), `testcontainers` 0.27 and
`testcontainers-modules` 0.15 are already in the lock (`rs/Cargo.lock:5560-5561`;
`paigasus-iam/Cargo.toml:139`, `:144`). `repo:machete` sees every new dependency used.

Gates to run before each push (root `CLAUDE.md`, the full graph): at least `:build :test :lint
:fmt`, `:deny` and `:osv` (`sha1_smol`), `:machete`, `:affected-smoke` (the literal members and the
`upstreams` groups), `:input-liveness` (the new gate inputs), `:redis-connect-single-site`,
`:iam-docker-policy-single-site`, `:observability-drift`, `:promtool` (the two alerts), `:breaking`
(a new enum value is additive), `:error-code-single-site`, `:pyo3-stub-drift`, `:ruff-ci` (the
Python bindings regenerate), `:typecheck` (the TS bindings, the SDK union and both console copy
tables), `:publish-metadata` and `:version-lockstep` (the new `publish = false` crates),
`:helm-render` (the values comment), and `:test-e2e` (the console playground). Several of these
need a specific bash on the development Mac (root `CLAUDE.md`, "This development Mac only").

## 9. Rollout

Nothing changes for a deployment without `[limits]`. IAM's behavior and metrics do not change
(A13). The console and the SDK change in the same PR as the error codes, so they ship with or before
the gateway change (§ 4.9). An operator who adds `[limits]` gets the refusals at once; a restart
applies it. With `backend = "redis"`, the Redis must meet D21 and be reachable at boot (D17). No
migration: the Redis keys are new and expire by themselves.

## 10. Out of scope (follow-up issues to open)

- A cost budget in currency, with a price table per model (D5).
- A runtime API or IAM entity for per-org budgets (D12).
- A per-org time zone and reset time for the budget period (D6). LiteLLM offers both.
- A soft budget (warn at a percentage), and dashboard panels for the new metrics. (The two
  fail-open alerts are in this issue, § 4.8.)
- `x-ratelimit-*` response headers on success. The gateway does not forward the upstream's own
  headers today, so adding its own would be a new contract.
- A token-per-minute (TPM) limit, a per-principal token cap (Q10), and a per-org in-flight cap
  (Q12), if Sven does not fold them in.
- Chart support for the gateway's `[limits]` and Redis, with the issue that deploys the gateway
  backend (§ 8).
- Redis Cluster support, if Q16 asks for it.

## 11. Open questions

- Q1. Answered 2026-10-02: the Redis store is in this issue (D1).
- Q2. Answered 2026-10-02: the sliding-window counter stays (D3). A Notion ADR is written before
  the plan.
- Q3. Is a token budget (D5) enough, or must the first budget be in currency? Recommendation:
  tokens now, currency as a follow-up.
- Q4. Answered 2026-10-02: fail-open, with a metric and a rate-limited log (D10).
- Q5. Off by default (D11), or on by default with conservative values? Off keeps A6; on protects a
  deployment whose operator forgets the table. Recommendation: off.
- Q6. Answered by revision 4: the port is async, and `charge` is a non-blocking call that the
  Redis adapter spawns (D16).
- Q8. Confirm the OpenAI `type` for the rate refusal (`requests`) before the code is written. The
  budget `type` stays `insufficient_quota`.
- Q9. Answered 2026-10-02: `daily`, `weekly` (ISO week) or `monthly`, UTC, default `monthly` (D6).
- Q10. Should there be an optional per-principal token cap, so that one member cannot use the whole
  org budget? Not in this spec until answered. Recommendation: a follow-up.
- Q11. Is D2's fail-closed rule right: a request whose scope names no org gets `500 internal` when
  an org limit or a budget is configured? Recommendation: yes. It is a malformed request, not a
  store outage, so D10 does not apply.
- Q12. Should there be a per-org cap on requests in flight, to bound the D7 overshoot? Not in this
  spec until answered. Recommendation: a follow-up.
- Q13. Answered 2026-10-02: a new SDK presentation `quota-exhausted` (§ 4.9).
- Q14. With `backend = "redis"`, the gateway does not start when Redis is down at boot (D17), so a
  scale-up or a rollout stalls during a Redis outage. Accept, or make the boot fail-open too?
  Recommendation: accept. It matches IAM (SMA-473 D10), and a fail-open boot hides a wrong URL.
- Q15. The gateway redacts `redis_url` with its existing `SecretString` pattern, not with IAM's
  `RedactedUrl` (D17). The redaction property is the same. Recommendation: accept.
- Q16. Redis Cluster is not supported, and the URL must always reach the primary (D18), as for IAM
  today. Recommendation: accept.
- Q17. Does the gateway share a Redis with IAM's caches, or use a dedicated one (D21)?
  Recommendation: a dedicated instance, or at least a dedicated logical database, with
  `maxmemory-policy noeviction`.
- Q18. Does the Notion ADR also record the `paigasus-redis` and `paigasus-test-docker`
  extractions? Recommendation: yes. They change IAM and two gates, so they are significant choices.
- Q19. The console always sends `stream_options` (§ 6). Does it need a switch for an upstream that
  rejects unknown fields? Recommendation: no switch now. vLLM and LiteLLM accept the field; add a
  switch only when a supported upstream rejects it.
- Q20. Both PRs (§ 12) carry `sma-677` in the branch name, so both link to SMA-677, and a merge of
  PR 1 can close the issue before the feature exists. Recommendation: a Linear sub-issue for PR 1
  (the extractions), with PR 1's branch named after it. This spec does not create it.

(Q7, "should the console send `include_usage`", is answered: yes, folded as A10 and § 4.9.)

## 12. Delivery shape

This issue is large: two lib extractions that touch IAM and two gates, plus the limit feature with
two adapters, two error codes, an SDK union member, seven metrics and two alerts. This spec
recommends two stacked PRs:

- **PR 1: extract `paigasus-redis` and `paigasus-test-docker`, no behavior change.** The two new
  crates; IAM moved onto both; the caller-supplied metric names and the IAM name-pin test; the
  widened `repo:redis-connect-single-site` and the re-scoped `repo:iam-docker-policy-single-site`
  (both scanning the gateway too, with no gateway hit yet); `rs/Cargo.toml`, the two crates'
  `moon.yml`, the IAM `moon.yml`, `ci/affected-graph/run.sh`, `rs/release-plz.toml`, the RUNBOOK
  `redis_conn` references, `rs/CLAUDE.md` and `docs/dev-setup.md`. Proof: every IAM test passes
  with no assertion change, and `repo:observability-drift` passes with no dashboard edit (A13,
  A14).
- **PR 2: the limits.** Everything else in this spec, on top of PR 1. It adds the `script`
  feature, the gateway `moon.yml` dependency and `upstreams` lines for the two crates, the
  `ts/apps/gateway-console/moon.yml` input lines, the nextest overrides and the gateway canary, the
  `mutex`, the alerts, and the TS changes.

Reasons: PR 1 is a pure refactor that a reviewer can check against "nothing changes", and it is
the part most likely to break IAM. PR 2 then reviews as a feature only. `main` requires up-to-date
branches, so PR 2 is rebased after PR 1 merges. Q20 recommends a Linear sub-issue for PR 1, so that
PR 1 does not close SMA-677. A third split (memory adapter first, Redis adapter after) was
considered. It is not recommended: the port shape and the contract suite are only proven when both
adapters run it.

## Challenge changelog — round 2 (revision 4)

**Verdict: APPROVE WITH CHANGES.** The coordinator accepted every finding. Each was checked against
the repo on 2026-10-02 and folded in revision 5.

Folded:

- BLOCKER, budget TTL on the Redis clock: relative `EXPIRE <ttl_secs>` from the gateway clock;
  drop and count a charge more than a day late; ARGV loses the instant; TTL test with both bounds;
  mutations 13 and 14 (D18, § 5.3, § 5.11).
- MAJOR, domain depends on `redis`: `Unavailable { kind }` with `Io | Server | Decode`; mapping in
  `adapters/limits/redis.rs`; `kind` metric label; `error` level for non-I/O kinds (D10).
- MAJOR, layering: `Limits`, `prime_metrics` and `ChargeGuard` move to `src/application/`, flat
  like IAM's; paths aligned in § 4.3-§ 4.6 and § 8.
- MAJOR, Redis durability and eviction: D21, § 6, the example config, a RUNBOOK section, Q17.
- MAJOR, no fail-open alert: two rules with `promtool` tests in PR 2 (§ 4.8); removed from § 10.
- MAJOR, Docker test infra: the shared crate `paigasus-test-docker` (coordinator's choice (a)),
  the re-scoped gate with its id kept, nextest overrides, a gateway canary, the `mutex`, pinned
  images, minimum Redis 6.2 tested with 7.4 (D22).
- MAJOR, incomplete change lists: `ci/affected-graph/run.sh:391`, `:397`; `generation.rs`
  (`:33`, `:368`, ten test lines) and the count of 14 production lines in 6 files;
  `impl Into<BreakerMetrics>` with `From<RedisRole>` (coordinator's choice); the iam-console copy
  table and test; `ts/packages/paigasus-proto/src/error.test.ts:60`; the seven extra RUNBOOK lines.
- MINOR, `NOSCRIPT` wording: `SCRIPT LOAD` then `EVALSHA` (D19).
- MINOR, topology wording: "a URL that always reaches the primary" (D18).
- MINOR, contract suite polling: an awaitable `apply_charge`; no polling (§ 5.2).
- MINOR, shutdown loss: a `TaskTracker` drained with a 5 s bound in `runtime::supervise` (D16);
  the follow-up is removed.
- MINOR, boot order: after `paigasus_observability::init` (D17).
- MINOR, the refusal message period: `BudgetExhausted` carries `period: BudgetPeriod` (D8).
- MINOR, key names: `tokens_per_period` and `budget_period` everywhere (D6, § 4.1).
- MINOR, empty policy: no store call, and no connect when every policy is empty (D11).
- MINOR, numeric bounds: caps and a charge clamp, with the arithmetic bound (D23).
- MINOR, lib surface: five `pub` items; the rest `pub(crate)`; the IAM name-pin test uses
  `with_open_breaker_for_tests` under `with_local_recorder` (D15).
- MINOR, error registry: `error.rs` is already an `emits` site; new files get `asserts` rows
  (§ 4.7).
- MINOR, citations: the streaming check is `chat.rs:109-111` and the egress call is `:115`.
- MINOR, `stream_options` risk: stated in § 6, with Q19.
- MINOR, mutations: "no ticket → no charge", the D2 `500`, and `x-should-retry: true` added.
- MINOR, § 12: the gateway `moon.yml` and gateway-console input lines go in PR 2.
- MINOR, the log line: renamed `chat completion metered` with an `outcome`; `debug` when no tokens.
- QUESTIONS: Q14-Q16 kept with recommendations; Q17 (shared or dedicated Redis), Q18 (ADR scope)
  and Q20 (a Linear sub-issue for PR 1) added.

Rejected: none.

## Revision 4 changelog

Sven's decisions on 2026-10-02, and what each one changed:

- Q1 → the Redis store is in this issue. D1 rewritten; new D15-D20; A11-A14; § 4.3, § 4.10, § 4.11,
  § 5.2-§ 5.3, § 5.7-§ 5.9 and § 12 added; the Redis follow-up removed from § 10.
- Q2 → the sliding window stays; the ADR is written before the plan. D3 gains the integer form that
  both adapters share.
- Q4 → fail-open. D10 rewritten: `LimitStoreError` is no longer uninhabited; new metric
  `gateway_limit_store_unavailable_total{op}`; `/readyz` stays IAM-only; the spend risk is in § 6.
- Decision 4 (shared code) → D15: `paigasus-redis`, caller-supplied metric names, the widened
  single-site gate.
- Decision 5 (backend) → D17: `[limits] backend`, `redis_url`, validation, environment variables.
  One deviation: the URL uses the gateway's `SecretString` pattern, not `RedactedUrl` (Q15).
- Q9 → D6: `daily`, `weekly` (ISO week), `monthly`; default `monthly`; one key per org and period
  with a TTL, so no reset job.
- Q13 → § 4.9: the new `quota-exhausted` presentation, with every file that the union touches.
- Q6 (answered by this revision) → D16: an async port with `async_trait`, and a non-blocking
  `charge` that the Redis adapter spawns with `Handle::try_current`.

Facts corrected while re-checking the research for revision 4:

- `redis_conn.rs` has 1250 lines, not 1197 (revision 3) or about 1135.
- IAM's use of `redis_conn` was undercounted in the research. Revision 5 corrects the count again:
  14 production lines in 6 files, `authz/generation.rs` included (D15).
- `SMA-705 D1` ("readiness does not check Redis") is about the console pods
  (`docs/ops/RUNBOOK-chart.md:538`), not IAM. IAM's `/readyz` pings only the database
  (`paigasus-iam/src/adapters/http/mod.rs:1016-1041`). D10 cites the code, not SMA-705.
- The `Presentation` union already has `rate-limited`. It has nine members.
- The IAM Redis test helper's default image is `redis:5.0`, not 7.x. D18 keeps the script within
  Redis 5.0 commands.
- The `PRESENTATION` table is at `presentation.ts:20` (one research note said `:17`).

## Challenge changelog — round 1 (revision 2)

**Verdict: APPROVE WITH CHANGES.** Each finding was checked against the repo code on 2026-09-27.

Folded:

- BLOCKER, the stream estimate. Verified: the console sends `{ model, messages, stream: true }`
  (`chat-route.ts:206`). The byte formula is replaced by a record count and a text-only request
  estimate (D14). Q7 is folded as A10. § 6 and § 5.3 are corrected, with a real-shaped chunk
  fixture.
- BLOCKER, the TS SDK table. Verified: `presentation.ts:20` is a total `Record`,
  `presentation.test.ts:13` asserts 60, `paigasus-proto/src/error.rs:233` asserts 60, and the two
  "nothing emits 429" comments exist. Added § 4.9, the file rows, the rollout order, and Q13.
- MAJOR, no charge on a non-stream timeout or disconnect. One charge guard for both paths (A4,
  § 4.5, § 4.6). A connect failure and a non-2xx answer charge zero. Note: `OpenAiError::Timeout`
  also covers a connect timeout, so a connect timeout charges the request estimate. This is
  accepted and stated.
- MAJOR, SDK retries on `budget-exhausted`. `x-should-retry` headers added (D8, A1, A3, tests).
- MAJOR, D3's Redis argument. The false rationale is withdrawn. D3 now says both algorithms need
  Lua on Redis. Q2 now recommends the ADR before the plan. This agent did not write the ADR: it
  needs Sven's algorithm choice.
- MAJOR, no per-org record for streams. The guard emits `chat completion charged` with explicit
  ids (§ 4.6). (Revision 5 renamed the line `chat completion metered`.)
- MAJOR, "cannot fail" is false. D10 now has poison recovery, saturating arithmetic, the clock
  clamp, and a no-panic `Drop`, with tests.
- MAJOR, A7 cannot be tested. `prime_metrics()` is a library function, the limit metric tests have
  their own binaries and use deltas, and mutation 7 deletes the priming.
- MINOR, precedence and `Retry-After`. D8 precedence; check and commit split (D4).
- MINOR, A1 wording. A1 and D3 are defined through the estimator.
- MINOR, `GatewayError` is `Copy`. Verified at `error.rs:51-52`. `Copy` fields, `chrono` added,
  the retryability test row added.
- MINOR, `Drop` on the unfold state. Verified: the closure destructures `StreamState` by move. The
  guard is a separate field; an exactly-once test is added.
- MINOR, D2. Verified: `TenancyNodeRef` has only three variants. `warn` plus fail-closed when an
  org limit is configured (Q11). Canonical PRN fixtures added.
- MINOR, the test harness. Verified: `MockState` stores only the last request. A counter, a store
  accessor or recording fake, and a contract suite are added.
- MINOR, A6 wording. Restated as a behavior guarantee; the `AppState` literals are listed.
- MINOR, scanner details. `"usage":{` match, three line endings, the non-stream formula, and the
  rolled-over window rule (D7).
- MINOR, the chart and the docs. Verified: `values.yaml:93-96` has `replicas: 2` and the
  "stateless" comment; `describe_gateway_metrics` says 7 families. All three are in § 7 and § 8.
- MINOR, `rate_limit_error`. Changed to `requests`, with a verification step (Q8).
- MINOR, refused requests still cost IAM calls. Stated in § 6; the refusal log line is `debug`.
- MINOR, layering. An injected application service with a clock (§ 4.3a); `LimitRules` keeps the
  config type out of the domain; `org_of` stays in `domain/mod.rs`.
- MINOR, D10's `Err` arm. `LimitStoreError` is uninhabited. (Revision 4 replaced this: D10 is now
  fail-open.)
- QUESTIONS. The window default is now `daily` (D6, Q9). (Revision 4 changed the default to
  `monthly`, per Sven.) Exempt orgs get `exempt = true` (D11). The env limitation for
  `[[limits.org]]` is stated (D12). The per-principal token cap (Q10) and the in-flight cap (Q12)
  are open questions and follow-ups.

Rejected: none. The only partial item: the challenger asked for the ADR before code. The spec
recommends it (Q2) but does not block on it, because the ADR needs a decision from Sven that an
unattended agent cannot make. (Sven decided on 2026-10-02; see the revision 4 changelog.)
