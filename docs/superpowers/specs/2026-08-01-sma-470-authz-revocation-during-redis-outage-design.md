# SMA-470 — Revocation during a Redis outage: bound the staleness, record the posture

**Status:** design (revised after adversarial review)
**Date:** 2026-08-01
**Issue:** [SMA-470](https://linear.app/smaschek/issue/SMA-470/iam-cover-revocation-during-redis-outage-and-decide-whether-to-offer-a)
**Milestone:** Paigasus IAM → Hardening

## 1. Problem

The IAM authz stack is deliberately fail-open on a Redis outage (D11/D12): the decision
cache and the entity/slice cache both degrade a Redis error to a miss and fall through to
the Postgres-backed loader. That posture is well tested **for reads** —
`authz_acceptance.rs:580` asserts a Redis outage never fails a request, and
`authz_cache_redis.rs:165,227` assert a `get` degrades to a miss.

What is untested is **revocation**. `RoleService::revoke` commits the grant deletion to
Postgres and then calls `GenerationsPolicyGenBumper::bump`, which logs and swallows a failed
`policy_gen` bump (`adapters/authz/generation.rs:136-140`; the same posture in
`adapters/persistence/pg_role_grants.rs:55-58`). Revoke a role while Redis is unavailable and
the revoke commits while the invalidation signal is lost.

SMA-470 asks for two things: a test that pins the resulting window, and a recorded decision
on whether operators may opt into fail-closed authz.

### 1.1 What reading the code actually found

The issue describes the consequence as "a revoked principal keeps access for up to the
decision-cache TTL". That is **not** what happens. Three defects compound, and the true
window is unbounded in all three cases.

All three share one root cause: **`CompiledPolicies::r#gen` does triple duty** — freshness
comparator, decision-cache key component, and install-ordering token — and it is sourced
from Redis, so when Redis fails or resets, all three break together.

**D-A — a Redis outage freezes the snapshot entirely.**
During the outage the decision cache is not the staleness source at all:
`CedarAuthorizer::cache_key` reads `entity_gen()`, that read errors, and the cache is
*bypassed* (`cache="bypass"`). The stale ALLOW comes from `PolicySnapshot`, which cannot
reload — both `reload_if_stale` (`policy_snapshot.rs:105`) and `load_and_compile`
(`:144`) read `policy_gen` from Redis first and propagate the error, so the TTL backstop
fails too. Staleness lasts as long as Redis is down, unbounded.

**D-B — the TTL backstop installs nothing, ever.**
`spawn_reload`'s TTL branch calls `reload_now()`, which compiles at the *same* gen when
nothing bumped, and `install_if_newer` requires `compiled.r#gen > state.compiled.r#gen`
(`:132`) — so the recompile is discarded. `loaded_at` is deliberately not refreshed on a
rejected install (`:126-129`), so `ttl_elapsed` stays true and the loop recompiles and
discards on every poll, forever. The backstop's stated purpose (module docs `:15-19`:
bound staleness "even if the generation counter never visibly moves to this replica") is
defeated by the guard that runs after it. `spawn_reload` has **no test anywhere in the
crate**.

D-B is also a standing production **load** defect nobody asked about: past
`policy_cache_ttl_secs` of uptime, every replica performs two full `list_all`s plus a Cedar
compile every `refresh_interval_secs` (default **1 second**) forever, and throws every one
of them away. Fixing D-B reduces steady-state reload work to once per
`policy_cache_ttl_secs`.

**D-C — a Redis data loss freezes the snapshot until process restart.**
`Generations::read` maps a missing key to `0` (`generation.rs:88`). After a Redis restart
without persistence, a failover to an empty replica, a `FLUSHALL`, or plain eviction of
`iam:authz:policy_gen`, the counter reads back `0`. Then `reload_if_stale` sees
`0 <= N` and skips, *and* the TTL backstop compiles a snapshot stamped `0` which
`install_if_newer` rejects (`0 > N` is never true). `ConnectionManager` reconnects cleanly,
so the service looks healthy while serving a frozen pre-flush policy set — every subsequent
revoke silently ignored — until the process restarts.

The two generation keys carry **no TTL**, so under a `maxmemory-policy` of `allkeys-lru` or
`allkeys-random` they are ordinary eviction candidates. That is D-C's cheapest trigger and
needs an operational mitigation (§5).

### 1.2 Evidence

