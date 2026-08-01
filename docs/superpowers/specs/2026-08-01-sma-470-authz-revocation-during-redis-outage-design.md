# SMA-470 — Revocation during a Redis outage: bound the staleness, record the posture

**Status:** design
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
window is unbounded in two of the three cases.

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

**D-C — a Redis data loss freezes the snapshot until process restart.**
`Generations::read` maps a missing key to `0` (`generation.rs:88`). After a Redis restart
without persistence, a failover to an empty replica, a `FLUSHALL`, or plain eviction of
`iam:authz:policy_gen`, the counter reads back `0`. Then `reload_if_stale` sees
`0 <= N` and skips, *and* the TTL backstop compiles a snapshot stamped `0` which
`install_if_newer` rejects (`0 > N` is never true). `ConnectionManager` reconnects cleanly,
so the service looks healthy while serving a frozen pre-flush policy set — every subsequent
revoke silently ignored — until the process restarts.

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

**D2 — Fix the three defects in this issue, not a follow-up.** SMA-470 is about revocation
during a Redis outage; D-A/D-B/D-C *are* that failure, in its worst forms. Writing the test
the issue asks for would expose them anyway.

**D3 — Bound the window rather than eliminate it.** Moving the generation counter into
Postgres, so the revoke and its bump commit in one transaction, would remove the window
entirely and is strictly stronger. It is also a migration, a `Generations` backend rewrite,
a change to D11's design, and an extra Postgres read on a path that is currently a cheap
Redis `GET`. Out of scope here; filed as a follow-up (§7).

**D4 — Boot no longer fails when Redis is down.** `PolicySnapshot::new` currently propagates
the gen-read error, so IAM refuses to start during a Redis outage. Under the §3 fallback it
boots on a Postgres-compiled snapshot stamped `0` and self-heals once Redis returns. This is
a deliberate behavior change, consistent with the fail-open posture and strictly more
available.

**D5 — No new metric.** The outage is already directly observable as
`iam_authz_decisions_total{cache="bypass"}`. A "running on a provisional stamp" metric would
pull in `names.rs`, the dashboards, `promtool` and the observability-drift gate for no
operational gain the existing signal doesn't already provide.

## 3. The fix

Confined to `rs/crates/services/paigasus-iam/src/adapters/authz/policy_snapshot.rs`.

### 3.1 Split the install-ordering token out of the generation stamp

`PolicySnapshot` gains `load_seq: AtomicU64`; `SnapshotState` gains `installed_seq: u64`.
`reload_now` claims a sequence number **before** it starts loading:

```rust
async fn reload_now(&self) -> Result<(), AuthzError> {
    let seq = self.load_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let fallback_gen = self.state.read().await.compiled.r#gen;
    let compiled = Self::load_and_compile(self.policies.as_ref(), self.grants.as_ref(), fallback_gen).await?;
    self.install_if_fresher(compiled, seq).await;
    Ok(())
}
```

`install_if_newer` becomes `install_if_fresher`, comparing `seq > state.installed_seq`
instead of `r#gen`. `new()` initializes `load_seq` and `installed_seq` to `0`, so the first
reload's seq of `1` installs.

`loaded_at` keeps its existing semantics — refreshed only on an actual install — but that
now behaves as the module docs intended: because a backstop recompile *does* install, each
one resets the TTL clock, so `spawn_reload` stops the recompile-and-discard spin it performs
today (`:126-129` describes the intent; the gen guard defeats it).

This preserves the monotonic-write guard's purpose and sharpens it. The module docs describe
the race as "the reload that observed an OLDER `policy_gen` acquires the lock AFTER one that
observed a newer gen" — that is an ordering over *when each load started*, which a sequence
number expresses directly and gen-comparison only approximates (it cannot order two reloads
at the same gen, which is exactly the case that breaks the backstop).

### 3.2 Tolerate an unreadable `policy_gen`

`load_and_compile` takes a `fallback_gen: u64` and stops propagating the gen-read error:

```rust
let observed_gen = match policies.policy_gen().await {
    Ok(g) => g,
    Err(err) => {
        tracing::warn!(error = %err, "policy_snapshot: policy_gen unreadable — compiling from Postgres anyway and stamping the last-known generation (fail-open, SMA-470)");
        fallback_gen
    }
};
```

Policies and grants are `list_all`-ed from **Postgres**, so the compiled set is genuinely
fresh even with Redis down; only the generation *stamp* is provisional. `new()` passes `0`
(nothing is installed yet, per D4).

The no-lost-update property of §"No-lost-update gen stamping" is unaffected: the stamp is
still read before the `list_all` calls, and a provisional stamp only ever undercounts, so
the next `reload_if_stale` still sees itself as stale.

### 3.3 Recover from a generation reset

`reload_if_stale` reloads on **inequality**, not advance:

```rust
if store_gen == current_gen {
    return Ok(());
}
self.reload_now().await
```

A regression (`store_gen < current_gen`) means Redis was reset and our stamp is meaningless,
so reloading and re-stamping is correct. It settles after one reload — the new stamp equals
the store's value, so the next call is a no-op. Safe because §3.1 made the install guard
seq-based, so the lower stamp no longer blocks the install.

`reload_if_stale` still returns `Err` when `policy_gen()` errors; every caller already logs
and swallows it (`CedarAuthorizer::is_authorized` step 1, `spawn_reload`'s loop). During an
outage the snapshot therefore refreshes only via the TTL backstop — which, after §3.1/§3.2,
works.

### 3.4 Resulting guarantees

| Scenario | Before | After |
|---|---|---|
| Revoke, Redis down | unbounded stale ALLOW (D-A) | ≤ `authz.policy_cache_ttl_secs` (30s) |
| Revoke, gen bump lost, Redis back | unbounded — the backstop installs nothing (D-B), so the snapshot only advances if some *unrelated* later write bumps the gen | ≤ `authz.policy_cache_ttl_secs` (30s), and sooner if any later bump lands |
| Revoke, Redis flushed | frozen until process restart (D-C) | ≤ `policy_cache_ttl_secs` (30s) |

**The guarantee is two bounds, not one.** Once the backstop installs same-gen recompiles,
the snapshot can pick up a revoke while `r#gen` — and therefore the decision-cache key — is
unchanged, so a pre-revoke cached ALLOW may still be served for up to
`authz.decision_cache_ttl_secs`. Worst-case end-to-end revocation latency on the fail-open
path is therefore `policy_cache_ttl_secs + decision_cache_ttl_secs` (30s + 30s = 60s at the
defaults), not a single TTL. §5 states this explicitly; today's RUNBOOK does not.

Deriving the key from a process-local counter instead would close the second bound, but each
replica's counter differs, which would fragment the key space and destroy cross-replica cache
sharing. Not worth it for a bound that is already documented and short.

## 4. Tests

### 4.1 Unit — `policy_snapshot.rs`

1. `ttl_backstop_installs_a_same_gen_recompile` — a grant lands with no gen bump;
   `reload_now()` must install it. Direct regression test for D-B (probe 1, inverted).
2. `reload_survives_an_unreadable_policy_gen` — a `PolicyStore` fake whose `policy_gen()`
   always errors; `reload_now()` must still install fresh Postgres data, stamped with the
   last-known gen. Covers D-A and §3.2.
3. `gen_regression_after_a_redis_flush_still_reloads` — gen 3 → 0 → 1; `reload_if_stale()`
   must pick up a grant added after the flush. Covers D-C (probe 2, inverted).
4. `install_if_fresher_rejects_an_older_load_arriving_after_a_newer_one` — the existing
   `install_if_newer_rejects_an_older_gen_arriving_after_a_newer_one_is_installed` test,
   re-expressed over `seq`. Its intent (a losing race must not regress the snapshot) is
   preserved.

### 4.2 Unit — `cedar_authorizer.rs`

5. `revoked_grant_stops_being_allowed_once_the_snapshot_reloads_without_a_gen_bump` — seed a
   grant, assert ALLOW, revoke it through the fake store *without* bumping, drive the
   snapshot reload, assert DENY. Pins the full revocation chain with the generation signal
   missing.

### 4.3 Acceptance — `authz_acceptance.rs`

6. `revoke_during_a_redis_outage_denies_once_the_snapshot_backstop_reloads` — the test the
   issue says is missing, against real Postgres + real Redis:
   - `authz.cache.backend = redis`, `policy_cache_ttl_secs = 1`, `refresh_interval_secs = 1`
     (both must be ≥ 1 and `refresh <= ttl` per `IamConfig::validate`).
   - Grant a role via `POST /v1/authz/role-grants`; assert `POST /v1/authz/is-authorized`
     returns `allowed: true`.
   - `redis_node.stop_with_timeout(Some(0))`, mirroring `authz_acceptance.rs:580`'s pattern.
   - `DELETE /v1/authz/role-grants/{id}` — commits to Postgres, the `policy_gen` bump is
     swallowed.
   - Drive the real backstop: `state.snapshot().spawn_reload(50ms, 50ms, shutdown)`
     (`AppState::snapshot()` is public; `main.rs:164` is the production caller). The
     acceptance harness does not spawn the loop itself, and driving the public
     `spawn_reload` exercises the actual production path rather than a private method.
   - Poll `is-authorized` until `allowed: false`, with a bounded timeout so a regression
     fails loudly instead of hanging.

   Docker-gated exactly like its neighbours: a missing daemon is a hard failure under `CI`
   and a skip on a Docker-less laptop.

## 5. Documentation

Rewrite `docs/ops/RUNBOOK-observability.md` §4 "Authz availability posture"
(`:533-554`). The fail-open paragraph (`:535-544`) is accurate and stays. The revocation
paragraph (`:546-554`) is not and must be replaced to state:

- the bump is **best-effort** and is swallowed when Redis is unavailable — it is not the
  guarantee the current text implies;
- the real guarantee is the snapshot's unconditional TTL backstop, served from Postgres and
  independent of Redis;
- worst-case revocation latency is `policy_cache_ttl_secs + decision_cache_ttl_secs`
  (60s at defaults), stated as two bounds;
- D1: fail-closed is not offered, and why.

Also correct `policy_snapshot.rs`'s module docs, which assert the same backstop behavior the
guard currently prevents (`:13-20`, `:35-43`).

## 6. Out of scope

- Fail-closed authz, in any form (D1).
- Postgres-backed generation counters (D3) — follow-up.
- The entity/slice cache and decision cache fail-open read paths: already correct and tested.
- Any new metric, dashboard, or alert (D5).

## 7. Follow-up

File a Linear issue for D3: move `policy_gen`/`entity_gen` into Postgres so a grant/revoke
and its generation bump commit atomically, eliminating the lost-invalidation window instead
of bounding it. Needs an ADR (it changes D11's design) and a migration.

## 8. Acceptance criteria

1. A revoke issued while Redis is down stops being ALLOWed within
   `policy_cache_ttl_secs + decision_cache_ttl_secs`, proven by an acceptance test against
   real Postgres and a stopped Redis container. (The test asserts the snapshot bound with
   Redis still down, where the decision cache is bypassed entirely and so contributes no
   additional window.)
2. The TTL backstop installs a recompile when the generation counter has not moved
   (D-B fixed), with a unit test.
3. A generation reset to `0` no longer freezes the snapshot (D-C fixed), with a unit test.
4. A Redis outage no longer prevents the snapshot from reloading from Postgres (D-A fixed),
   with a unit test.
5. The RUNBOOK's authz-availability section states the real bound and records the
   no-fail-closed decision.
6. `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
   :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift :release-parity
   :release-parity-py :release-parity-ts --base origin/main --include-relations` is green.
