# SMA-473 — Bound the Redis client retry budget

**Status:** design
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
| `POST /v1/authz/is-authorized` | **19–28 s** | 3–4 |
| `DELETE /v1/authz/role-grants/{id}` (revoke) | **28.4 s** | 4 |

The authz stack is deliberately fail-open on a Redis outage (SMA-470 D1: Redis is a pure
accelerator, so denying during its outage would convert a latency degradation into a total
authorization outage). That reasoning holds only if the degradation is *small*. At 20–30 s
per decision nearly every caller times out anyway, so operationally the outage is much
closer to an authz outage than to a degradation. **Bounding the retry budget restores D1's
premise; it does not weaken it.**

### 1.1 The mechanism, precisely

Read from `redis-1.3.0/src/aio/connection_manager.rs` and `backon-1.6.0`:

- `ConnectionManager` holds an `ArcSwap`'d **shared connection future**. Every command
  (`send_packed_command`) loads and awaits it.
- When Redis is down that future is a `new_connection` running the full `backon` retry
  schedule before resolving to `Err`.
- The resulting I/O error triggers `Self::reconnect`, which CAS-swaps in a **fresh** future
  and spawns it detached. The next command therefore awaits a brand-new cycle.
- Net effect: **one full retry cycle per failed command.** Three to four reads per request
  reproduces the measured 19–28 s exactly.

**The cost is the retry schedule, not the per-attempt timeouts.** redis-rs 1.3.0 already
defaults `connection_timeout` to 1 s and `response_timeout` to 500 ms
(`DEFAULT_CONNECTION_TIMEOUT` / `DEFAULT_RESPONSE_TIMEOUT`). What is unbounded is
`number_of_retries = 6`, `min_delay = 100 ms`, `exponent_base = 2.0`, `max_delay` unset —
a `100+200+400+800+1600+3200 ms` schedule, ~6.3 s per cycle as a floor.

**Two `backon` details that determine which knob actually moves the number**, and which
neither redis-rs's own doc comment nor the current RUNBOOK prose states correctly:

1. **Jitter *adds*.** `ExponentialBackoff::next` computes
   `tmp_cur = tmp_cur + tmp_cur.mul_f32(rng.f32())`, so an actual sleep is
   `delay × [1.0, 2.0]` — a cycle is 6.3–12.6 s, expected ~9.5 s. (redis-rs documents
   `min(max_delay, rand(0 .. min_delay * base^try))`, which describes neither the addition
   nor point 2.)
2. **`max_delay` caps the *pre-jitter base* delay, and never applies to the first step.**
   When `current_delay` is `None` the code returns `self.min_delay` directly, bypassing the
   `max_delay` clamp entirely. So at `number_of_retries = 1` there is exactly **one** sleep,
   it is `min_delay × [1,2]`, and `max_delay` is **inert**.

Consequence: `number_of_retries` is the only lever that moves the measured number.
`max_delay` is a guard against a future raise of the count, not a fix in itself.

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

The other four `connect` constructors — `RedisDecisionCache::connect`, `SliceCache::connect`,
`RedisApiKeyCache::connect`, `Generations::connect_redis` — have **no production callers**;
they exist for the `tests/*_redis.rs` testcontainer suites.

### 1.3 Evidence

- `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:628` — `connect_redis`, bare
  `ConnectionManager::new`.
- `rs/crates/services/paigasus-iam/src/adapters/oidc/redis_cache.rs:42` —
  `RedisJwksCache::connect`, same.
- `rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs:207,249` — per-token cache read.
- `rs/crates/services/paigasus-iam/src/adapters/oidc/redis_cache.rs:71,78` — fail-closed
  `log_unavailable` mapping.
- `rs/crates/services/paigasus-iam/tests/authz_acceptance.rs:705` — the measurement and the
  90 s liveness budget it forced.
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
instead, so a failed command costs ~2.1 s and a decision ~6–8 s. That shape is **out of
scope** by decision; it is a real residual and §6 records it.

