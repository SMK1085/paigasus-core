# SMA-476 — Circuit-break a known-down Redis

**Status:** design (revised after adversarial review)
**Date:** 2026-08-06
**Issue:** [SMA-476](https://linear.app/smaschek/issue/SMA-476/iam-circuit-break-a-known-down-redis-a-blackholed-backend-still-costs)
**Project:** Paigasus IAM
**Follows:** [SMA-473](https://linear.app/smaschek/issue/SMA-473/iam-bound-the-redis-client-retry-budget-an-outage-costs-20-30s-per) (whose D7 deferred exactly this)

## 1. Problem

SMA-473 capped the reconnect retry budget in `adapters::redis_conn` (`number_of_retries`
6 → 1). That bounds a Redis outage **only when each connect attempt errors immediately** —
a stopped or refused backend, where `ECONNREFUSED` comes back instantly. It is measured:
0.2–0.6 s per authz decision, down from 19–28 s.

It does not bound a **blackholed** backend: SYN dropped rather than refused (a network
partition, a wedged host, a firewall/ACL blackhole). There no attempt errors early, so every
attempt runs to `connection_timeout`, and the per-command cost is dominated by that timeout
rather than by the retry schedule.

|  | per failed command | self-query decision (2–3 cycles) | gated cross-principal decision (4–6 cycles) |
| -- | -- | -- | -- |
| stopped/refused (fixed by SMA-473) | ~100–200 ms | 0.2–0.6 s | 0.4–1.2 s |
| blackholed (this issue) | **~2.1 s** | ~4–6 s | ~8–13 s |

The ~2.1 s is `2 × connection_timeout` (two attempts at 1 s each, since one retry means two
attempts) plus one jittered `min_delay` sleep of 100–200 ms. **It has never been measured** —
nothing in this repo has ever been run against a blackholed backend.

### 1.1 Why capping retries cannot fix this, and why lowering the timeout is the wrong knob

The retry cap removed the *schedule* (`100+200+400+800+1600+3200 ms`). What remains is the
per-attempt `connection_timeout`, which is 1 s and which SMA-473 D1 deliberately left alone.
Lowering it to, say, 250 ms would take the blackholed shape to ~0.6 s per command — a real
improvement for one line of code, and D1's reasoning (the timeouts were not what caused the
measured 19–28 s) does not forbid revisiting it.

It is nevertheless declined here (D2). `paigasus-iam` may be pointed at a **remote or managed**
Redis — Upstash, cross-AZ Elasticache, TLS over the public internet — where a cold dial (TCP
handshake + TLS + `HELLO`/`AUTH` round-trip) can plausibly exceed 250 ms while the backend is
entirely healthy. A tightened global timeout would convert that into spurious connect failures,
and with a breaker in place, into spurious breaker trips.

### 1.2 What the code actually looks like today

`adapters::redis_conn::connect` is the sole `ConnectionManager` construction site, enforced by
the `repo:redis-connect-single-site` CI gate. It builds connections and nothing else — it has
no view of commands. Five adapters hold a `ConnectionManager` directly and issue commands
through redis-rs's `AsyncCommands`:

| adapter | commands | error posture |
| -- | -- | -- |
| `authz::generation::Generations::Redis` | `get`, `incr` | propagates `AuthzError::Backend`; callers degrade |
| `authz::decision_cache::RedisDecisionCache` | `get`, `set_ex` | fail-**open** (error → `None`, a plain miss) |
| `authz::entity_cache::SliceCache` | `get`, `set_ex` | fail-**open** (falls through to the inner loader) |
| `api_keys::cache::RedisApiKeyCache` | `get`, `set_ex`, `del` | fail-**open** (error → `None`) |
| `oidc::redis_cache::RedisJwksCache` | `get`, `set_ex` | fail-**closed** (`AuthnError::Unavailable` → 503) |

Eleven call sites, four distinct commands. That is the whole surface a breaker must cover.

### 1.3 Connections are not always shared

`AppState::new` opens **one** `ConnectionManager` when `authz.cache.backend = "redis"` and
shares it across `Generations` + `RedisDecisionCache` + `SliceCache` + `RedisApiKeyCache`.
But when `authz.cache.backend = "memory"` and `api_keys.introspect_cache.backend = "redis"`,
the API-key cache dials its **own** connection from `api_keys.introspect_cache.redis_url`
(`adapters/http/mod.rs:565-576`). `RedisJwksCache::connect` always dials its own, from
`authn.jwks_cache.redis_url`.

So there are up to three independent connections against up to three independent backends. A
single global breaker would be wrong: one backend going down would short-circuit traffic to
two healthy ones.

Two pre-existing behaviours this design inherits rather than changes, both of which the RUNBOOK
must state because D10's metric labels depend on them:

- When an authz Redis connection exists, the API-key cache **reuses it and ignores
  `api_keys.introspect_cache.redis_url` entirely**, even when that URL points somewhere else.
- Two handles may point at the *same* physical Redis (the ordinary deployment, where
  `authz.cache` and `authn.jwks_cache` are both `redis` against one server). Their breakers are
  independent and will open and recover at different times.

### 1.4 The `ConnectionManager` fact that drives the whole design

`ConnectionManager` does not hold a connection. It holds a **memoized future**:

```rust
connection: ArcSwap<SharedRedisFuture<MultiplexedConnection>>   // connection_manager.rs:335
type SharedRedisFuture<T> = Shared<BoxFuture<'static, RedisResult<T>>>;  // :387
```

`send_packed_command` loads that guard, clones the `Shared` and awaits it (`:681`). A
`futures_util::future::Shared` **caches its output**: once the underlying dial resolves, every
later await returns the cached result instantly. On an error, `reconnect_if_io_error!` CAS-swaps
in a fresh dial future and **spawns it detached** (`:649`) — it runs to completion whether or not
anyone is waiting.

Two consequences, both of which the first draft of this spec got wrong in opposite directions:

1. **Back-to-back commands each pay a full dial.** Command N's failure spawns dial N+1 at time
   T; command N+1 arrives at ~T and awaits an in-flight dial, paying the full ~2.1 s. This is
   why §1's table is right, and why the existing RUNBOOK text about burning "a full cycle per
   failed command" is right — *for commands issued faster than a dial completes*.
2. **A command issued after a quiet gap pays nothing.** If the breaker has been open for longer
   than a dial takes, the detached dial has already resolved and memoized its `Err`. The
   half-open probe loads that resolved future and gets its answer in microseconds, **without
   touching the network** — then swaps in yet another fresh dial whose result only becomes
   visible to the *next* probe.

Consequence 2 is what makes the recovery model in D7 what it is, and it is the single most
surprising property in this design.

## 2. Decisions

### D1 — A circuit breaker, scoped per connection, not per process

One breaker per `ConnectionManager`, `Arc`-shared so every clone of a handle observes the same
state. This is the only scoping that survives §1.3's split configuration: a breaker follows the
connection it protects, so a down JWKS Redis cannot short-circuit a healthy authz Redis.

`RedisHandle: Clone` must share the breaker through an `Arc`. This is load-bearing and easy to
break silently — every one of the eleven call sites does `let mut conn = self.conn.clone()` per
command, so a `#[derive(Clone)]` over a non-`Arc` breaker field would compile and give every
call its own breaker, which would never open. §4.1 pins it with a test.

### D2 — `connection_timeout` and `response_timeout` stay at the redis-rs defaults

Per §1.1: the deployment topology is not constrained by anything in this repo (there is no IAM
deployment manifest), so a global tightening risks false failures against a legitimately-slow
remote backend. SMA-473 D1's pinning test
(`the_tuned_config_leaves_every_other_knob_at_the_redis_rs_default`) stays green and unedited.

No config knob is added either — not for the timeouts, not for the breaker. This follows
SMA-473's precedent of hardcoded constants pinned by loud tests, and avoids a knob whose wrong
setting (threshold 0, a ten-minute window) is worse than no knob at all. The counter-argument —
that an unconstrained topology is exactly what a knob is for — is noted and rejected on scope:
nothing has yet needed it, and adding it later is additive.

### D3 — The breaker intercepts at `redis::aio::ConnectionLike`, not at the call sites

`RedisHandle` — the new type `redis_conn::connect` returns — wraps the `ConnectionManager` plus
the breaker and implements `redis::aio::ConnectionLike`. redis-rs's `AsyncCommands` is a blanket
impl over that trait, so **all eleven call sites keep working verbatim**: `conn.get(&key).await`
compiles and behaves identically. The only change inside the five adapters is a field type
(`ConnectionManager` → `RedisHandle`) and the corresponding constructor signature.

Verified against the vendored source rather than assumed:

- Every call site routes through `req_packed_command` — `Cmd::query_async` calls it at
  `redis-1.3.0/src/cmd.rs:655`.
- The blanket impl is `impl<T> AsyncCommands for T where T: aio::ConnectionLike + Send + Sync + Sized`
  (`commands/mod.rs:3288`). Note **`Sync`**, which the trait's own declaration does not require.
- `RedisFuture<'a, T>` is `Pin<Box<dyn Future + Send + 'a>>` (`types.rs:624`), so `&'a mut self`
  is no obstacle to returning an immediately-ready synthetic error — the box need not borrow.

Two alternatives were considered and rejected:

- **A typed method surface** (`get_bytes`/`get_u64`/`set_ex`/`incr`/`del` on `RedisHandle`).
  Cleaner as an object, but it rewrites all eleven call sites *and their error handling* — which
  is exactly the code AC3 requires to stay unchanged. It would make "the postures are unchanged"
  a claim to re-verify rather than a property preserved by construction.
- **A generic `execute(closure)`** the handle runs. Smaller than the typed surface, but every
  call site still changes shape, and closure-passing reads worse than the transparent decorator.

`ConnectionLike` is also the *correct* interception point rather than merely a convenient one:
`req_packed_command` is where `ConnectionManager` awaits the shared connect future (§1.4).
Nothing can reach that path without going through the breaker.

**The `Send + Sync` bound is a design constraint, not a footnote.** The breaker's lock is a
`std::sync::Mutex` whose guard is **never held across an `.await`** — `admit()` and `record()`
each lock, decide, and drop the guard before any await. A guard held across the inner await would
make the returned future `!Send` and fail to compile; reaching for `tokio::sync::Mutex` instead
would make `admit()` async and change the design. §6 lists the redis-rs coupling as a residual.

### D4 — The open breaker's error is a synthetic `RedisError` of kind `Io`

Short-circuiting must be **indistinguishable from a real connection failure** to every caller,
because that is what preserves five different error postures without touching any of them.
`RedisError::from((ErrorKind::Io, "…"))` satisfies `is_io_error()`
(`errors/redis_error.rs:135-141, 246-261, 321-323`), and all five adapters read only
`err.kind()` — so the synthetic error is genuinely indistinguishable to every one of them.

The message text is pinned by test to a fixed literal —
`"redis circuit breaker open (SMA-476)"` — and must contain **no URL, host, or credential**.
This is not cosmetic: unlike the five adapters, `cedar_authorizer.rs:167` and
`generation.rs:141` log the wrapping `AuthzError` with `error = %err`, i.e. the error's
`Display`, so the literal reaches the logs.

Short-circuits are **not** recorded as breaker failures — only real command outcomes move the
state machine.

### D5 — The failure classifier is `is_io_error()` OR a reconnect-worthy `retry_method()`

```rust
fn counts_as_failure(err: &RedisError) -> bool {
    err.is_io_error()
        || matches!(
            err.retry_method(),
            RetryMethod::Reconnect | RetryMethod::ReconnectFromInitialConnections
        )
}
```

Neither half alone is correct, which is why both are needed:

- **`retry_method()` alone misses the case this issue is about.** A connect timeout becomes
  `io::Error::from(io::ErrorKind::TimedOut)` (`aio/runtime.rs:189-193`), i.e. `ErrorKind::Io`
  that is *not* `is_connection_dropped()`, and `retry_method` maps that to
  `RetryMethod::RetryImmediately` — **not** `Reconnect` (`errors/redis_error.rs:451-464`). A
  blackholed backend produces exactly this error, so a `Reconnect`-only classifier would never
  open the breaker.
- **`is_io_error()` alone misses two kinds redis-rs itself treats as connection-fatal.**
  `ErrorKind::Parse` and `ErrorKind::AuthenticationFailed` both map to `RetryMethod::Reconnect`
  (`redis_error.rs:447-448`) and drive `ConnectionManager` reconnects, but neither is an IO
  error. A desynced connection or a rotated password would otherwise produce an endless
  reconnect loop the breaker never opens on.

Excluded, deliberately: `UnexpectedReturnType`, `Client`, `Extension`, `InvalidClientConfig`,
`RESP3NotSupported`, and every `Server(..)` error. All of them mean the backend answered — it is
healthy, and the fault is ours or the data's. Counting them would let a data bug disable caching
fleet-wide.

A note on mechanism, because it is easy to "simplify" wrongly later: a `WRONGTYPE` never reaches
the breaker as an `Err` at all. `send_packed_command` returns `Ok(Value::ServerError(..))`, and
the conversion to a `RedisError` happens one layer up in `Cmd::query_async` via
`val.extract_error()?` (`cmd.rs:655`). The classifier is correct, but not for the reason "server
errors are excluded" suggests — they were never included.

`ErrorKind` is `#[non_exhaustive]`, so the classifier must not be written as an exhaustive match.

### D6 — Three consecutive failures opens it

`FAILURE_THRESHOLD = 3` consecutive failures; any success resets the counter to 0.

Three rather than one, because SMA-473 chose `number_of_retries = 1` specifically to tolerate a
first connect attempt landing in a **failover gap**; opening on a single failure would defeat
that.

Stated honestly, because the first draft of this spec had it both ways: **under real
concurrency, a routine Redis failover will trip this breaker.** Three concurrent requests failing
together is not a high bar, and a failover under load produces far more than three. What the
threshold buys is protection against a *serial, low-traffic* blip, not against a failover. The
consequence is a bounded period of cache bypass (fail-open handles) or 503s (the JWKS handle)
after every failover — quantified in D7 and listed as residual risk #2 in §6.

The wall-clock exposure before the breaker opens is not `3 × 2.1 s` in general: under load the
three failures land near-simultaneously (~2.1 s total), while under strictly serial traffic it is
~6.3 s. Both figures are documented rather than one being presented as *the* number.

### D7 — Recovery takes up to two open windows, not one — so the window is 2 s

`OPEN_DURATION = 2s`.

After the window expires, exactly one probe is admitted (D8). Per §1.4's consequence 2, that
probe consumes the **already-memoized** result of the dial spawned when the breaker opened —
instantly, without touching the network — and only *then* swaps in a fresh dial. So the first
probe after Redis recovers still sees the stale `Err` and re-opens; the *second* probe sees the
fresh success and closes.

```
recovery ≤ 2 × OPEN_DURATION + one dial ≈ 2 × 2 s + ~2.1 s ≈ 6 s worst case
```

This inverts the cost model the first draft used, in both directions:

- **Probes are essentially free**, not ~2.1 s each. There is no steady-state probe cost to
  amortise, which is what makes a *short* window affordable.
- **Recovery is two windows**, so the window must be short for the total to stay reasonable.

Hence 2 s rather than the 5 s a "probes cost 2.1 s" model would have argued for. The only cost
of a shorter window is one detached dial per window (0.5/s) — negligible.

The residual §1 opened with is therefore reduced but not eliminated: the ~2.1 s per-command cost
still applies to the failures that open the breaker, and to the in-flight cohort (§6 risk #4).
What the breaker removes is that cost on every *subsequent* command.

**No automated test pins the two-window bound.** It is a property of redis-rs's internals, cited
above and re-derived from source, and confirmed once by the §5 manual procedure. §4.5 adds a
hermetic recovery test that pins the observable contract (the breaker does re-close, within a
bounded number of windows) without asserting the exact count, so a redis-rs change that improves
this does not red the build.

### D8 — Half-open admits exactly one probe, and cannot wedge

After `OPEN_DURATION`, the state moves to HalfOpen and **one** caller is admitted, chosen by an
atomic compare-and-swap under the same `std::sync::Mutex`. Every other concurrent caller
**short-circuits immediately** rather than queueing behind the probe — queueing would couple
unrelated request latencies to one probe's dial and rebuild the thundering herd the breaker
exists to prevent.

The probe's outcome is delivered by an **RAII guard** (`ProbePermit`), not by a bare
`record(result)` call. This is not stylistic. The probe runs inside an axum handler, and axum
drops handler futures on client disconnect (`serve_http` also wraps the router in a
`TimeoutLayer`). With a bare call, a probe future dropped mid-await would leave the breaker
HalfOpen **forever**: the CAS has already fired so no second probe is ever admitted, no success
ever resets it, and every command short-circuits for the remaining process lifetime — a silent,
permanent cache bypass, and on the JWKS handle, permanent 503s until restart.

`ProbePermit` is issued for **every** admitted command, in every state, not only half-open
probes — so what `Drop` does with an unreported permit has to be right in Closed and Open too, not
just in HalfOpen. A dropped permit means **no result was ever observed**: it is not evidence about
the backend at all, only about the caller (a client disconnect, or `serve_http`'s
`TimeoutLayer`). Treating "no information" as "failure" is a category error, and in the Closed
state it is actively harmful — axum drops handler futures on client disconnect, so three
*cancelled*, not failed, client requests would trip `FAILURE_THRESHOLD` and open the breaker
against a perfectly healthy Redis; on the fail-closed JWKS handle that is `OPEN_DURATION` (2 s)
during which every token-authenticated request 503s for a reason that has nothing to do with
Redis. (Surfaced by code review; the fix is `Drop` scoping its action to HalfOpen, below.)

**HalfOpen is the one state where an abandoned permit is a wedge hazard**, for the reason above:
the CAS has already fired, so nothing else will ever re-arm that window before
`HALF_OPEN_DEADLINE`.

Two independent defences, because this failure mode is severe and silent:

1. The guard's `Drop` records a **failure**, re-opening the window — but **only when the breaker
   is currently HalfOpen**. In Closed and Open, `Drop` is a no-op: there is no wedge to guard
   against, and counting the drop would be the category error above.
2. A staleness deadline: `HALF_OPEN_DEADLINE = 5s`. If the state has been HalfOpen longer than
   that, another probe is admitted regardless. Five seconds comfortably exceeds a worst-case
   ~2.1 s dial, so it never pre-empts a probe that is merely slow.

A probe failure re-opens immediately for another full window.

Timing uses `std::time::Instant` (monotonic). No `Clock` port injection: the breaker measures
elapsed durations, never wall-clock instants, so there is nothing for a test clock to control
that parameterizing the durations does not already handle.

### D9 — The breaker covers every handle, including the fail-closed JWKS one

Uniform coverage. `RedisJwksCache` still returns `AuthnError::Unavailable` on every Redis error
— the fail-closed posture is unchanged — it just returns it instantly instead of after ~2.1 s.

The asymmetry, stated with the corrected numbers: for the fail-**open** caches an open breaker is
pure win (a fast bypass instead of a slow one). For the fail-**closed** JWKS path it also means
up to ~6 s (D7) during which **100% of token-authenticated requests 503**, including after Redis
has recovered — `RedisJwksCache::get`'s error propagates through `JwksProvider::key_for`
(`adapters/oidc/jwks.rs:207,216`) with no fallback. Combined with D6, a routine failover under
load now costs a ~6 s token-auth outage where today it costs only the in-flight requests.

That is accepted deliberately, and it is why `OPEN_DURATION` is 2 s rather than 5 s. Carving JWKS
out was reconsidered after the recovery bound was corrected and still rejected: it would leave
the one path where latency becomes a hard 503 paying the full blackhole cost on every request,
and would make `redis_conn` non-uniform. The mitigation is detection, not exclusion — hence D10's
separate critical alert for `role="jwks"`.

### D10 — Two metrics, a closed-set role label, and a severity that follows the posture

`iam_redis_breaker_state{role}` — gauge, `0 = closed`, `1 = half_open`, `2 = open`. **Set to 0
at construction**, not only on transition: an unset gauge renders as "No data", which an operator
cannot distinguish from a broken scrape or an unregistered metric.

`iam_redis_breaker_transitions_total{role, to}` — counter, `to` ∈ `open | half_open | closed`.

The counter is *not* redundant with the gauge, which the first draft claimed. With
`OPEN_DURATION = 2s` against a 15–30 s scrape interval, a breaker that opens for 2 s every 30 s —
a chronically sick backend, which is precisely the condition worth catching early — is sampled as
`0` in most scrapes. `changes(gauge[10m])` undercounts it by construction and the `for:` clause
never holds. The counter is the only artifact that survives a sub-scrape-interval state.

`role` comes from a new `RedisRole` enum — `Authz | ApiKeys | Jwks` — so the label set is closed
by the type system and cannot mint cardinality. `redis_conn::connect` grows a `RedisRole`
parameter; the five adapters' `connect` entry points pass their own.

Both families get a `describe_gauge!`/`describe_counter!` registration in `main.rs` alongside
every other IAM metric, and both go into `paigasus-observability::names::ALL`.

Three RUNBOOK caveats, all consequences of §1.3 rather than of this design:

- `role="api_keys"` exists **only** in the split configuration. Ordinarily the API-key cache
  reuses the authz handle and its commands are attributed to `role="authz"` — a missing
  `api_keys` series does not mean the API-key cache is idle.
- Two roles may front the **same physical Redis** with independent breakers, so `role="authz"`
  at 0 while `role="jwks"` is at 2 does not imply two backends.
- The gauge is per-replica: aggregate `max by (job, role)`, never `sum`.

### D11 — The boot dial is not breaker-mediated

`redis_conn::connect` stays eager (`ConnectionManager::new_with_config` awaits the initial
connection), so `AppState::new` still fails fast when Redis is down at boot (SMA-473 D10). The
breaker starts Closed and wraps **commands only** — a single boot dial has nothing to break on,
and a process that is about to exit has no state worth recording.

If SMA-473 D10's deferred retry-loop-at-boot ever ships, this decision must be revisited
together with it: a boot that retries is a boot that *can* accumulate failures, and whether those
should seed the breaker is a real question. It is out of scope here, and the RUNBOOK §6 bullet
says so.

### D12 — The CI gate gets stronger, not just preserved

Once no adapter names `ConnectionManager`, `repo:redis-connect-single-site` bans the **type
name** outside `redis_conn.rs` in addition to the constructors it already bans. That turns
"the breaker is used everywhere" from a convention into a structural property: there is no way
to obtain a connection that bypasses it, because there is no way to name one.

The gate's comment filter anchors on `:[0-9]+:[[:space:]]*//` (`moon.yml:192`), so a `/* … */`
block comment naming `ConnectionManager` would trip the new ban. The gate's own comment block
already documents its portability traps; this one joins them.

The two `#[cfg(test)]` sites that legitimately call `ConnectionManager::new_lazy_with_config`
(`entity_cache.rs:233`, `api_keys/cache.rs:386`) move to the test-only constructors below, so
they exercise the production config *and* the production breaker rather than a raw manager. The
gate's existing "must name `connection_manager_config()` on the same line" rule is retired along
with them, since the type ban subsumes it.

### D13 — `RedisApiKeyCache::from_connection` narrows to `pub(crate)`

`adapters/mod.rs:15` declares `pub(crate) mod redis_conn`, so `RedisHandle` is a crate-private
type. `RedisApiKeyCache::from_connection` is `pub` (`api_keys/cache.rs:221`); changing its
parameter to `RedisHandle` would be a private-type-in-public-interface, which
`cargo clippy --workspace -- -D warnings` fails the build on.

It narrows to `pub(crate)`. Its only callers are in-crate (`http/mod.rs:577` and its own
`#[cfg(test)]` module); no `tests/*.rs` uses it. The alternative — promoting `redis_conn` to
`pub mod` and re-exporting `RedisHandle`/`RedisRole` — would widen the public API of a service
binary for no consumer.

Note this differs from `MemoryGenerations`, whose `pub`-with-private-fields posture exists for
the same lint but is *not* a precedent here: that type lives in a `pub mod`
(`adapters::authz::generation`), so its situation is not analogous. `RedisHandle` itself is
`pub(crate)` with private fields, and `Generations::Redis(RedisHandle)` — a public variant of a
public enum — needs the same treatment `MemoryGenerations` got, i.e. `RedisHandle` must be `pub`
*within* a `pub(crate)` module, which does not leak.

## 3. The fix

### 3.1 `adapters/redis_conn.rs` — the breaker and the handle

```rust
pub(crate) const FAILURE_THRESHOLD: u32 = 3;
pub(crate) const OPEN_DURATION: Duration = Duration::from_secs(2);
pub(crate) const HALF_OPEN_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedisRole { Authz, ApiKeys, Jwks }   // as_label() -> &'static str

#[derive(Clone, Copy, PartialEq, Eq)]
enum BreakerState { Closed, Open, HalfOpen }

struct Breaker {
    role: RedisRole,
    open_duration: Duration,        // fields, not consts, so tests use 50ms
    half_open_deadline: Duration,
    inner: std::sync::Mutex<Inner>, // { state, consecutive_failures, changed_at }
}
```

`Breaker::admit()` locks, applies D7/D8's transitions, drops the guard, and returns either a
short-circuit decision or a `ProbePermit` RAII guard. `ProbePermit::record(&RedisResult<_>)`
classifies via D5 and consumes the guard; if `record` was never called, its `Drop` records a
failure **only when the breaker is currently HalfOpen** — in Closed and Open it is a no-op,
because an unrecorded drop is not evidence about the backend (D8). Any state change sets the
gauge and increments the transitions counter (D10). The mutex guard is never held across an await
(D3).

`RedisHandle` is `pub` inside the `pub(crate)` module, with private fields, `Clone` (sharing the
breaker `Arc` — D1), and implements `ConnectionLike`:

```rust
impl redis::aio::ConnectionLike for RedisHandle {
    fn req_packed_command<'a>(&'a mut self, cmd: &'a Cmd) -> RedisFuture<'a, Value> { … }
    fn req_packed_commands<'a>(&'a mut self, p: &'a Pipeline, off: usize, n: usize)
        -> RedisFuture<'a, Vec<Value>> { … }
    fn get_db(&self) -> i64 { self.conn.get_db() }
}
```

`req_packed_commands` is implemented for real even though no call site pipelines today — leaving
it `unimplemented!()` would be a live panic behind a trait method redis-rs may call.

`connect(redis_url, role) -> RedisResult<RedisHandle>`.

Two `#[cfg(test)] pub(crate)` constructors, which are **not** interchangeable:

- `new_lazy_for_tests(url, role)` — a real, lazily-connecting handle with a **Closed** breaker
  and short test durations. Used wherever a test must actually dial.
- `with_open_breaker_for_tests(url, role)` — a handle whose breaker is forced **Open**. Used by
  §4.3 to prove short-circuiting without dialling.

Plus `#[cfg(test)] pub(crate) mod test_support`, holding the blackhole/RESP listener (§4.2) so
the four adapter modules and `redis_conn`'s own tests share one implementation.

### 3.2 The five adapters

Field type and constructor signature only:

- `authz::generation` — `Generations::Redis(RedisHandle)`; `redis_connect(url)` passes
  `RedisRole::Authz`.
- `authz::decision_cache::RedisDecisionCache` — `conn: RedisHandle`; `connect` +
  `from_connection`.
- `authz::entity_cache::SliceCache` — same.
- `api_keys::cache::RedisApiKeyCache` — same; `connect` passes `RedisRole::ApiKeys`;
  `from_connection` narrows to `pub(crate)` (D13).
- `oidc::redis_cache::RedisJwksCache` — same; `connect` passes `RedisRole::Jwks`.

Every `conn.get(...)`, `conn.set_ex(...)`, `conn.incr(...)`, `conn.del(...)` and every error
match arm is untouched.

### 3.3 Composition root

`http::connect_redis` takes a `RedisRole` and forwards it. `AppState::new` passes
`RedisRole::Authz` for the shared connection and `RedisRole::ApiKeys` for the split-configuration
dial. `ConnectionManager` disappears from `http/mod.rs`'s imports, which D12's gate then enforces.

### 3.4 `paigasus-observability` + `main.rs`

`IAM_REDIS_BREAKER_STATE` and `IAM_REDIS_BREAKER_TRANSITIONS_TOTAL` added to `names.rs` and to
`ALL`, with doc comments covering the 0/1/2 encoding, the closed `role`/`to` label sets, the
per-replica aggregation rule, and D10's three attribution caveats. `describe_gauge!` /
`describe_counter!` calls join the existing block in `main.rs:368-451`, following the
`IAM_OUTBOX_PARKED_ROWS` precedent (`:424-427`) for the "aggregate `max by`, never `sum`" wording.

### 3.5 Alert rules + fixture + dashboard

Three rules in `ops/observability/prometheus/rules/`:

| alert | expression | severity |
| -- | -- | -- |
| `IamRedisBreakerOpen` | `max by (job, role) (iam_redis_breaker_state{role!="jwks"}) != 0` for 2 m | warning |
| `IamJwksRedisBreakerOpen` | `max by (job, role) (iam_redis_breaker_state{role="jwks"}) != 0` for 1 m | **critical** |
| `IamRedisBreakerFlapping` | `sum by (job, role) (increase(iam_redis_breaker_transitions_total{to="open"}[10m])) > 5` | warning |

The severity split follows the posture (D9): an open JWKS breaker is a total token-auth outage,
not a degradation, so `warning` would under-page it. It cannot be left to
`IamAuthzRedisCacheBypassed` as a "critical companion" either — that rule is structurally silent
under `authz.cache.backend = "memory"` (`iam.rules.yml:158-160`), which is exactly the
split-config where a JWKS Redis may be the only Redis.

`!= 0` rather than `== 2` because the gauge legitimately sits at `1` while a probe is in flight.
`IamRedisBreakerFlapping` exists because neither of the first two can fire on a breaker that
opens and closes inside one scrape interval (D10).

The promtool fixture needs a **control series** — a role whose breaker stays at 0 — since an
all-firing fixture cannot distinguish `!= 0` from `>= 0` (the SMA-466 lesson). Fixtures evaluate
at 1-minute resolution (`rules/tests/gateway.test.yml:3,7`), so the fixture models steady states,
not the sub-second probe transition.

One Grafana panel in `ops/observability/grafana/dashboards/iam.json` — breaker state by role —
matching what every other IAM gauge has.

## 4. Tests

### 4.1 Breaker state machine — unit, `redis_conn.rs`

Docker-free, no sockets; a `Breaker` constructed with a 50 ms window.

- Closed passes through; two failures then a success resets the counter (no open at the third).
- Three consecutive counted failures → Open.
- Open short-circuits without admitting, and does not record the short-circuit as a failure.
- After the window: exactly one probe admitted out of N concurrent `admit()` calls; the other
  N−1 short-circuit rather than block (D8).
- Probe success → Closed; probe failure → Open for another full window.
- **A dropped `ProbePermit` re-opens a HalfOpen breaker** — the D8 wedge guard. Written by
  obtaining a permit while HalfOpen and dropping it without calling `record`.
- **A dropped `ProbePermit` does NOT open a Closed breaker** — the D8 correction. Drops
  `FAILURE_THRESHOLD` permits while Closed and asserts the breaker is still admitting; a failure
  here means cancelled client requests can trip the breaker against a healthy backend.
- **A HalfOpen state older than `HALF_OPEN_DEADLINE` admits another probe** — the second D8
  defence, in case the first is ever refactored away.
- Classifier (D5): `ErrorKind::Io` counts; a timeout-flavoured `Io` counts (the blackhole case);
  `ErrorKind::Parse` and `AuthenticationFailed` count; `ErrorKind::UnexpectedReturnType` does
  **not**. (Note `TypeError` does not exist in redis 1.3.0 — the enum is at
  `errors/redis_error.rs:13-50`.)
- `RedisHandle::clone()` shares one breaker: open it through one clone, assert a second clone
  short-circuits (D1).
- Constants pinned: `FAILURE_THRESHOLD == 3`, `OPEN_DURATION == 2s`, `HALF_OPEN_DEADLINE == 5s`,
  with the same "do not relax this assertion" framing SMA-473 used.
- The synthetic error's `Display` equals the pinned literal and contains no host or URL (D4).

### 4.2 The blackhole measurement — unit, in-crate (AC1)

In `redis_conn.rs`'s `#[cfg(test)] mod tests`, **not** under `tests/`: an integration test is a
separate crate linked against the lib built without `cfg(test)`, so it cannot see
`new_lazy_for_tests` at all.

The listener, in `test_support`, is a `tokio::net::TcpListener` whose accept loop **pushes each
accepted `TcpStream` into a `Vec` that lives for the test's duration**. Dropping the stream — the
natural way to write "ignore the socket" — makes the kernel send FIN/RST, redis-rs's setup-pipeline
read returns EOF immediately, and command #1 costs microseconds instead of 2.1 s. The accept
task's `JoinHandle` is retained for the same reason.

This reproduces the blackholed shape because the dial always awaits a server response and the
whole dial sits inside `rt.timeout(connection_timeout)`:

- `connection_setup_pipeline` always appends `CLIENT SETINFO LIB-NAME`/`LIB-VER`
  (`redis-1.3.0/src/connection.rs:1380-1400`).
- `client.rs:495-520` wraps `get_multiplexed_async_connection_inner` — resolver included — in
  `rt.timeout(…)`.

**Documented precondition:** the 1 s-per-attempt bound holds because that setup pipeline is
non-empty. It is guarded by `if !connection_info.skip_set_lib_name`, and an empty pipeline
short-circuits to `Ok` without I/O (`aio/mod.rs:110-112`), which would move the hang to
`response_timeout` (500 ms) instead. A plain `redis://host:port` URL (RESP2, no auth, db 0) keeps
it non-empty.

One test produces **both** numbers, using a lazy handle (the eager production `connect` would
itself hang ~2.1 s and return `Err` before any command could be issued — the same reason the
existing SMA-473 test is built that way):

- Command #1, breaker Closed: assert `1.9s <= elapsed < 3.5s`. This is the measured per-command
  blackhole cost — the figure SMA-476 opened as calculated-only. The **lower** bound is the
  load-bearing half: it is what proves the listener blackholed rather than refused.
- Commands #4 onwards, breaker Open: assert `< 100 ms` each. Not sub-millisecond — that is below
  the scheduler-jitter floor of a contended CI runner, and 100 ms is still 20× discriminating
  against a 2.1 s dial.
- Ten commands aggregate: assert `< 14 s`. Ten un-broken commands cost ~21 s; three failures plus
  seven short-circuits cost ~6.3 s. The bound sits between, with margin on both sides, so it
  fails if the breaker never opens and passes only if it did.

Loose bounds are deliberate and follow the house precedent at `redis_conn.rs:126-129`.

### 4.3 The postures are unchanged — unit (AC3)

In each adapter's own `#[cfg(test)] mod tests`, against a handle from
`with_open_breaker_for_tests` pointed at §4.2's **blackhole listener** — deliberately not at a
closed port. A closed port refuses in microseconds, which is indistinguishable from a
short-circuit; a blackhole would cost ~2.1 s if the command were actually dialled. That is what
makes the elapsed-time assertion load-bearing rather than decorative.

| path | asserted |
| -- | -- |
| `RedisDecisionCache::get` | `None` — a miss, not an error |
| `RedisDecisionCache::put` | returns, swallowed |
| `SliceCache::load` | inner loader called; its value returned |
| `RedisApiKeyCache::get` | `None` |
| `RedisJwksCache::get` | `Err(AuthnError::Unavailable)` — fail-**closed** |

Each also asserts `< 100 ms` elapsed, which is what distinguishes "short-circuited" from
"dialled".

### 4.4 Recovery — unit (D7)

`test_support`'s listener carries an `Arc<AtomicBool>` mode flag. While set, it blackholes
(accept and hold); when cleared, it answers as a minimal RESP server — `+OK` for the setup
pipeline's two `CLIENT SETINFO` commands, `$-1` for a `GET`.

The test drives the breaker Open against the blackhole, clears the flag, and asserts the breaker
returns to Closed within a bounded number of windows. It asserts **a bound, not an exact count**
(D7): the two-window behaviour is a redis-rs internal, and a future version that recovers in one
window should not red the build.

### 4.5 Observability — unit (AC5)

One test asserting `iam_redis_breaker_state` is emitted with the right `role` label and value
after a forced transition, and that the transitions counter increments. `tests/drift.rs`
(`paigasus-observability`) only proves rules→`ALL`; it never proves anything *emits* a family, so
without this AC5 rests on construction claims alone.

### 4.6 Existing suites

`redis_conn.rs`'s four SMA-473 tests stay green **unedited** — D2 changes no config knob.
`a_command_against_an_unreachable_backend_fails_fast` and
`connect_is_eager_so_a_dead_backend_fails_at_construction` now exercise `RedisHandle`; the first
must keep issuing exactly one command so the breaker is still Closed when it measures.

Docker-gated suites that stop a Redis container mid-test and then keep issuing commands —
`tests/authz_cache_redis.rs`, `tests/authz_acceptance.rs` (`redis_cache_backend_fails_open_…`,
and the revocation-during-outage case), `tests/authz_generations_redis.rs`,
`tests/api_key_cache_redis.rs`, `tests/redis_jwks_cache.rs` — may now trip the breaker mid-run.
Their assertions are about `None` / fall-through / `Unavailable`, all of which an open breaker
produces identically, so they should hold unchanged. This is called out because "unchanged" is a
claim to verify when the suites run, not an assumption.

## 5. Documentation

**RUNBOOK §4 "Authz availability posture"** — the "A blackholed Redis is the residual" paragraph
is rewritten: the ~2.1 s figure becomes **measured** (citing §4.2) and is reframed as the cost of
the failures that open the breaker and of the in-flight cohort, not of every command. New
material: the breaker's states and constants, D7's two-window recovery bound and *why* it is two,
D9's JWKS asymmetry with the ~6 s token-auth outage spelled out, D6's failover-trip consequence,
and D10's two metrics with the three attribution caveats.

**RUNBOOK §4** gains `IamRedisBreakerOpen`, `IamJwksRedisBreakerOpen` and
`IamRedisBreakerFlapping` entries in the house format.

**RUNBOOK §4, existing text at `:1055-1056`** — "a `ConnectionManager` burns a **full cycle per
failed command**" becomes conditional. It is true only when commands are issued faster than a
dial completes; the breaker deliberately introduces a longer gap (§1.4). Left uncorrected it
contradicts D7.

**RUNBOOK §6 "Future"** — the "A Redis circuit breaker" bullet shrinks to what is genuinely still
open: `connection_timeout` staying at 1 s per D2, and SMA-473 D10's boot-tolerance residual with
D11's note that the two interact.

**Real SYN-drop procedure** — a §4 subsection, with numbers from an actual run. Both mechanisms
are described accurately, because the obvious descriptions are wrong:

- `docker pause` does **not** drop SYNs. The cgroup freezer stops the *process*; the listening
  socket stays in the kernel, which completes handshakes into the accept backlog (redis's
  `tcp-backlog` defaults to 511). So it reproduces §4.2's shape — connect succeeds, the read
  hangs — until the backlog fills. That is the right shape and the easiest to run; it is just not
  a SYN drop. `docker unpause` is what makes it a recovery test.
- `iptables -I INPUT -p tcp --dport 6379 -j DROP` will **not** catch host traffic to a
  Docker-published port: that path is DNAT'd in `nat PREROUTING`/`OUTPUT` and traverses
  `FORWARD`/`DOCKER-USER`, not `INPUT`. The rule must go in `DOCKER-USER`, or target the
  container's own netns.

## 6. Rollout, rollback, residual risk

**Rollout** is a plain deploy — no migration, no config change, no new required setting. A
deployment that never has a Redis problem never observes the breaker at all.

**Rollback** is reverting the commit. There is no persisted state and no schema change.

**Residual risks, named:**

1. **Recovery lags Redis by up to ~6 s** (D7), and for the JWKS path that window is 100% 503s
   (D9). Bounded, documented, and the reason `OPEN_DURATION` is 2 s.
2. **A routine failover trips the breaker under load** (D6) — three concurrent IO failures is a
   low bar. Costs risk #1's window on every failover, where today it costs only the in-flight
   requests.
3. **`connection_timeout` stays 1 s** (D2), so the failures that open the breaker still cost
   ~2.1 s each. The RUNBOOK states the remaining figure rather than implying the residual is gone.
4. **The in-flight cohort is unprotected.** Every command admitted before the third failure lands
   pays the full ~2.1 s — under 200 concurrent requests that is 200 × 2.1 s, not 3 × 2.1 s. The
   breaker bounds the *duration* of an outage's cost, not its initial burst.
5. **`ConnectionLike` is a lower-level trait than `AsyncCommands`, and D7 depends on
   `futures::Shared` memoization.** A redis-rs major bump could change either. Both couplings are
   called out in the module doc, and §4.4 pins the observable contract rather than the mechanism.
6. **No automated test pins the two-window recovery bound** (D7) — deliberately, so an
   improvement upstream does not red the build. The §5 manual procedure is what confirms it.

## 7. Out of scope

- **Lowering `connection_timeout`/`response_timeout`, and any config knob for them or for the
  breaker** (D2).
- **SMA-473 D10's boot-tolerance residual** — a retry loop around `redis_conn::connect` in
  `AppState::new` (D11). Stays in RUNBOOK §6, now with a note that it interacts with D11.
- **A fail-closed authz option** — SMA-470 D1 declines it by recorded decision; an open breaker
  produces *more* cache bypasses, which is correct behaviour under that decision.
- **Postgres-backed generation counters** — RUNBOOK §6, needs an ADR and a migration.
- **Making the half-open probe force a real dial** (holding the `Client` and probing with a fresh
  connection + `PING`). It would cut recovery to one window, but reintroduces a real ~2.1 s dial
  per window and adds a second connection path. Shortening the window achieves most of the
  benefit for none of the cost.

## 8. Acceptance criteria

| # | AC | Where it is met |
| -- | -- | -- |
| 1 | The blackholed shape is measured, not calculated, with before/after numbers | §4.2 (hermetic, Docker-free, in CI) + §5's manual `docker pause`/`DOCKER-USER` procedure |
| 2 | The chosen lever sits behind `adapters::redis_conn`, gate still holds | §3.1, and D12 *strengthens* the gate to ban the `ConnectionManager` type |
| 3 | Authz fail-open and JWKS fail-closed postures unchanged | D3/D4 preserve them by construction; §4.3 proves them |
| 4 | RUNBOOK §4 and §6 updated; §6's bullet shrinks | §5 |
| 5 | Breaker state observable, RUNBOOK says how to read it | D10, §3.4, §3.5, §4.5, §5 |