D-B and D-C were confirmed empirically with throwaway probes against
`PolicySnapshot`'s own test fakes (added, run, and reverted during design):

| Probe | Observed |
|---|---|
| `reload_now()` (the TTL backstop) after a grant with no gen bump | picked up the un-bumped grant = **false** |
| After a flush (gen 3 → 0, then a real bump to 1), `reload_if_stale()` | sees the new grant = **false**, gen stuck at 3 |
| …then the TTL backstop `reload_now()` | sees the new grant = **false**, gen **still 3** |

The RUNBOOK (`docs/ops/RUNBOOK-observability.md:546-554`) currently tells operators that
"cached decisions also expire on a plain TTL … bounding the worst-case staleness window".
For the snapshot path that claim is false today.

## 2. Decisions

**D1 — No fail-closed opt-out.** Redis is a pure accelerator; the authoritative policy set
is compiled from Postgres. Denying every request during a Redis outage would convert a
latency degradation into a total outage. The contract is TTL-bounded staleness, fail-open.
This decision is only defensible if the bound is real, which is why §3 ships with it rather
than after it. Recorded in the RUNBOOK (§5).

**D2 — Fix the defects in this issue, not a follow-up.** SMA-470 is about revocation during
a Redis outage; D-A/D-B/D-C *are* that failure, in its worst forms. Writing the test the
issue asks for would expose them anyway.

**D3 — Bound the window rather than eliminate it.** Moving the generation counter into
Postgres, so the revoke and its bump commit in one transaction, would remove the window
entirely and is strictly stronger. It is also a migration, a `Generations` backend rewrite,
a change to D11's design, and an extra Postgres read on a path that is currently a cheap
Redis `GET`. Out of scope here; filed as a follow-up (§8).

**D4 — Decouple the decision-cache key from the generation counter.** The key's policy
component becomes a **content hash of the compiled policy set**, not `r#gen`. This is the
central change and is what makes the rest of the design safe; §3.1 explains why the
alternative is unsound.

**D5 — Add snapshot-reload telemetry.** *(Reverses an earlier "no new metric" decision —
see §7.)* A `cache="bypass"` counter already exists but has **no dashboard panel and no
alert**, and it observes the `entity_gen` read failing, which is not the property being
introduced. Nothing today would reveal a D-B regression.

**D6 — Booting with Redis down stays out of scope.** An earlier draft claimed this design
made IAM boot during a Redis outage. It does not: `AppState::new` calls `connect_redis`
(`adapters/http/mod.rs:301`) — an eager `ConnectionManager::new` — *before*
`PolicySnapshot::new` (`:333`), so with `backend = redis` the service already fails to start
at the connect. Making the accelerator connect lazily is a separate change (§8).

## 3. The fix

### 3.1 Decouple the decision-cache key from the generation counter (D4)

**Why this is required, not an optimization.** §3.3 below deliberately reloads on generation
*inequality* so a reset counter can recover. That makes the installed `r#gen`
**non-monotonic** — and `r#gen` is currently the decision-cache key's policy component
(`cedar_authorizer.rs:141-149,:170` → `decision_cache.rs:41-48`), consulted *before*
evaluation (`cedar_authorizer.rs:171-196` precedes `:204`). A stamp that drops from 7 back
to 0 re-enters a key space that was live earlier, whose Redis entries survive up to
`decision_cache_ttl_secs` — so a pre-revoke cached `Allow` can be returned ahead of a
snapshot that correctly denies. Today `install_if_newer`'s monotonicity makes that
impossible by construction. Fixing D-C without this would *introduce* the exact failure this
issue exists to close — and silently, since cache-hit ALLOWs are not re-audited
(`cedar_authorizer.rs:184-195` re-audits denials only).

**The change.** `CompiledPolicies` gains `content_hash: String` — a `blake3` hex digest over
a canonical encoding of the compiled *inputs* (policy ids + kinds + sources, and grant ids +
principal + role_key + scope + linked_policy_id, each sorted). `blake3` is already a
dependency of `decision_cache.rs`. `decision_key`'s first component becomes that hash
instead of `policy_gen`.

This is strictly better than a counter on every axis the design cares about:

- **Cross-replica sharing is preserved** — identical policy sets hash identically everywhere,
  which a process-local counter could never achieve.