**D2 — One retry, not zero and not two.** One retry still absorbs a connection killed
mid-flight during a Redis failover inside a single command, which is the case the retry
budget exists for. Because the first delay is always `min_delay` (§1.1 point 2), one retry
costs exactly one 100–200 ms sleep, which meets the ~1 s-per-decision target with room to
spare. Two retries would cost 300–600 ms per command (1.2–2.4 s per decision) and the second
attempt only helps in a narrow window; zero retries removes the failover absorption entirely
for a saving that does not change the outcome.

**D3 — Set `max_delay` anyway, as a schedule guard.** At `number_of_retries = 1` it is inert
(§1.1). It is set to 500 ms so that raising the retry count later cannot silently restore an
unbounded schedule. The constant's doc comment must say it is inert today, or a future
reader will mis-read it as load-bearing.

**D4 — Hardcoded constants in a shared helper; no new config surface.** No operator has
asked to tune these, there is one sane value, and a mistuned value silently reintroduces the
bug. Exposing them in `iam.toml` would add defaults, `IamConfig::validate` bounds, docs and
tests for a knob with a single correct setting. Deliberate non-goal, recorded here so it is a
decision rather than an omission.

**D5 — Cover both production call sites, including JWKS.** Same defect, same one-line fix
through the same helper. This changes **only how fast** the JWKS path fails; the D15
fail-closed posture is untouched and stays correct.

**D6 — No circuit breaker.** With the retry budget capped, the measured shape is already
under the target. A breaker (open → half-open probe → closed, plus state, metrics and its own
tests) would only pay for itself in the blackholed shape that D1 puts out of scope. Recorded
as a follow-up in §6, not built here.

**D7 — Share `connect`, not just the config.** A shared `connect()` makes the tuned schedule
unbypassable: no call site can construct a `ConnectionManager` without it. It returns a bare
`redis::RedisResult` so each caller keeps its own — and deliberately different — error
mapping verbatim.

## 3. The fix

### 3.1 New module — `adapters::redis`

`rs/crates/services/paigasus-iam/src/adapters/redis.rs`, registered in `adapters/mod.rs`.
One purpose: own how this service dials Redis.

```rust
/// Retries AFTER the first attempt. redis-rs defaults to 6, i.e. a
/// `100+200+400+800+1600+3200 ms` schedule that `backon`'s jitter stretches to
/// ~6.3–12.6 s per cycle — and a `ConnectionManager` burns a full cycle per failed
/// command, so a single authz decision cost a measured 19–28 s (SMA-473). One retry
/// still absorbs a connection dropped mid-failover while bounding a dead backend to a
/// single `min_delay` sleep.
const CONNECT_RETRIES: usize = 1;

/// Guard only — INERT at `CONNECT_RETRIES = 1`. `backon` applies `max_delay` to the
/// pre-jitter base delay and never to the first step (the first delay is always
/// `min_delay`), so with one retry this is never reached. It exists so that raising
/// `CONNECT_RETRIES` later cannot silently restore an unbounded schedule.
const RETRY_MAX_DELAY: Duration = Duration::from_millis(500);

/// The tuned config every Redis connection in this service is opened with. Exposed
/// (not just used by `connect`) so the latency-bound test can build a lazily-connecting
/// manager from the EXACT production config rather than a hand-rolled copy.
pub(crate) fn connection_manager_config() -> ConnectionManagerConfig;

/// Opens `redis_url` and wraps it in a `ConnectionManager` built with
/// [`connection_manager_config`]. Returns a bare `RedisResult` so each caller applies
/// its own error mapping (they differ, deliberately — see the two call sites).
pub(crate) async fn connect(redis_url: &str) -> redis::RedisResult<ConnectionManager>;
```

`connection_manager_config` sets **only** `set_number_of_retries(CONNECT_RETRIES)` and
`set_max_delay(RETRY_MAX_DELAY)` on a `ConnectionManagerConfig::new()`. Every other field
stays at the redis-rs default, and the module doc says which and why — `min_delay` (a sane
failover retry delay, and at one retry it *is* the entire budget), `exponent_base` (unreached
at one retry), `connection_timeout` / `response_timeout` (already bounded at 1 s / 500 ms;
tightening them was considered and declined, D1).

