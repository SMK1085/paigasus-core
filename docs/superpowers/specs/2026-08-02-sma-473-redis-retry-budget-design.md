# SMA-473 — Bound the Redis client retry budget

**Status:** design (revised after adversarial review)
**Date:** 2026-08-02
**Issue:** [SMA-473](https://linear.app/smaschek/issue/SMA-473/iam-bound-the-redis-client-retry-budget-an-outage-costs-20-30s-per)
**Project:** Paigasus IAM
**Follows:** [SMA-470](https://linear.app/smaschek/issue/SMA-470/iam-cover-revocation-during-redis-outage-and-decide-whether-to-offer-a) (found while implementing it)

## 1. Problem

`paigasus-iam` opens every Redis connection with a bare `ConnectionManager::new(client)`,
i.e. a stock `ConnectionManagerConfig::default()`. Against a **down** backend that default
costs ~6.3–12.6 s per failed command, and a single authz decision makes several such reads.
Measured on a real stopped-Redis container (the SMA-470 acceptance test):

| request | measured | cycles |
|---|---|---|
| `POST /v1/authz/is-authorized` | **19–28 s** | 2–3 |
| `DELETE /v1/authz/role-grants/{id}` (revoke) | **28.4 s** | 3–4 |

The authz stack is deliberately fail-open on a Redis outage (SMA-470 D1: Redis is a pure
accelerator, so denying during its outage would convert a latency degradation into a total
authorization outage). That reasoning holds only if the degradation is *small*. At 20–30 s
per decision nearly every caller times out anyway, so operationally the outage is much
closer to an authz outage than to a degradation. **Bounding the retry budget restores D1's
premise; it does not weaken it.**

### 1.1 The mechanism, precisely

Read from `redis-1.3.0/src/aio/connection_manager.rs` and `backon-1.6.0`:

- `ConnectionManager` holds an `ArcSwap`'d **shared connection future**. Every command
  (`send_packed_command`, `connection_manager.rs:678`) loads and awaits it.
- When Redis is down that future is a `new_connection` running the full `backon` retry
  schedule before resolving to `Err`.
- The resulting I/O error triggers `Self::reconnect`, which CAS-swaps in a **fresh** future
  and spawns it **detached** (`connection_manager.rs:649`); the failing command returns
  immediately without awaiting it. The next command therefore awaits a brand-new cycle.

**The retry budget covers establishing a connection, never a command.**
`send_packed_command` does `connection_result?.send_packed_command(cmd).await` and then
`reconnect_if_dropped!`, which triggers a background reconnect and **returns the `Err` to
the caller** (`connection_manager.rs:390-398`). No command is ever retried. This matters
because it is the only thing `number_of_retries` buys (see D2).

**"One cycle per failed command" is an upper bound for a specific code shape**, not an
identity. It holds for a single in-flight request issuing reads back-to-back with no
intervening work — which is exactly what `is_authorized` does. It does **not** hold for
*concurrent* commands: they all await the same `Shared` future and only the CAS winner
spawns the replacement, so N concurrent reads cost one cycle, not N.

*Independently measured:* the existing Docker-free unit test
`api_keys::cache::tests::redis_cache_fails_open_when_the_backend_is_unreachable` issues
`get` + `put` + `evict` against a closed port and takes **28.403 s** — 9.47 s per cycle,
squarely in the predicted 6.3–12.6 s band with an expected ~9.5 s. The model is confirmed by
something other than the SMA-470 measurement it was derived from.

**The cost is the retry schedule, not the per-attempt timeouts.** redis-rs 1.3.0 already
defaults `connection_timeout` to 1 s and `response_timeout` to 500 ms
(`DEFAULT_CONNECTION_TIMEOUT` / `DEFAULT_RESPONSE_TIMEOUT`). What is unbounded *in practice*
is `number_of_retries = 6`, `min_delay = 100 ms`, `exponent_base = 2.0` — a
`100+200+400+800+1600+3200 ms` schedule, ~6.3 s per cycle as a floor.

**Two `backon` details determine which knob actually moves the number:**

1. **Jitter *adds*.** `ExponentialBackoff::next` computes
   `tmp_cur = tmp_cur + tmp_cur.mul_f32(rng.f32())` (`exponential.rs:234`), so an actual
   sleep is `delay × [1.0, 2.0]` — a cycle is 6.3–12.6 s, expected ~9.5 s. (redis-rs
   documents `min(max_delay, rand(0 .. min_delay * base^try))`, which describes neither the
   addition nor point 2.)
2. **`max_delay` caps the *pre-jitter base* delay, and never applies to the first step.**
   When `current_delay` is `None` the code returns `self.min_delay` directly
   (`exponential.rs:210-213`), bypassing the clamp. So at `number_of_retries = 1` there is
   exactly **one** sleep, it is `min_delay × [1,2]`, and `max_delay` is **inert**.

Note the schedule is not literally unbounded even today: redis's `ConnectionManagerConfig`
leaves `max_delay` at `None`, so `backon`'s own `ExponentialBuilder::new()` default of
`Some(60 s)` applies (`exponential.rs:61`) — inert here, because a 6-step schedule peaks at
3.2 s. Consequence: **`number_of_retries` is the only lever that moves the measured number.**

### 1.2 What reading the code found beyond the issue

The issue names one call site. There are **two** production sites, and the unnamed one is
worse.

`RedisJwksCache::connect` (`adapters/oidc/redis_cache.rs:42`, wired at
`adapters/http/mod.rs:568`) uses the same bare `ConnectionManager::new(client)`. Unlike the
authz caches it does **not** fail open: `RedisJwksCache::get` maps any Redis error to
`AuthnError::Unavailable` (spec §4.3/D15 — key material, so fail-closed is correct), and
`JwksProvider::key_for` reads the cache on **every** token validation
(`adapters/oidc/jwks.rs:207,249`).

So under `authn.jwks_cache.backend = "redis"`, a Redis outage makes every authenticated
request burn a full ~6–12 s reconnect cycle **and then 503**. That is a hard authentication
outage with ~10 s of latency bolted on. The RUNBOOK's availability section documents only
the authz cost and never mentions it.

A third path shares the same handle and is also undocumented: `RedisApiKeyCache`
(`adapters/http/mod.rs:468-490`) is read on **every** API-key-authenticated request. A miss
costs a `get` **and** a `put` (2 cycles); `RevokeApiKey` / `ArchiveServiceAccount` add an
`evict`. The RUNBOOK itself calls the gateway's `IntrospectApiKey`/`IsAuthorized` pair "the
hottest gRPC path in normal operation" (`RUNBOOK-observability.md:520-522`).

### 1.3 Evidence

- `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:628` — `connect_redis`, bare
  `ConnectionManager::new`.
- `rs/crates/services/paigasus-iam/src/adapters/oidc/redis_cache.rs:42` —
  `RedisJwksCache::connect`, same.
- `rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs:207,249` — per-token cache read;
  `redis_cache.rs:71,78` — fail-closed `log_unavailable` mapping.
- `rs/crates/services/paigasus-iam/src/application/authorize.rs:76-79` — `decide_gated` runs
  a full second `is_authorized` for a non-self query.
- `rs/crates/services/paigasus-iam/src/adapters/authz/policy_snapshot.rs:209-214` —
  `reload_if_stale` returns **before** reading `policy_gen` once the stamp is provisional.
- `rs/crates/services/paigasus-iam/tests/authz_acceptance.rs:705` — the measurement and the
  90 s liveness budget it forced.
- `ops/observability/prometheus/rules/iam.rules.yml` — eight alerts, **none** on
  `iam_authz_decisions_total{cache="bypass"}`.
- `docs/ops/RUNBOOK-observability.md` §"Authz availability posture" — documents the current
  cost and names SMA-473 as the unshipped mitigation.

## 2. Decisions

**D1 — Cap the retry count only; leave every timeout at the redis-rs default.**
`number_of_retries` 6 → **1**. `min_delay` (100 ms), `exponent_base`, `connection_timeout`
(1 s) and `response_timeout` (500 ms) are all left alone. The two timeouts are what the
issue explicitly warns off, and they are already bounded.

*Consequence, stated rather than hidden:* this bounds the **measured** failure shape (Redis
stopped / port closed, where `ECONNREFUSED` is instant) to ~100–200 ms per failed command. A
**blackholed** Redis — SYN dropped, packet loss — pays `connection_timeout` per attempt
instead, so a failed command costs ~2.1 s. That shape is **out of scope** by decision; §6
records it as a residual.

**D2 — One retry, not zero and not two.** The retry budget applies only to *establishing* a
connection (§1.1), so the case it buys is narrow and specific: the first connect attempt
landing in a **failover gap** — old primary gone, new one not yet accepting — which a
100–200 ms delay is well matched to. One retry covers that; zero does not. Two retries would
cost 300–600 ms per command and the extra attempt only helps in a still-narrower window.

*(An earlier draft justified this as "absorbing a connection killed mid-flight inside a
single command". That mechanism does not exist — `send_packed_command` never retries a
command. The decision survives; the reasoning was wrong and is corrected here so a future
reader does not raise the count chasing a phantom.)*

**D3 — Set `max_delay` anyway, as a schedule guard.** At `number_of_retries = 1` it is inert
(§1.1). It is set to 500 ms so that raising the retry count later caps each step at 500 ms
rather than falling back on `backon`'s 60 s default. The constant's doc comment must say it
is inert today, or a future reader will mis-read it as load-bearing.

**D4 — Hardcoded constants in a shared helper; no new config surface.** No operator has
asked to tune these, there is one sane value, and a mistuned value silently reintroduces the
bug. Exposing them in `iam.toml` would add defaults, `IamConfig::validate` bounds, docs and
tests for a knob with a single correct setting. Deliberate non-goal, recorded here so it is a
decision rather than an omission.

**D5 — Route *every* `ConnectionManager` construction through the helper, not just the two
production sites.** *(This widens an earlier decision to cover production sites only.)* Three
reasons, in order of weight:

1. It makes D-A's guard enforceable: with zero exceptions, a CI grep for
   `ConnectionManager::new` outside the helper module can be strict-equality (§4.3). With
   exceptions it cannot, and AC1 becomes untestable prose.
2. It is a **measured 28.4 s → <1 s** cut to the Docker-free unit-test suite (§1.1), from a
   one-line change to `api_keys/cache.rs:384`.
3. The four "test-only" constructors (`decision_cache.rs:128`, `api_keys/cache.rs:210`,
   `entity_cache.rs:67`, `generation.rs:60`) connect *after* testcontainers reports the
   container ready, so they never needed a 6-retry budget; nobody chose it for them.

**D6 — Cover the JWKS path.** Same defect, same one-line fix through the same helper. This
changes **only how fast** the JWKS path fails; the D15 fail-closed posture is untouched and
stays correct.

**D7 — No circuit breaker.** With the retry budget capped, the measured shape is already
under target. A breaker (open → half-open probe → closed, plus state, metrics and its own
tests) would only pay for itself in the blackholed shape D1 puts out of scope. Follow-up
(§7), not built here.

**D8 — Share `connect`, not just the config.** A shared `connect()` makes the tuned schedule
unbypassable: no call site can construct a `ConnectionManager` without it. It returns a bare
`redis::RedisResult` so each caller keeps its own — and deliberately different — error
mapping verbatim.

**D9 — Ship an alert for the outage this fix makes quiet.** *(New; the single most important
change from the adversarial review.)* Today an operator learns Redis is down because latency
explodes into `IamHighErrorRate` / `IamGrpcHighErrorRate` and client timeouts — the RUNBOOK
says so explicitly (`:665-667`) and instructs "Page on 'Redis is down'". **Those alerts fire
*because* of the latency this fix removes.** There is no rule on
`iam_authz_decisions_total{cache="bypass"}` (§1.3), and `IamPolicySnapshotReloadsStalled`
stays quiet during a Redis outage by SMA-470's design.

Shipping the latency fix alone would therefore make a total Redis outage produce correct,
fast decisions, a flat error rate, and **zero alerts** — while the service silently loses its
decision cache, its cross-replica API-key revocation (`RevokeApiKey`/`ArchiveServiceAccount`
evictions stop being global), and, under `jwks_cache.backend = "redis"`, 503s every
authenticated request. Trading a loud degradation for a silent one is a regression, not a
neutral change. So this issue also adds an `IamAuthzRedisCacheBypassed` rule with a promtool
fixture and a RUNBOOK entry.

**D10 — Accept the reduced boot-time tolerance; do not add a boot retry loop.** `connect` is
**eager** (`ConnectionManager::new_with_config`), preserving today's fail-fast-at-boot
contract: `AppState::new` opens Redis at `adapters/http/mod.rs:301` and `main.rs:60` has no
retry loop, so a `?` exits the process.

A `ConnectionManagerConfig` is fixed for the manager's lifetime, so a longer *boot* budget
cannot coexist with a short *steady-state* one on the same manager — it would take an
explicit retry loop around `connect` in `AppState::new`. We are not adding one, because the
practical delta is small: the process already crashes on a Redis-down boot today (after
~6–12 s rather than ~200 ms), so every deployment already relies on container restart
backoff, whose cadence (10 s → 20 s → 40 s …) dominates the dial either way. The real change
is that a Redis start-up window **shorter than ~10 s** used to be absorbed silently and will
now cost one crash-restart.

This is a genuine behavior change and §6 records it. An earlier draft's claim of "no
behavior changes anywhere" was false and is deleted.

## 3. The fix

### 3.1 New module — `adapters::redis_conn`

`rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs`, registered in
`adapters/mod.rs`. One purpose: own how this service dials Redis.

Named `redis_conn`, **not** `redis`: six files in this crate use bare `redis::` paths, so a
sibling module called `redis` would make any `use crate::adapters::redis;` shadow the extern
crate and break `redis::Client::open` in that module.

```rust
/// Retries AFTER the first attempt. redis-rs defaults to 6, i.e. a
/// `100+200+400+800+1600+3200 ms` schedule that `backon`'s jitter stretches to
/// ~6.3–12.6 s per cycle — and a `ConnectionManager` burns a full cycle per failed
/// command, so a single authz decision cost a measured 19–28 s (SMA-473).
///
/// The budget buys exactly one thing: tolerance while ESTABLISHING a connection
/// (`send_packed_command` never retries a command — it surfaces the error and
/// reconnects in the background). One retry covers a first attempt landing in a
/// failover gap, which `min_delay` (100–200 ms jittered) is well matched to.
const CONNECT_RETRIES: usize = 1;

/// Guard only — INERT at `CONNECT_RETRIES = 1`. `backon` applies `max_delay` to the
/// pre-jitter base delay and never to the first step (the first delay is always
/// `min_delay`), so with one retry this is never reached. It exists so that raising
/// `CONNECT_RETRIES` later caps each step here rather than at `backon`'s 60 s default.
const RETRY_MAX_DELAY: Duration = Duration::from_millis(500);

/// The tuned config every Redis connection in this service is opened with. Exposed
/// (not just used by `connect`) so the config test can assert on it directly.
pub(crate) fn connection_manager_config() -> ConnectionManagerConfig;

/// Opens `redis_url` and wraps it in a `ConnectionManager` built with
/// [`connection_manager_config`]. EAGER (`new_with_config`) — it awaits the initial
/// connection, preserving `AppState::new`'s fail-fast-at-boot contract (D10). Returns a
/// bare `RedisResult` so each caller applies its own error mapping (they differ,
/// deliberately — see the call sites).
pub(crate) async fn connect(redis_url: &str) -> redis::RedisResult<ConnectionManager>;
```

`connection_manager_config` sets **only** `set_number_of_retries(CONNECT_RETRIES)` and
`set_max_delay(RETRY_MAX_DELAY)` on a `ConnectionManagerConfig::new()`. Every other field
stays at the redis-rs default, and the module doc says which and why — `min_delay` (a sane
failover retry delay, and at one retry it *is* the entire budget), `exponent_base` (unreached
at one retry), `connection_timeout` / `response_timeout` (already bounded at 1 s / 500 ms;
tightening them was considered and declined, D1).

### 3.2 Call sites

Every site, per D5. The two production sites keep their existing error mapping exactly:

| site | mapping (unchanged) |
|---|---|
| `http::connect_redis` (`mod.rs:628`) | `AuthnError::Backend(Box::new(e))` |
| `RedisJwksCache::connect` (`redis_cache.rs:42`) | `log_unavailable(None, err.kind())` → `AuthnError::Unavailable` |

`Client::open` moves inside `connect`. `RedisJwksCache::connect` currently maps a
`Client::open` failure through `log_unavailable` too; that stays true because `connect`
returns both failures as a `RedisError`.

Also converted (D5): the four testcontainer constructors `RedisDecisionCache::connect`
(`decision_cache.rs:128`), `SliceCache::connect` (`entity_cache.rs:67`),
`RedisApiKeyCache::connect` (`cache.rs:210`), `Generations::connect_redis`
(`generation.rs:60`); and the two `#[cfg(test)]` lazy constructions
(`entity_cache.rs:232`, `api_keys/cache.rs:384`) — the latter is the measured 28.4 s → <1 s
win, and both should route through `connection_manager_config()`.

### 3.3 Resulting bound

One `min_delay × [1,2]` sleep per failed command. Cycle counts depend on the path and on
whether the policy-snapshot stamp is still **trusted** (first outage moments) or has gone
**provisional** (steady state, where `reload_if_stale` returns without reading `policy_gen`):

| path | cycles | today | after |
|---|---|---|---|
| `is-authorized`, **self** query, steady state | 2 | ~19 s | **0.2–0.4 s** |
| `is-authorized`, **self** query, stamp still trusted | 3 | ~28 s | **0.3–0.6 s** |
| `is-authorized`, **gated** (cross-principal) query | 4–6 | ~38–57 s | **0.4–1.2 s** |
| `DELETE /role-grants/{id}` (+ post-commit bump) | 3–4 | **28.4 s** (measured) | **0.3–0.8 s** |
| API-key authenticated request (miss: `get`+`put`) | 2 | ~19 s | **0.2–0.4 s** |
| any authenticated request (JWKS, then 503) | 1 | ~6–12 s | **0.1–0.2 s** |

The gated path is worth calling out because it is the one the SMA-470 measurement never saw:
`decide_gated` runs a full second `is_authorized` before the real one whenever
`req.principal != actor` (`authorize.rs:76-79`), and the acceptance test used a self query.

Behavior is unchanged everywhere **except** boot tolerance (D10): the authz caches still
degrade a Redis error to a miss and fail open, JWKS still fails closed with `Unavailable`,
and `AppState::new` still fails fast when Redis is unreachable at boot — just ~50× sooner.

### 3.4 Alert — `IamAuthzRedisCacheBypassed` (D9)

New rule in `ops/observability/prometheus/rules/iam.rules.yml`, on the existing
`iam_authz_decisions_total{cache="bypass"}` series. `bypass` is emitted only when the
`entity_gen` read errors, which on the `memory` backend cannot happen — so a sustained
nonzero rate means the Redis backend is unhealthy, with no healthy-state false positives.
Severity `critical`, matching the RUNBOOK's existing "Page on 'Redis is down'" instruction.

The promtool fixture must carry a **control series** — a healthy window where the rule does
*not* fire — or an all-firing fixture cannot distinguish `rate(...) > 0` from `rate(...) >=
0` (the SMA-466 lesson).

Under `authz.cache.backend = "memory"` the series does not exist and the rule stays silent;
the RUNBOOK entry must say so, since that mirrors the already-documented
`audit.retention.enabled = false` silent-alert trap.

## 4. Tests

### 4.1 Unit — config assertions (primary guard, deterministic)

`ConnectionManagerConfig` exposes public getters — `number_of_retries()`, `max_delay()`,
`min_delay()`, `exponent_base()`, `connection_timeout()`, `response_timeout()`
(`connection_manager.rs:119-152`). Assert directly on `connection_manager_config()`:

- `number_of_retries() == 1` and `max_delay() == Some(500ms)` — the change itself.
- `min_delay()`, `exponent_base()`, `connection_timeout()`, `response_timeout()` are still at
  the redis-rs defaults — this pins **D1's "left alone" half**, which nothing else asserts,
  so a future edit that quietly tightens a timeout has to argue with a test.

This is the primary guard because it is exact. A wall-clock deadline only reliably catches a
regression all the way back to 6 retries; a regression to 3 yields 700–1400 ms and would
pass or flake against a 1 s bound depending on runner load.

### 4.2 Unit — latency bound (secondary, proves the config is actually applied)

`#[tokio::test]` (required: `new_lazy_with_config` calls `runtime.spawn`, which panics
outside a Tokio runtime). Builds a lazily-connecting manager from the **production**
`connection_manager_config()` — `new_with_config` would eagerly connect and fail — pointed at
`redis://127.0.0.1:1`. Precedent: `entity_cache.rs:232`, `api_keys/cache.rs:384`.

- **Bound:** a `GET` resolves to `Err` within **2 s**. Loose on purpose: §4.1 owns exactness,
  this only proves the config reaches the manager rather than being built and dropped.
- **Control:** `err.is_io_error()` (public, `errors/redis_error.rs:321`). Without it the
  deadline could pass for the wrong reason — a malformed URL erroring instantly looks
  identical to a fast, correct failure. Note this control cannot separate a fast refuse from
  a slow timeout (both surface as an IO error); the deadline does that.

### 4.3 CI — no bypassing call site

A repo-level grep gate (precedent: `wasm-getrandom-free`) asserting `ConnectionManager::new`
/ `new_with_config` / `new_lazy_with_config` appear **only** in `adapters/redis_conn.rs`.
D5's zero-exception scope is what makes this strict-equality rather than an allowlist. This
is the only thing that catches a *new* call site added later; §4.1 only catches a changed
constant.

### 4.4 Existing suites

No assertion changes. `tests/authz_acceptance.rs` and `tests/api_key_cache_redis.rs` get
faster, and the unit suite loses ~28 s. The SMA-470 acceptance test's 90 s convergence budget
stays (it is a liveness deadline, not a bound assertion) but its explanatory comments become
wrong and are corrected in §5.

## 5. Documentation

The prose is dense and interlinked; several paragraphs change arithmetic once this lands.

**`docs/ops/RUNBOOK-observability.md` §"Authz availability posture":**

1. Replace "**Fail-open is NOT free: budget ~20–30 s per authz decision**" and the paragraphs
   under it with the shipped numbers (§3.3), keeping the retry-schedule-vs-timeouts
   explanation — it is still the reason the fix looks the way it does.
2. Delete "**None of this is shipped yet** — it is pre-existing and tracked as **SMA-473**"
   and the "no config-only workaround" sentence.
3. **Rewrite the detection guidance** (`:665-667`) — "Expect the symptom to arrive as
   `IamGrpcHighErrorRate` / `IamHighErrorRate` / client-side timeouts" is exactly what stops
   being true. Point at the new `IamAuthzRedisCacheBypassed` alert (D9) instead.
4. Add a note that once `max_delay` is *set*, it does not apply to the first delay step.
   Do **not** "correct" the existing `max_delay`-unset sentence: it already reads "no per-step
   cap is applied beyond `backon`'s own inert 60 s default", which is accurate.
5. Revise the revocation-freshness paragraph: the "**budget nearer ~55 s than ~31 s**" figure
   and the `ttl + poll + 2 × retry cycle` worst case both collapse — the retry-cycle term is
   now sub-second, so the honest worst case returns to `ttl + poll` (~31 s at defaults) plus
   the reload's own duration. **Preserve** the surrounding notes that do not depend on the
   retry cycle: that `refresh_interval_secs == policy_cache_ttl_secs` is permitted, and that
   raising the poll interval to its maximum doubles the bound to `2 × policy_cache_ttl_secs`.
6. **Add** the missing JWKS note (§1.2): under `authn.jwks_cache.backend = "redis"` a Redis
   outage is a **fail-closed authentication** outage — every authenticated request 503s —
   not merely an authz slowdown. Add the API-key path (`RedisApiKeyCache`, 2 cycles per miss,
   the hottest gRPC path) alongside it.
7. Record the blackholed-Redis residual (D1) and the boot-tolerance change (D10) so the new
   numbers are not read as universal.
8. New alert entry for `IamAuthzRedisCacheBypassed` in the alert catalog: meaning, the
   `memory`-backend silence trap, confirm steps, remediation.

**`adapters/authz/cedar_authorizer.rs` module doc** (step 3 of the `is_authorized` flow):
replace the "stock `ConnectionManagerConfig::default()` … measured 19–28 s per decision …
tracked follow-up" passage with the shipped configuration and its bound.

**`tests/authz_acceptance.rs`** (~lines 705–720): the two comments justifying the 90 s budget
cite "~20-30s per request" and "amendment A / SMA-473" as unshipped. Update the figures and
keep the budget, with a one-line note on why it stays wide.

**No CI gate validates any of this.** `observability-drift` reads only `ops/observability/**`
and `paigasus-observability/**`; nothing checks `docs/ops/RUNBOOK-observability.md`. Review,
not CI, is the gate for AC5/AC6 — stated so it is not assumed otherwise.

## 6. Rollout, rollback, residual risk

**Rollout.** Library-config change plus one alert rule. No schema/API/config-file impact, no
migration. A rolling deploy is safe; mixed old/new replicas simply have different outage
latencies.

**Rollback.** Revert the commit. No persisted state depends on it.

**Residual risks.**

- *Blackholed Redis is still slow* (D1). ~2.1 s per failed command because
  `connection_timeout` then dominates. Documented, not fixed. Follow-up levers in order of
  cost: lower `connection_timeout` (~250 ms is generous for a LAN Redis), then a circuit
  breaker (D7). **Unverified:** whether `connection_timeout` also wraps DNS resolution — a
  production `redis://redis:6379` hostname resolves before the TCP connect, and this design
  has not established which side of the timeout that falls on.
- *Boot tolerance drops ~50×* (D10). A Redis start-up window under ~10 s that used to be
  absorbed silently now costs one crash-restart. Container restart backoff dominates
  recovery either way.
- *Less absorption of a slow failover.* A failover needing more than one connect attempt now
  surfaces as a cache bypass. This is the designed fail-open behavior — correctness is
  unaffected — but it will make `iam_authz_decisions_total{cache="bypass"}` briefly noisier
  during a failover, which is also what D9's alert watches. The rule's `for:` duration must be
  long enough not to page on a routine failover.
- *No background reconnect while idle.* With RESP2 (the default here — no `push_sender` is
  configured) there is no background reconnect loop; a reconnect is spawned only when a
  command observes an I/O error. Under idle traffic the manager does not reconnect at all.
- *Constant drift.* §4.1 pins the values and §4.3 pins the call sites; between them the
  failure messages must name SMA-473, or a future edit silently restores 20 s decisions.

## 7. Out of scope / follow-ups

- **Bounding the blackholed/hung-Redis shape** — `connection_timeout` tightening and/or a
  circuit breaker (D1, D7).
- **A boot-time retry loop** around `connect` in `AppState::new` (D10), if the crash-restart
  on a short Redis start-up window turns out to matter.
- **Operator-tunable retry config** (D4) — deliberate non-goal until someone needs it.
- **Revisiting the JWKS fail-closed posture** (D15). This design makes it fail *fast*; whether
  it should fail *open* is a separate, deliberately-taken decision and is not reopened here.
- **A first-class "Redis unhealthy" metric.** D9's alert infers it from the authz bypass
  counter; the JWKS and API-key paths have no equivalent signal and are covered only by
  correlation in the RUNBOOK.

## 8. Acceptance criteria

1. Every `ConnectionManager` construction in `paigasus-iam` — production and test — goes
   through `adapters::redis_conn`, which caps `number_of_retries` at 1 and sets a `max_delay`
   guard.
2. Both production call sites (`http::connect_redis`, `RedisJwksCache::connect`) are
   converted, each preserving its existing error mapping exactly.
3. A unit test asserts the exact config values **and** that the deliberately-untouched fields
   are still at redis-rs defaults; a second `#[tokio::test]` proves a command against an
   unreachable Redis errors within 2 s with an `is_io_error()` control.
4. A CI grep gate fails if `ConnectionManager::new*` appears outside `adapters/redis_conn.rs`.
5. `IamAuthzRedisCacheBypassed` ships in `iam.rules.yml` with a promtool fixture that
   includes a non-firing control series, plus a RUNBOOK alert entry noting the
   `memory`-backend silence.
6. `docs/ops/RUNBOOK-observability.md` §"Authz availability posture" reflects the shipped
   numbers, no longer names SMA-473 as unshipped, replaces the "expect it as
   `IamHighErrorRate`" detection guidance, revises the ~55 s revocation figure while
   preserving the poll-interval notes, and documents the JWKS fail-closed authn outage, the
   API-key path, the blackhole residual and the boot-tolerance change.
7. `cedar_authorizer.rs`'s module doc and the SMA-470 acceptance-test comments no longer cite
   19–28 s or an unshipped follow-up.
8. The unit-test suite no longer spends ~28 s in
   `redis_cache_fails_open_when_the_backend_is_unreachable`.
9. The full CI gate graph passes:
   `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
   :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift :release-parity
   :release-parity-py :release-parity-ts --base origin/main --include-relations`.