- **A gen reset is a non-event** for the cache.
- **A lost bump is a non-event** for the cache: once the snapshot reloads via the backstop,
  the content changes, the hash changes, and the pre-revoke entries become unreachable.
  This collapses what an earlier draft had to publish as a second, additive TTL bound.
- **Mixed-version deploys can't cross-contaminate**: old replicas key on a decimal `r#gen`,
  new ones on a 64-char hex digest, so the two key spaces are disjoint by construction.

`r#gen` survives, but with exactly one job: the freshness comparator for reload decisions.

### 3.2 Split the install-ordering token out of the generation stamp

`PolicySnapshot` gains `load_seq: AtomicU64`; `SnapshotState` gains `installed_seq: u64`.
`install_if_newer` becomes `install_if_fresher`, comparing `seq > state.installed_seq`.

**The seq is claimed immediately before the first Postgres read, not at reload entry.** The
safety property the guard exists for is "never regress to a policy set read *earlier from
Postgres*", so the token must order the *data read*, not the reload's entry. Claiming it at
entry would put the `policy_gen` read — the step that can stall arbitrarily during exactly
the outage this spec targets — between the token and the data it labels. `load_and_compile`
therefore claims the seq and returns it alongside the compiled set.

A rejected install is logged at `debug` with both sequence numbers; today it is silent.

`loaded_at` keeps its existing semantics — refreshed only on an actual install — but that
now behaves as the module docs intended: a backstop recompile *does* install, so each one
resets the TTL clock and `spawn_reload` stops the recompile-and-discard spin it performs
today.

### 3.3 Tolerate an unreadable `policy_gen`

`load_and_compile` takes a `fallback_gen: u64` and stops propagating the gen-read error:

```rust
let (observed_gen, trusted) = match policies.policy_gen().await {
    Ok(g) => (g, true),
    Err(err) => (fallback_gen, false),   // warn on transition only — see below
};
```

Policies and grants are `list_all`-ed from **Postgres**, so the compiled set is genuinely
fresh even with Redis down; only the generation *stamp* is provisional. The snapshot records
`stamp_trusted: bool` alongside it.

The warn fires **on transition** (trusted → provisional and back), not per attempt: at
`refresh_interval_secs = 1` a per-attempt warn is ≥1 line/second/replica for the whole
outage, on top of the existing reload-failure warn.

The no-lost-update property is unaffected: the stamp is still read before the `list_all`
calls, and a provisional stamp only ever undercounts.

### 3.4 Recover from a generation reset, without a recompile storm

`reload_if_stale` reloads on **inequality**, not advance — a regression means Redis was reset
and the stamp is meaningless, so re-stamping is correct. It settles after one reload.

Two guards keep that from becoming a self-DoS on the authz hot path:

1. **Single-flight.** A `tokio::sync::Mutex` reload gate: `reload_if_stale` uses `try_lock`
   and, if a reload is already in flight, returns immediately and decides against the current
   snapshot. This also removes a *pre-existing* herd — today every in-flight request runs its
   own full `reload_now` after a bump.
2. **Provisional-stamp suppression.** While `stamp_trusted == false`, request-driven reloads
   are suppressed and refreshing is left to the backstop. Without this, a flapping Redis
   where `reload_if_stale`'s read succeeds (N) but `load_and_compile`'s read fails (stamping
   M ≠ N) yields permanent inequality — **a full policy recompile per authorization
   decision, indefinitely**.

`reload_if_stale` still returns `Err` when `policy_gen()` errors; every caller already logs
and swallows it. During an outage the snapshot therefore refreshes via the TTL backstop
alone — which, after §3.2/§3.3, works.

### 3.5 Resulting guarantees

| Scenario | Before | After |
|---|---|---|
| Revoke, Redis down | unbounded (D-A) | ≤ `policy_cache_ttl_secs + refresh_interval_secs` |
| Revoke, gen bump lost, Redis back | unbounded — the backstop installs nothing (D-B), so the snapshot only advances if an *unrelated* later write bumps the gen | ≤ `policy_cache_ttl_secs + refresh_interval_secs`, sooner if any later bump lands |
| Revoke, Redis flushed / gen key evicted | frozen until process restart (D-C) | ≤ `policy_cache_ttl_secs + refresh_interval_secs` |