### 3.2 Call sites

Both keep their existing error mapping exactly:

| site | before | after |
|---|---|---|
| `http::connect_redis` (`mod.rs:628`) | `ConnectionManager::new(client)` → `AuthnError::Backend(Box::new(e))` | `adapters::redis::connect(url)` → same mapping |
| `RedisJwksCache::connect` (`redis_cache.rs:42`) | `ConnectionManager::new(client)` → `log_unavailable(None, err.kind())` → `AuthnError::Unavailable` | `adapters::redis::connect(url)` → same mapping |

`Client::open` moves inside `connect`. `RedisJwksCache::connect` currently maps a
`Client::open` failure through `log_unavailable(None, err.kind())` as well; that stays true
because `connect` returns both failures as a `RedisError`.

The four test-only constructors are left alone (D5 scope; no production caller).

### 3.3 Resulting bound

One `min_delay × [1,2]` sleep per failed command. Counting the reads that actually pay a
cycle during a full outage — `policy_gen` via `reload_if_stale`, `entity_gen` for the
decision-cache key, and `SliceCache`'s own `entity_gen`; the decision-cache `get`/`put` are
skipped because the key was never built:

| path | cycles | today | after |
|---|---|---|---|
| `POST /v1/authz/is-authorized` | 3 | **19–28 s** | **0.3–0.6 s** |
| `DELETE /v1/authz/role-grants/{id}` (+ post-commit bump) | 4 | **28.4 s** | **0.4–0.8 s** |
| any authenticated request (JWKS, then 503) | 1 | ~6–12 s | **0.1–0.2 s** |

No behavior changes anywhere: the authz caches still degrade a Redis error to a miss and
fail open, JWKS still fails closed with `Unavailable`, and `AppState::new` still fails fast
when Redis is unreachable **at boot**. Only the time-to-error moves.

## 4. Tests

### 4.1 Unit — `adapters::redis` (Docker-free, the primary guard)

One test, built from the **production** `connection_manager_config()` via
`ConnectionManager::new_lazy_with_config` (`new_with_config` would eagerly connect and fail),
pointed at `redis://127.0.0.1:1` — a closed port that refuses instantly. Precedent:
`entity_cache.rs:232` and `api_keys/cache.rs:384` already use exactly this pattern.

- **Bound:** a `GET` resolves to `Err` within a hard **1 s** deadline. That is 5–10× headroom
  over the 100–200 ms expectation and 6.3× under today's floor — a band wide enough that it
  fails only on a real regression, never on a slow runner.
- **Control:** the returned error is an IO/connection kind. Without it the deadline could
  pass for the wrong reason — a malformed URL or an invalid config erroring instantly would
  look identical to a fast, correct failure.

The test must go through `connection_manager_config()` itself. A hand-rolled config in the
test would assert nothing about what `connect` actually uses.

### 4.2 Existing suites

No assertion changes required. `tests/authz_acceptance.rs` and `tests/api_key_cache_redis.rs`
get faster; the SMA-470 acceptance test's 90 s convergence budget stays as-is (it is a
liveness deadline, not a bound assertion) but its explanatory comments become wrong and are
corrected in §5.

## 5. Documentation

The prose is dense and interlinked; several paragraphs change arithmetic once this lands.

**`docs/ops/RUNBOOK-observability.md` §"Authz availability posture":**

1. Replace "**Fail-open is NOT free: budget ~20–30 s per authz decision**" and the paragraphs
   under it with the shipped numbers (§3.3), keeping the retry-schedule-vs-timeouts
   explanation — it is still the reason the fix looks the way it does.
2. Delete "**None of this is shipped yet** — it is pre-existing and tracked as **SMA-473**"
   and the "no config-only workaround" sentence.
3. Correct the two `backon` facts (§1.1): jitter *adds*, and `max_delay` never applies to the
   first step. The current text gets the jitter right but implies `max_delay` is a general
   per-step cap.
