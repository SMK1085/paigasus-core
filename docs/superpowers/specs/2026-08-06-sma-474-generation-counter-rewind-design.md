# SMA-474 — Generation-counter rewind: restore monotonicity at the counter

**Status:** design
**Date:** 2026-08-06
**Issue:** [SMA-474](https://linear.app/smaschek/issue/SMA-474/iam-entity-gen-has-the-same-missing-key0-reset-defect-sma-470-fixed)
**Milestone:** Paigasus IAM → Hardening

## 1. Problem

`Generations` keeps two Redis counters — `iam:authz:policy_gen` and `iam:authz:entity_gen`.
Both are written with a bare `INCR` (so neither carries a TTL) and read with a `GET` whose
missing-key case maps to `0` (`adapters/authz/generation.rs:81-94`). Any event that loses the
key therefore **silently rewinds** that counter: eviction under an `allkeys-*` maxmemory
policy, a `FLUSHALL`, a restart without persistence, or a failover to an empty replica.

SMA-470 fixed this class of defect for `policy_gen` only, via two changes that both live
*downstream* of the counter: the policy snapshot reloads on generation **inequality** rather
than advance, and the decision cache is keyed on `CompiledPolicies::content_hash` rather than
the counter. `entity_gen` received neither. It remains a raw component of **two** key spaces:

| Key space | Shape | TTL |
|---|---|---|
| Entity-slice cache | `iam:authz:slice:<entity_gen>:<resource-prn>:<principal-prn>` | `authz.slice_cache_ttl_secs` (default 60) |
| Decision cache | `iam:authz:dec:<policy_content>:<entity_gen>:<blake3>` | `authz.decision_cache_ttl_secs` (default 30) |

A rewind lets the fleet **re-enter a key space that was live earlier**, and any entry still
inside its TTL is replayed. Concretely: the counter reaches 1, a tenancy mutation bumps it to
2, the key is evicted so the next read returns 0, and the following mutation bumps it back to
1 — where entries written *before* mutation #2 are still cached. Access changes driven by
tenancy state (an organization archived, a membership removed) are what flow through this
path.

The issue asks for one of two outcomes: port SMA-470's treatment, or record a decision that
the RUNBOOK's `volatile-*` mandate is sufficient mitigation on its own.

### 1.1 What reading the code changed about the framing

**The exposure is TTL-bounded, unlike SMA-470's.** SMA-470's unbounded window came from a
monotonicity *gate*: `install_if_newer` required the generation to strictly advance, so a
rewind to `0` permanently blocked every subsequent install and froze the snapshot until
process restart. Nothing on the entity path has such a gate — there is no snapshot, only two
`set_ex` caches. The worst case here is therefore `slice_cache_ttl_secs +
decision_cache_ttl_secs` (90 s at the defaults), and a rewind that does *not* collide with a
still-live key space is merely a cold cache, which is safe. The fix must be proportionate to
that, which is why this design does not reach for SMA-470's machinery.

**Neither option the issue offers is right on its own.** Content-addressing does not transfer
(§2.1), and documentation alone leaves the failure completely undetectable (§2.2). The
remaining option — the one the issue does not name — is to fix the defect **at the counter**
rather than at either key space, which fixes both counters at once and leaves the two cache
key shapes untouched.

### 1.2 Mitigating factor carried over from SMA-470's review

`AppState::new` wires the generations, the slice cache and the decision cache off the **same**
Redis connection (`adapters/http/mod.rs:313-402`), so a `FLUSHALL` that resets a counter also
wipes both caches — leaving a cold, correct cache rather than a stale one. The hazard needs a
**selective** loss of just the counter key, which is exactly what `allkeys-*` eviction
produces, since neither generation key carries a TTL.

## 2. Decisions

### 2.1 D1 — Content-addressing the entity path is rejected, on structural grounds

SMA-470 replaced `policy_gen` in the decision-cache key with a blake3 content hash of the
compiled policy set. That worked because the compiled policy set is a **single global object
already resident in memory** at the moment the key is built — `CedarAuthorizer` reads the
snapshot in step 2 and uses its `content_hash` in step 3, at no extra cost.

An entity slice has neither property. It is per-`(resource, principal)`, and it only exists
**after** the Postgres load that the slice cache exists to avoid. Deriving the cache key from
the slice's content would require performing that load on every lookup, which is precisely
what the cache is there to prevent. This is a structural mismatch, not a cost trade-off, and
no amount of engineering makes the technique transfer.

### 2.2 D2 — `volatile-*` alone is rejected as sufficient mitigation

The RUNBOOK's `maxmemory-policy` mandate is the correct *mitigation* and stays. It is not
sufficient as the *only* response, because nothing verifies or detects a violation. A
misconfigured cluster produces silent rewinds indefinitely, and the mandate is enforced by
hoping an operator read the RUNBOOK. Detection is the missing half.

### 2.3 D3 — The fix lives in `Generations`, and applies to both counters

The mechanism is counter-agnostic and belongs in the shared `read`/`bump` helpers. Scoping it
to `entity_gen` would mean *adding* a conditional — more code, not less — and the RUNBOOK's
mandate already names both keys, so a half-fix leaves the mandate half-needed.

Honest accounting of the benefit per counter:

- **`entity_gen`** — a correctness fix. This is the counter with no downstream protection.
- **`policy_gen`** — observability and monotonicity hygiene, *not* a correctness fix. Post
  SMA-470 it already self-heals: `reload_if_stale` compares on inequality
  (`policy_snapshot.rs:216`), so a rewind triggers exactly one reload which re-stamps the
  snapshot at the rewound value, after which `store_gen == current_gen` and the churn stops.
  The repair is compatible with that: a repaired counter still differs from the installed
  stamp, so the same single reload happens.

### 2.4 D4 — An unrepairable rewind returns `Err`, reusing the existing bypass

Every caller of a generation read already has a tested fail-open path for an error, because
SMA-444/SMA-470 built them for the Redis-outage case:

| Caller | Existing behaviour on `Err` |
|---|---|
| `CedarAuthorizer::cache_key` | returns `None` → decision cache bypassed, `iam_authz_decisions_total{cache="bypass"}` |
| `SliceCache::load` | skips the slice cache, falls through to the Postgres loader |
| `PolicySnapshot::reload_if_stale` | error is caught and logged by `is_authorized`; decides against the last-known-good snapshot |
| `PolicySnapshot::load_and_compile` | stamps provisionally; the TTL backstop owns refresh |

An unrepairable rewind means the counter cannot be trusted, which is semantically identical to
"the counter could not be read". Reusing these paths costs **zero new degradation code** and is
correct by construction. The cliff is a performance one: while the repair keeps failing, every
decision pays a raw Postgres slice load. That is safe, and it is loud — `IamAuthzRedisCacheBypassed`
already alerts on exactly this signal.

### 2.5 D5 — The repair is `INCRBY`, not a Lua `EVAL`

The `script` feature is deliberately trimmed from the workspace `redis` dependency
(`rs/Cargo.toml:130-135`), so `redis::Script` would require re-enabling it. It is not needed.
`INCRBY key (high_water + 1)` is atomic in a single round trip and always lands **strictly
forward**: the result is `observed + high_water + 1`, which exceeds `high_water` for any
`observed >= 0`. Concurrent repairs from several replicas simply add, overshooting further —
harmless, because the generation is a cache-key discriminator with no meaning of its own, and
because after the first successful repair no replica detects a rewind any more, so the process
converges immediately. `u64` headroom is not a practical concern.

## 3. The fix

### 3.1 Carry a process-local high-water mark per Redis counter

`Generations::Redis` grows from holding a bare `ConnectionManager` to holding a
`RedisGenerations` struct: the connection plus two `Arc<AtomicU64>` high-water marks. This
mirrors `MemoryGenerations`' existing shape, so every clone of a `Generations` handle shares
the same marks — the same `Arc`-backed, cheap-to-clone posture the type already has.

The one construction site outside the module is `adapters/http/mod.rs:323`
(`Generations::Redis(conn.clone())`). Rather than widen the public variant's payload for that
caller, add a `Generations::from_connection(conn)` constructor alongside the existing
`memory()` / `redis_connect()` entry points, matching the `from_connection` naming already used
by `SliceCache` and `RedisDecisionCache` for the shared-connection wiring.

`Generations::Memory` is untouched. In-process `AtomicU64` counters cannot rewind, so the
memory backend needs no guard and pays no cost.

### 3.2 One shared guard, used by both `read` and `bump`

Every successful Redis observation raises its counter's high-water mark with `fetch_max`. Both
paths then run the same guard on the observed value:

1. `observed >= high_water` — return it. This is the steady state: one atomic compare, no
   extra round trip, no behavioural change.
2. `observed < high_water` — a rewind. Issue `INCRBY key (high_water + 1)`, raise the
   high-water to the result, count `outcome="repaired"`, warn, and return the repaired value.
3. The `INCRBY` fails — count `outcome="repair_failed"` and return `Err` (D4).

**The guard must be in `bump` as well as `read`.** `INCR` against a rewound-to-missing key
returns `1`, which is precisely the dangerous re-entry: a tenancy mutation immediately after
an eviction would write its cache entries into the gen-1 key space where pre-mutation entries
may still be live. A guard on `read` alone would leave that hole open.

A genuinely fresh deployment is undisturbed: nothing has bumped, so `high_water` is `0`,
`observed` is `0`, and `0 >= 0` takes the steady-state branch.

### 3.3 Why the repair must be written back to Redis

A local-only jump forward — using `high_water + 1` in-process without persisting it — is not
sufficient. Other replicas would still read the rewound value and keep writing into the old
key space, so the fleet would fragment into per-replica key spaces rather than converge. The
`INCRBY` is what makes every replica agree on a generation beyond the rewind.

### 3.4 Limitation: a replica's high-water is a lower bound, not the fleet maximum

A replica's high-water reflects only what **that replica** has observed. A replica whose mark
lags the fleet maximum could repair to a value another replica still has live entries under,
so the repair does **not** categorically eliminate key-space re-entry in every topology.

Two things bound this in practice. First, every replica reads `entity_gen` on essentially
every authz decision (`CedarAuthorizer::cache_key`, plus the slice cache's own read), so a
replica that is serving traffic lags the fleet by sub-seconds; a replica that is *not* serving
traffic is also not consuming the cache. Second, in the lagging case the outcome is no worse
than today's TTL bound — the repair can only reduce the set of colliding generations, never
enlarge it.

The claim this design makes is therefore **"strictly reduces the exposure"**, not "eliminates
it". Actual elimination is the operator setting `maxmemory-policy` to a `volatile-*` value —
which is exactly what the new metric exists to prompt. Eliminating the window structurally is
SMA-475's job (§6).

### 3.5 Resulting guarantees

- A rewind is detected on the first read or bump that observes it, by any replica.
- After one successful repair, the fleet's counter is strictly beyond the repairing replica's
  entire observed history, so that replica cannot re-enter any key space it has used.
- A rewind is no longer silent: it increments a counter, emits a warn, and can fire an alert.
- A rewind that cannot be repaired degrades to the existing, tested cache-bypass posture —
  correct, slower, and already alerted.
- The memory backend, both cache key shapes, and the `authz.*` config surface are all
  unchanged.

## 4. Observability

New counter in `paigasus-observability`'s `names.rs`:

```
iam_authz_generation_rewinds_total{counter="policy_gen"|"entity_gen", outcome="repaired"|"repair_failed"}
```

Both label sets are closed and tiny — no cardinality risk. The Redis `ErrorKind` is
deliberately **not** a label, matching the SMA-470 posture that an error is never a label.

- **Grafana** — a panel on the IAM dashboard (`ops/observability/grafana/dashboards/iam.json`),
  alongside the existing authz panels.
- **Prometheus** — `IamAuthzGenerationRewound`, **warning** severity:
  `sum(increase(iam_authz_generation_rewinds_total[15m])) > 0`. A rewind is near-conclusive
  evidence that `maxmemory-policy` is `allkeys-*`, i.e. that the RUNBOOK mandate is being
  violated. Warning rather than critical because the mechanism self-heals and the residual
  exposure is TTL-bounded.
- **promtool fixture** — in `ops/observability/prometheus/rules/tests/iam.test.yml`, including
  a **control series** that must *not* fire (SMA-466's lesson: an all-firing fixture cannot
  discriminate a correct expression from a trivially-true one).
- **RUNBOOK** — a catalog row and an alert entry in `docs/ops/RUNBOOK-observability.md`, plus
  two corrections to existing text (§5).

## 5. Documentation

`docs/ops/RUNBOOK-observability.md` needs three edits beyond the new alert entry:

1. The **`maxmemory-policy` mandate** paragraph currently states that evicting a generation key
   "silently rewinds that counter". After this change it does not — it is detected, repaired
   and counted. The mandate itself stays (it is still the right configuration, and §3.4's
   limitation means detection is not a substitute for it), but the "silently" claim must go and
   the new signal must be named as the way a violation surfaces.
2. The **`entity_gen` bound** paragraph ("This bound covers policy and role-grant revocation
   only") should record that the 90 s tenancy-path bound is now the *residual* exposure after
   repair, and cross-reference D1's structural reason for why content-addressing did not
   transfer.
3. The **metric catalog** table gains the new counter.

## 6. Out of scope

- **Retrying a swallowed bump.** The *stall* half of the defect — a `bump_entity_gen` that
  fails and is swallowed (`pg_organizations.rs:45-47`, `pg_teams.rs:38-40`,
  `pg_projects.rs:38-40`) — is a different failure mode with a different fix (a pending-bump
  flag flushed on the next successful Redis interaction). It is not what SMA-474 describes and
  is not attempted here.
- **Postgres-backed generation counters.** SMA-475 eliminates the window rather than bounding
  it, and wants an ADR first. This design is deliberately the cheap, local hardening that makes
  the current architecture honest in the meantime; it does not prejudge SMA-475.
- **Any change to the two cache key shapes**, to `authz.*` config, or to the memory backend.

## 7. Tests

### 7.1 Unit — `generation.rs`

- The memory backend is unaffected: existing tests continue to pass unchanged, and the guard
  is never reached on that path.
- Both `read` and `bump` raise the high-water mark.
- A fresh handle at `0` with nothing bumped takes the steady-state branch (no spurious repair).

### 7.2 Docker-gated integration — `authz_generations_redis.rs`

This file already has the container harness and the CI-hard-fail / local-skip gating.

- **Rewind is repaired:** bump to N, `DEL` the key, then read — must return a value `> N`,
  never `0`.
- **The repair is persisted:** after the above, Redis itself holds the repaired value, so a
  *second* `Generations` handle reading the same key observes it. `authz_acceptance.rs:449`
  already establishes the two-independent-`Generations` pattern this needs.
- **A rewind followed by a bump cannot re-enter a used generation:** bump to N, `DEL`, then
  `bump` — the result must exceed N, proving the guard is on the bump path too (§3.2). This is
  the test that fails if the guard is only added to `read`.
- **`policy_gen` and `entity_gen` repair independently** — repairing one must not move the
  other, matching the existing independence test.

Every new test must be **mutation-tested**: each is required to fail against the pre-fix code
before it is accepted, matching the bar SMA-470 set.

### 7.3 Known test gap — the `repair_failed` path

There is no clean way to fail *only* the `INCRBY`. Every route tried (a wrong-type key, a
non-integer value) fails the preceding `GET` instead, exercising the pre-existing error path
rather than the new one. The plan is to extract the repair behind a seam and unit-test its
error mapping directly, and to rely on the existing caller-side bypass tests
(`entity_cache.rs`'s `load_fails_open_to_the_inner_loader_when_entity_gen_errors`,
`cedar_authorizer.rs`'s bypass tests) for the behavioural contract. This is recorded as a
deliberate gap rather than papered over with a test that proves nothing.

## 8. Rollout, rollback, residual risk

**Rollout** is a plain deploy; there is no migration, no config change, and no key-shape
change, so replicas running the old and new code interoperate — an old replica simply does not
detect rewinds, and a repair written by a new replica is an ordinary counter value to it.

**Rollback** is a plain revert. A counter left at a repaired (larger) value is harmless: it is
a discriminator, and a larger value is exactly as valid as a smaller one.

**Residual risk** is §3.4's lagging-replica case, bounded by the same 90 s TTL that bounds the
defect today, and the `repair_failed` test gap in §7.3.

## 9. Acceptance criteria

1. A rewound Redis generation counter is detected by `Generations` on the next `read` **or**
   `bump`, for both `policy_gen` and `entity_gen`.
2. A detected rewind is repaired forward in Redis with a single atomic `INCRBY`, so other
   replicas converge on the repaired value.
3. A repair that fails returns `Err`, degrading onto the existing cache-bypass paths — no new
   degradation code, and no request is failed.
4. `iam_authz_generation_rewinds_total{counter, outcome}` is emitted, panelled, and alerted on
   by `IamAuthzGenerationRewound`, with a promtool fixture that includes a non-firing control
   series.
5. The memory backend, both cache key shapes, and the `authz.*` config surface are unchanged.
6. The RUNBOOK no longer claims a generation-key eviction "silently rewinds" the counter, and
   records D1 (why content-addressing does not transfer) and D2 (why `volatile-*` alone is not
   sufficient) as the decisions SMA-474 asked for.