**The bound includes the poll interval.** The backstop is only *checked* once per `poll`
(`policy_snapshot.rs:181-183`), and `IamConfig::validate` permits
`refresh_interval_secs == policy_cache_ttl_secs` (`config.rs:669-674`), so a legal config
gives 30 + 30 = **60s**, not 30s. Plus reload duration (two `list_all`s + a Cedar compile).

**The bound is conditional on Postgres being reachable.** §3.3 removes the Redis error path,
but a `list_all` or `PolicyEngine::compile` error still aborts the install and leaves
`loaded_at` untouched. A single malformed policy row therefore reproduces unbounded staleness
behind one `warn!` — a named residual risk (§6), and the reason D5's telemetry matters.

**Role-grant/policy revocation only.** `entity_gen` has the identical missing-key→`0` defect
(`generation.rs:82-90`) and an identically swallowed bump, with no §3.4 equivalent. Access
changes driven by *tenancy* state (org archive, membership removal) remain bounded by
`slice_cache_ttl_secs` (60s) + `decision_cache_ttl_secs` (30s). §5 must state the two
separately rather than publishing one number; §8 tracks closing it.

## 4. Tests

### 4.1 Unit — `policy_snapshot.rs`

1. `ttl_backstop_installs_a_same_gen_recompile` — a grant lands with no gen bump;
   `reload_now()` must install it. Regression test for D-B.
2. `reload_survives_an_unreadable_policy_gen` — a store returning `Ok(5)` once (for `new()`)
   and erroring thereafter, so "stamped the last-known gen" is distinguishable from "stamped
   0". Covers D-A and §3.3.
3. `gen_regression_after_a_redis_flush_still_reloads` — gen 3 → 0 → 1; `reload_if_stale()`
   must pick up a grant added after the flush. Covers D-C.
4. `install_if_fresher_rejects_an_older_load_arriving_after_a_newer_one` — the existing
   race test, re-expressed over `seq`; its intent is preserved.
5. `spawn_reload_backstop_installs_and_resets_the_ttl_clock` —
   `#[tokio::test(start_paused = true)]` driving the real loop with `tokio::time::advance`.
   Docker-free, deterministic, and retires "`spawn_reload` has no test anywhere in the crate".
6. `provisional_stamp_suppresses_request_driven_reloads` — §3.4 guard 2, pinning that a
   flapping gen read cannot drive a recompile per decision.

### 4.2 Unit — `decision_cache.rs` / `cedar_authorizer.rs`

7. `decision_key_changes_when_the_policy_content_changes` and
   `decision_key_is_stable_across_replicas_for_identical_content` — the D4 property.
   Replaces the existing `decision_key_changes_when_policy_gen_changes`.
8. `revoked_grant_stops_being_allowed_once_the_snapshot_reloads_without_a_gen_bump` — seed a
   grant, assert ALLOW, revoke without bumping, drive the reload, assert DENY. **This test is
   only passable because of D4**: with a gen-keyed cache the key would be byte-identical and
   `MemoryDecisionCache` would return the cached ALLOW before evaluation ever ran.

### 4.3 Acceptance — `authz_acceptance.rs`

9. `revoke_during_a_redis_outage_denies_once_the_snapshot_backstop_reloads` — the test the
   issue says is missing, against real Postgres + real Redis:
   - `authz.cache.backend = redis`, `policy_cache_ttl_secs = 1`, `refresh_interval_secs = 1`.
   - Grant via `POST /v1/authz/role-grants`; assert `is-authorized` → `allowed: true`.
   - `redis_node.stop_with_timeout(Some(0))`, mirroring `authz_acceptance.rs:580`.
   - `DELETE /v1/authz/role-grants/{id}` — commits to Postgres, the bump is swallowed.
   - Drive the real backstop via `state.snapshot().spawn_reload(...)` **at the configured
     1s/1s**, not an arbitrary fast interval, so the test measures the documented bound
     rather than mere liveness. Assert denial within `ttl + poll + slack`.
   - **Phase 2:** restart Redis and assert the revoked principal stays denied — the case
     where the counter returns while the bump is still lost.
   - Own the loop's lifetime: pass a real shutdown signal and await the `JoinHandle` before
     the test returns, so the task cannot outlive the testcontainer.

   Docker-gated like its neighbours. Note the harness never calls `IamConfig::validate`
   (`tests/support/mod.rs`), so config bounds are the test's own responsibility.

## 5. Documentation