4. Revise the revocation-freshness paragraph: the "**budget nearer ~55 s than ~31 s**" figure
   and the `ttl + poll + 2 × retry cycle` worst case both collapse — the retry-cycle term is
   now sub-second, so the honest worst case returns to `ttl + poll` (~31 s at defaults) plus
   the reload's own duration. The sentence "disappears entirely once SMA-473 bounds the retry
   budget" must be rewritten as shipped fact, not a forward reference.
5. **Add** the missing JWKS note (§1.2): under `authn.jwks_cache.backend = "redis"` a Redis
   outage is a **fail-closed authentication** outage — every authenticated request 503s —
   not merely an authz slowdown. This is new information, independent of the fix, and its
   absence is arguably a bigger documentation gap than the stale latency numbers.
6. Record the blackholed-Redis residual (D1) so the new numbers are not read as universal.

**`adapters/authz/cedar_authorizer.rs` module doc** (step 3 of the `is_authorized` flow):
replace the "stock `ConnectionManagerConfig::default()` … measured 19–28 s per decision …
tracked follow-up" passage with the shipped configuration and its bound.

**`tests/authz_acceptance.rs`** (~lines 705–720): the two comments justifying the 90 s budget
cite "~20-30s per request" and "amendment A / SMA-473" as unshipped. Update the figures and
keep the budget, with a one-line note on why it stays wide.

## 6. Rollout, rollback, residual risk

**Rollout.** Pure library-config change, no schema/API/config-file impact. No migration, no
coordination — a rolling deploy is safe, and mixed old/new replicas simply have different
outage latencies.

**Rollback.** Revert the commit. No persisted state depends on it.

**Residual risks.**

- *Blackholed Redis is still slow* (D1). ~2.1 s per failed command, ~6–8 s per decision,
  because `connection_timeout` then dominates. Documented, not fixed. The follow-up levers,
  in order of cost: lower `connection_timeout` (~250 ms is generous for a LAN Redis), then a
  circuit breaker (D6).
- *Less absorption of a slow failover.* A failover needing more than one reconnect attempt
  now surfaces as a cache bypass rather than being ridden out inside the command. This is the
  designed fail-open behavior — correctness is unaffected and the manager keeps reconnecting
  in the background — but it will make `iam_authz_decisions_total{cache="bypass"}` briefly
  noisier during a failover than it is today.
- *Constant drift.* Nothing outside the module's own test asserts the retry count, so the
  §4.1 test is the only thing standing between a future edit and a silent return to 20 s
  decisions. Its failure message must say so.

## 7. Out of scope / follow-ups

- **Bounding the blackholed/hung-Redis shape** — `connection_timeout` tightening and/or a
  circuit breaker (D1, D6). File as a follow-up if the residual matters in practice.
- **Operator-tunable retry config** (D4) — deliberate non-goal until someone needs it.
- **Revisiting the JWKS fail-closed posture** (D15). This design makes it fail *fast*; whether
  it should fail *open* is a separate, deliberately-taken decision and is not reopened here.
- **The four test-only `connect` constructors** — no production caller, left alone.

## 8. Acceptance criteria

1. `paigasus-iam` opens **every** production Redis connection through
   `adapters::redis::connect`, which caps `number_of_retries` at 1 and sets a `max_delay`
   guard; no call site constructs a `ConnectionManager` directly.
2. Both production call sites — `http::connect_redis` and `RedisJwksCache::connect` — are
   converted, each preserving its existing error mapping exactly.
3. A Docker-free unit test proves a command against an unreachable Redis errors within 1 s,
   built from the production config, with an error-kind control assertion.
4. `docs/ops/RUNBOOK-observability.md` §"Authz availability posture" reflects the shipped
   numbers, no longer names SMA-473 as unshipped, corrects the two `backon` facts, revises
   the ~55 s revocation figure, and documents the JWKS fail-closed authn outage.
5. `cedar_authorizer.rs`'s module doc and the SMA-470 acceptance-test comments no longer cite
   19–28 s or an unshipped follow-up.
6. The full CI gate graph passes:
   `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
   :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift :release-parity
   :release-parity-py :release-parity-ts --base origin/main --include-relations`.