Rewrite `docs/ops/RUNBOOK-observability.md` §4 "Authz availability posture" (`:533-554`).
**Both** paragraphs change — an earlier draft wrongly kept the first:

- `:542-544` currently says fail-closed "was considered and explicitly **deferred** to a
  separate authz-hardening effort". D1 decides it is **not offered**; the text must say so.
- `:546-554` must state that the bump is **best-effort and swallowed** when Redis is
  unavailable; that the real guarantee is the snapshot's Postgres-backed TTL backstop; the
  bound as `policy_cache_ttl_secs + refresh_interval_secs`, qualified as "assuming Postgres
  is reachable"; and the separate, longer `entity_gen` bound (§3.5).
- Note that decision-cache **ALLOW hits are not re-audited**, so a staleness window is also
  an audit gap.
- Add a `maxmemory-policy` mandate: `iam:authz:policy_gen`/`entity_gen` carry no TTL and are
  evictable under `allkeys-*`. Requiring `volatile-*` is the cheapest mitigation in this
  spec.

Add a dashboard panel and an alert for D5's metric, mirroring `IamOutboxRelayStalled`
(`iam.rules.yml:20-24`). Correct `policy_snapshot.rs`'s module docs (`:13-20`, `:35-43`),
which assert the backstop behavior the guard currently prevents.

## 6. Rollout, rollback, residual risk

**Rollout.** No migration, no feature flag; rollback is a plain revert. During a rolling
restart old and new replicas coexist, but D4 makes their decision-cache key spaces disjoint
(§3.1), so neither can serve the other a stale entry. The binding constraint during the
deploy is the *old* replicas' unbounded staleness, not the new bound.

**Residual risks.**
- A persistently failing Postgres read or a malformed policy row keeps the last-good snapshot
  forever behind a single `warn!`. D5's telemetry is what surfaces it.
- `entity_gen`-driven staleness (§3.5) is unchanged by this work.
- Non-monotonic `policy_gen` reads from a Sentinel failover or a read replica would drive
  repeated reloads; §3.4's single-flight bounds the cost, but reads should come from one
  authority.

## 7. Changed decisions vs. the pre-review draft

- **D4 (content-hash key) is new** — adversarial review showed the original design would have
  *introduced* a stale-ALLOW path by making the cache key non-monotonic. This is the largest
  change and it simplifies the guarantee from two additive bounds to one.
- **D5 reverses "no new metric."** The original rationale — "already observable as
  `cache=bypass`" — is factually wrong: that series has no panel and no alert anywhere in
  `ops/observability/`, and it measures the `entity_gen` read, not backstop health.
- **D6 replaces the claim that boot would no longer fail** — that was unreachable, because
  `connect_redis` runs first.
- **The published bound grew** from `policy_cache_ttl_secs` to
  `policy_cache_ttl_secs + refresh_interval_secs`, and is now qualified on Postgres
  reachability and scoped to role-grant/policy revocation.

## 8. Out of scope / follow-ups

- Fail-closed authz, in any form (D1).
- Postgres-backed generation counters (D3) — file a follow-up; needs an ADR and a migration.
- `entity_gen`'s identical missing-key→`0` defect (§3.5) — file a follow-up.
- Lazy/retrying Redis connect so the fleet can boot Redis-down (D6) — file a follow-up.
- The entity/slice and decision cache fail-open *read* paths: already correct and tested.

## 9. Acceptance criteria

1. A revoke issued while Redis is down stops being ALLOWed within
   `policy_cache_ttl_secs + refresh_interval_secs`, proven by an acceptance test against real
   Postgres and a stopped Redis container, driven at the configured interval.
2. The TTL backstop installs a recompile when the generation counter has not moved (D-B),
   with both a direct unit test and a Docker-free `start_paused` test of the real loop.
3. A generation reset to `0` no longer freezes the snapshot (D-C), with a unit test.
4. A Redis outage no longer prevents the snapshot from reloading from Postgres (D-A), with a
   unit test.
5. The decision-cache key is derived from compiled-policy content, not `r#gen` (D4), proven
   by a key-stability/key-change test pair.
6. A flapping generation read cannot drive a policy recompile per decision (§3.4).
7. The RUNBOOK states the real bound, the separate `entity_gen` bound, the
   `maxmemory-policy` mandate, and records D1.
8. `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
   :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift :release-parity
   :release-parity-py :release-parity-ts --base origin/main --include-relations` is green.
