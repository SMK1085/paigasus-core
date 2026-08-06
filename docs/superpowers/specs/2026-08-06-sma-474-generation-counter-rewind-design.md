# SMA-474 — Generation-counter rewind: restore monotonicity at the counter

**Status:** design (revised after adversarial review)
**Date:** 2026-08-06
**Issue:** [SMA-474](https://linear.app/smaschek/issue/SMA-474/iam-entity-gen-has-the-same-missing-key0-reset-defect-sma-470-fixed)
**Milestone:** Paigasus IAM → Hardening

## 1. Problem

`Generations` keeps two Redis counters — `iam:authz:policy_gen` and `iam:authz:entity_gen`.
Both are written with a bare `INCR` (so neither carries a TTL) and read with a `GET` whose
missing-key case maps to `0` (`adapters/authz/generation.rs:81-90`, the `unwrap_or(0)` at
`:87`). Any event that loses the key therefore **silently rewinds** that counter: eviction
under an `allkeys-*` maxmemory policy, a `FLUSHALL`, a restart without persistence, or a
failover to an empty replica.

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
`set_ex` caches (`entity_cache.rs:117`, `decision_cache.rs:170`), neither of which holds any
in-process state. The worst case here is therefore `slice_cache_ttl_secs +
decision_cache_ttl_secs` (90 s at the defaults), and a rewind that does *not* collide with a
still-live key space is merely a cold cache, which is safe.

**That bound rests on a wiring invariant that nothing enforces.** `MemoryDecisionCache` has no
TTL and no eviction (`decision_cache.rs:77-84`), so pairing it with a Redis-backed
`Generations` would make the window unbounded again. Today `AppState::new` only ever pairs it
with `Generations::memory()`, which cannot rewind (`http/mod.rs:312-325`, `:401-404`). This
design depends on that pairing and does not change it; a future wiring that breaks it
invalidates §1.1's bound.

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

This has a consequence for §4's alerting that the first draft of this design got wrong: of the
four rewind causes listed in §1, three (`FLUSHALL`, restart without persistence, failover to
an empty replica) destroy the caches along with the counter and are therefore **benign**. Only
selective eviction is hazardous. The signal cannot distinguish them by itself — see §4.

## 2. Decisions

### 2.1 D1 — Content-addressing the entity path is rejected, on structural grounds

SMA-470 replaced `policy_gen` in the decision-cache key with a blake3 content hash of the
compiled policy set. That worked because the compiled policy set is a **single global object
already resident in memory** at the moment the key is built — `CedarAuthorizer` reads the
snapshot at `cedar_authorizer.rs:189` and uses its `content_hash` at `:193`, at no extra cost.

An entity slice has neither property. It is per-`(resource, principal)`, and it only exists
**after** the Postgres load that the slice cache exists to avoid: `SliceCache::load` builds its
key at `entity_cache.rs:97`, before any Postgres read, and `CedarAuthorizer` builds the
decision-cache key at `:193`, before the slice load at `:227`. Deriving either key from the
slice's content would require performing that load on every lookup, which is precisely what
the cache is there to prevent. Nor is there a cheap per-org discriminator (a version column, a
`max(updated_at)`) that is not itself a Postgres round trip — that is SMA-475's territory. This
is a structural mismatch, not a cost trade-off.

### 2.2 D2 — `volatile-*` alone is rejected as sufficient mitigation

The RUNBOOK's `maxmemory-policy` mandate is the correct *mitigation* and stays. It is not
sufficient as the *only* response, because nothing verifies or detects a violation. A
misconfigured cluster produces silent rewinds indefinitely, and the mandate is enforced by
hoping an operator read the RUNBOOK. Detection is the missing half.

### 2.3 D3 — The fix lives in `Generations`, and applies to both counters

The mechanism is counter-agnostic and belongs in the shared `read`/`bump` helpers. Scoping it
to `entity_gen` would mean *adding* a conditional, and the RUNBOOK's mandate already names both
keys, so a half-fix leaves the mandate half-needed.

Honest accounting of the benefit per counter:

- **`entity_gen`** — a correctness fix. This is the counter with no downstream protection.
- **`policy_gen`** — observability and monotonicity hygiene, *not* a correctness fix. Post
  SMA-470 it already self-heals: `reload_if_stale` compares on inequality
  (`policy_snapshot.rs:216`), so a rewind triggers exactly one reload which re-stamps the
  snapshot at the rewound value, after which `store_gen == current_gen` and the churn stops.

D4 is what makes applying this to `policy_gen` safe. An earlier draft had an unrepairable
rewind return `Err`, which would have been a **freshness regression** on `policy_gen`: an `Err`
drives `load_and_compile` to a provisional stamp, and `reload_if_stale` then suppresses
request-driven reloads entirely (`policy_snapshot.rs:209-214`), costing same-decision
revocation visibility — up to `policy_cache_ttl_secs + refresh_interval_secs` (~31 s). Trading
that for hygiene would have been a bad deal. With D4 as revised, `read` never returns a *new*
error, so the trade disappears.

### 2.4 D4 — A failed repair falls back locally; it never invents a new error

`Err` from `Generations::read` stays reserved for what it means today: **the `GET` itself
failed**. Every caller already has a tested fail-open path for that, built for the Redis-outage
case by SMA-444/SMA-470:

| Caller | Existing behaviour on `Err` |
|---|---|
| `CedarAuthorizer::cache_key` (`:164-168`) | returns `None` → decision cache bypassed, `iam_authz_decisions_total{cache="bypass"}` |
| `SliceCache::load` (`entity_cache.rs:90-96`) | skips the slice cache, falls through to the Postgres loader |
| `PolicySnapshot::reload_if_stale` (`:215`) | error is caught and logged by `is_authorized` (`cedar_authorizer.rs:180-182`); decides against the last-known-good snapshot |
| `PolicySnapshot::load_and_compile` | stamps provisionally; the TTL backstop owns refresh |

A **failed repair** is different and must not reuse that path. Two reasons:

1. **It would be permanent.** The high-water mark never decreases, so once a repair fails,
   `observed < high_water` holds on that replica forever — every subsequent read errors, giving
   100 % bypass on both caches with no self-heal and no backoff, until the process restarts or
   the fleet organically bumps past the mark. A raw Postgres slice load per decision,
   indefinitely, is a worse operational outcome than the 90 s stale window this design exists
   to shrink.
2. **On `policy_gen` it costs freshness, not just speed** (§2.3).

So: on a failed repair, `read`/`bump` return `Ok(high_water + JUMP)` **locally** without having
persisted it, count `outcome="repair_failed"`, and warn. §3.3 rejects a local-only jump as a
*replacement* for the write, correctly — the fleet would fragment into per-replica key spaces.
As a *fallback* the trade inverts: fragmented key spaces are disjoint and therefore safe, they
merely stop the fleet sharing cache entries until the counter is repaired, and that is far
cheaper than bypassing the caches entirely.

**Considered and rejected:** making this asymmetric — `Err` for `entity_gen` (where a bypass is
merely slow) and a local fallback for `policy_gen` (where it costs freshness). `Err` is strictly
safer for `entity_gen`, but only against §3.4's residual case, and it buys that at the price of
the permanent-bypass cliff in (1). Uniform local fallback is the better trade, and
`repair_failed` alerts so the root cause gets fixed.

### 2.5 D5 — The repair is `INCRBY`, not Lua and not `SET NX`

The `script` feature is deliberately trimmed from the workspace `redis` dependency
(`rs/Cargo.toml:130-135`), so `redis::Script` would require re-enabling it. It is not needed.

**Invariant.** `INCRBY key delta` with `delta = high_water + JUMP` yields `stored + delta`,
which for any stored value `>= 0` satisfies `result >= high_water + JUMP`. That — not a
specific arithmetic result — is the property the guard depends on. (The value at `INCRBY` time
need not equal what the `GET` returned; they are separate round trips, and in the missing-key
case the stored value is `0` by `INCRBY`'s own initialization.)

**Boundary type.** Redis counters are **i64**, not u64. `INCRBY` past `i64::MAX` returns `ERR
increment or decrement would overflow`, and a delta above `i64::MAX` is rejected outright as
out of range. §3.2's ceiling check exists for this; "u64 headroom" is not the constraint.

**Considered and rejected — `SET key <high_water + JUMP> NX`.** Attractive because it is
idempotent under concurrency: only the first writer wins, so N concurrent repairs cannot
compound. But it silently no-ops when the key *exists* at a lower value (a failover to a stale
replica), so it needs an `INCRBY` fallback anyway — two primitives, two code paths, and two
round trips in the partial-rewind case. With the per-process single-flight and the ceiling
check in §3.2, `INCRBY`'s overshoot is bounded and harmless, so the simpler shape wins.

**Overshoot is bounded.** Every replica reads a generation on essentially every decision
(`cedar_authorizer.rs:164`, and `pg_entity_slice.rs:173` via `SliceCache::load`), so at the
instant of a rewind many in-flight requests observe it at once. §3.2 single-flights the repair
per counter per process, so each replica contributes at most one `INCRBY` per rewind event, and
a fleet of N replicas advances the counter by at most `N × (high_water + JUMP)`. At
`JUMP = 1_000_000` and N in the hundreds that is ~10⁸ per rewind event against an `i64::MAX` of
~9.2 × 10¹⁸ — around 10¹⁰ rewind events of headroom. The counter is a cache-key discriminator
with no meaning of its own, so a large value costs nothing.

## 3. The fix

### 3.1 Carry a process-local high-water mark per Redis counter

`Generations::Redis` grows from holding a bare `ConnectionManager` to holding a
`RedisGenerations` struct: the connection, two `Arc<AtomicU64>` high-water marks, and two
single-flight repair mutexes (§3.2). This mirrors `MemoryGenerations`' existing shape, so every
clone of a `Generations` handle shares the same marks — the same `Arc`-backed, cheap-to-clone
posture the type already has.

Two consequences to handle rather than discover during implementation:

- `Generations::Redis(ConnectionManager)` is a **public tuple variant**, so changing its payload
  is a breaking change to a `pub enum`. Add a `Generations::from_connection(conn)` constructor
  alongside `memory()` / `redis_connect()` — matching the `from_connection` naming already used
  by `SliceCache` and `RedisDecisionCache` — and update the one construction site,
  `http/mod.rs:323`.
- `RedisGenerations` needs the same `pub`-but-unconstructible treatment `MemoryGenerations`
  already carries for the `private_interfaces` lint (`generation.rs:25-35`): every field
  private, reachable only through the variant.

`Generations::Memory` is untouched. In-process `AtomicU64` counters cannot rewind, so the
memory backend needs no guard and pays no cost.

### 3.2 A pure guard, used by both `read` and `bump`

The decision logic is a **pure function with no connection**, so it can be unit-tested
exhaustively without Docker:

```rust
enum GuardOutcome { Steady, Repair { delta: u64 }, Ceiling }
fn guard(observed: u64, high_water: u64) -> GuardOutcome
```

- `observed >= high_water` → `Steady`. This is the steady state: one atomic compare, no extra
  round trip, no behavioural change.
- `observed < high_water` and `high_water.saturating_add(JUMP)` is within the i64 ceiling →
  `Repair { delta: high_water + JUMP }`.
- otherwise → `Ceiling`.

Around it, both `read` and `bump` do the same thing. Every successful Redis observation raises
its counter's high-water with `fetch_max`. On `Repair`, take that counter's single-flight mutex
(mirroring `PolicySnapshot::reload_gate`, `policy_snapshot.rs:128-142`) so concurrent in-flight
requests on this replica issue one `INCRBY`, not one each; re-check the guard under the lock,
then `INCRBY key delta`, raise the high-water to the result, count `outcome="repaired"`, warn,
and return it. On an `INCRBY` failure, fall back locally per D4 and count
`outcome="repair_failed"`. On `Ceiling`, fall back locally and count `outcome="ceiling"`; the
RUNBOOK documents the operator remediation (flush `iam:authz:slice:*` and `iam:authz:dec:*`,
then `SET` both counters to `0`).

**The guard must be in `bump` as well as `read`.** `INCR` against a rewound-to-missing key
returns `1`, which is precisely the dangerous re-entry: a tenancy mutation immediately after an
eviction would write its cache entries into the gen-1 key space where pre-mutation entries may
still be live. A guard on `read` alone would leave that hole open. Note the consequence when a
repair fails on the bump path: the `INCR` has already committed, so Redis is left holding the
re-entry value while this replica uses its local fallback. Other replicas detect and repair on
their own next read; §7.2 asserts the resulting Redis state rather than leaving it implied.

A genuinely fresh deployment is undisturbed: nothing has bumped, so `high_water` is `0`,
`observed` is `0`, and `0 >= 0` takes `Steady`.

**Two contract changes to document at the call sites.** `bump`'s doc currently says "Both
return the value AFTER the bump" (`generation.rs:94`) — after a repaired bump the delta is not
1, and the memory backend still returns +1, so the two backends now differ. No caller depends
on the value (every site at `pg_organizations.rs:46`, `pg_teams.rs:39`, `pg_projects.rs:39`,
`pg_role_grants.rs:68`, `generation.rs:140` discards it). And `GenerationsReader`'s port doc
(`cedar_authorizer.rs:106-115`) describes a pure read; post-fix a read can perform a Redis
write, which its test doubles do not model.

### 3.3 Why the repair must be written back to Redis

A local-only jump forward — using `high_water + JUMP` in-process without persisting it — is not
sufficient as the primary path. Other replicas would still read the rewound value and keep
writing into the old key space, so the fleet would fragment into per-replica key spaces rather
than converge. The `INCRBY` is what makes every replica agree on a generation beyond the
rewind. (As a *fallback* after a failed `INCRBY`, that same fragmentation is acceptable — see
D4.)

### 3.4 `JUMP` must be large, and why the minimum jump is actively wrong

The repair jumps by a large constant, `JUMP = 1_000_000`, not by the minimum `+1`. This is not
defensive padding — **a minimum jump makes the defect worse than doing nothing**, and worse the
more mature the deployment:

> Fleet at `entity_gen = 100`; replica A's high-water is 100. Over the next 5 s replica B
> performs 5 tenancy mutations, driving the counter 100 → 105 and writing slice entries at gens
> 101–105 (TTL 60 s). A serves no traffic in that window — idle, or held out of the load
> balancer by a readiness gate during a rolling deploy — so its mark stays at 100.
> `allkeys-lru` then evicts the counter key. **Repairing to `high_water + 1` puts A in the
> gen-101 key space**, whose entries are 5 s old and reflect the *pre-mutation* state: a stale
> `Allow`. **Without any repair** A would have read `0` and used the gen-0 key space, whose
> entries expired ~100 mutations ago — cold and correct.

The minimum jump therefore *creates* the collision. `JUMP = 1_000_000` sizes the gap against
the only thing that matters: a cache entry is live only if it was written within the longest
cache TTL, so the repair is safe as long as `high_water + JUMP` exceeds every generation used
in that window. One million generations is one million tenancy mutations — at a 60 s TTL, over
16,000 mutations per second sustained. That is falsifiable and far outside any plausible load
for this system.

**Residual limitation, stated precisely.** The guarantee needs `JUMP` to exceed the bumps the
fleet performed *since this replica's last successful counter read*. For any replica serving
traffic that is sub-second's worth. It is not bounded for a replica that has not read the
counter in a long time — a canary held out of the LB for an hour, say — and could in principle
land inside the live band again. `policy_gen` is immune in practice (the snapshot's poll tick
reads it every `refresh_interval_secs`, default 1 s, regardless of traffic); `entity_gen` is
read only on decisions, so it carries this residue. The claim is therefore that `JUMP` reduces
the residual window by roughly six orders of magnitude, **not** that it eliminates it.
Structural elimination needs a durable generation floor, which is SMA-475.

### 3.5 What happens to the orphaned entries

Entries written under generations between the rewound value and the repaired one are simply
abandoned and age out at their own TTL. Redis memory is unchanged by the repair — those entries
already existed — so a repair does not itself relieve the pressure that caused an eviction.

### 3.6 Resulting guarantees

- A rewind **below the observing process's high-water mark** is detected on the first `read` or
  `bump` that observes it. A rewind to a value at or above that mark, or any rewind seen by a
  just-restarted replica (whose mark starts at 0), is undetectable by construction.
- After one successful repair, the fleet's counter is beyond every generation that replica has
  observed, plus `JUMP` — so that replica cannot re-enter any key space with live entries,
  subject to §3.4's residue.
- A rewind is no longer silent: it increments a counter, emits a warn, and fires an alert.
- A rewind that cannot be repaired degrades to a disjoint local key space, never to a permanent
  cache bypass and never to a new error.
- The memory backend, both cache key shapes, and the `authz.*` config surface are unchanged.

### 3.7 New failure mode introduced: the read path can now be rejected under `maxmemory`

`RUNBOOK-observability.md:869-877` currently guarantees that the generation read is a plain
`GET` — `readonly fast`, **not** `denyoom` — so it "keeps succeeding at `maxmemory` even under
`noeviction`". `INCRBY` carries the same `write denyoom` flags as `INCR`, so after this change
a `read` **can** be rejected with `OOM command not allowed`, and it can be rejected by exactly
the memory pressure that caused the eviction being repaired. `-READONLY` is a second new route
if a proxy or Sentinel ever routes to a replica.

This is a real new correlated failure, and it is why D4 must not turn a failed repair into an
error: the fallback keeps an OOM'd repair from cascading into a full cache bypass at the worst
possible moment. §5 corrects the RUNBOOK paragraph that currently promises the opposite.

## 3a. Post-implementation amendments

Written after the branch was built. §3.2 as drafted said the single-flight mutex is taken with a
blocking `lock`, so "concurrent in-flight requests on this replica issue one `INCRBY`, not one
each." The shipped code (`RedisGenerations::repair`, `generation.rs`) takes it with `try_lock`
instead, and that is a correction, not a style choice.

**Why the blocking version was wrong.** A failed repair deliberately never raises the high-water
mark (D4) — that is what keeps the fallback stable across repeated failures instead of
ratcheting the delta toward the ceiling. But it also means the in-gate re-check
(`high_water >= delta`) can never short-circuit while a repair keeps failing: every waiter drains
through in turn, each one re-attempts the same doomed `INCRBY` against the same unhealthy Redis,
and none of them return until their own attempt completes. Under sustained `maxmemory` pressure
— precisely the condition most likely to make a repair fail in the first place, per §3.7 — a
blocking gate serializes every generation read on this replica behind one failing Redis round
trip at a time, capping the authz hot path at roughly one decision per RTT. That is a
per-replica throughput ceiling introduced by the fix itself, on the exact failure path the fix
exists to make safe.

**The fix.** `try_lock`, not `lock`. A caller that finds the gate already held returns the
deterministic local fallback immediately — `delta`, i.e. `high_water + REWIND_JUMP`, the same
value a failed repair itself returns — rather than queueing behind the in-flight attempt. This
mirrors `PolicySnapshot::reload_if_stale`, which `try_lock`s its own reload gate for the same
reason (`policy_snapshot.rs`). The cost is accepting more transient key-space fragmentation than
the blocking version would have produced: a call that loses the gate race may end up using a
different generation than the in-flight repair lands on. D4 already treats that shape of
fragmentation as safe — it is disjoint, not re-entrant, and self-corrects once the fleet
converges — so this is not a new risk, only a larger dose of an already-accepted one.

**What did not change.** The gate still ensures at most one in-flight `INCRBY` per counter per
process — `try_lock` prevents concurrent writers the same way `lock` would, it just declines to
queue a second one. AC2's "single-flighted per counter per process" holds under either
implementation. §3.2's prose above is left as the dated design record; this section is what
actually shipped.

## 4. Observability

New counter in `paigasus-observability`'s `names.rs`:

```
iam_authz_generation_rewinds_total{counter="policy_gen"|"entity_gen",
                                   outcome="repaired"|"repair_failed"|"ceiling",
                                   reason="missing"|"lower"}
```

All three label sets are closed and tiny — no cardinality risk. The Redis `ErrorKind` is
deliberately **not** a label, matching the SMA-470 posture that an error is never a label.
`reason` distinguishes a vanished key from a key that came back at a lower value (a stale
failover), which are different operator stories.

Wiring this metric takes **three** edits, not just the obvious one:

1. the `names.rs` const;
2. an entry in `names::ALL` — **a CI gate**: `names.rs:94-99` documents the drift test that
   extracts every `iam_`-prefixed identifier from the committed dashboards/rules and asserts
   each is in `ALL`, so a panel or rule referencing the metric fails the build without it;
3. a `describe_counter!` in `describe_iam_metrics` (`main.rs:368-399`), whose doc comment
   hard-codes "the **24** metric families `paigasus-iam` emits directly" (`main.rs:361`) and
   must become 25.

**Grafana** — a panel on the IAM dashboard (`ops/observability/grafana/dashboards/iam.json`),
alongside the existing authz panels.

**Prometheus** — `IamAuthzGenerationRewound`, **warning** severity, `sum by (counter, outcome)`
(never a bare `sum()` — see below), with an explicit `for:` duration.

The alert's diagnosis must **not** claim a rewind is near-conclusive evidence of `allkeys-*`.
Per §1.2, three of the four rewind causes destroy the caches along with the counter and are
benign; only selective eviction is hazardous, and the metric cannot tell them apart on its own.
The RUNBOOK entry therefore enumerates all four causes and gives the human triage step —
whether the `iam:authz:slice:*` / `iam:authz:dec:*` key spaces are also empty, which
distinguishes whole-Redis loss from selective eviction. A code-side probe (a sentinel key, or
`DBSIZE`) was considered and rejected: it adds a round trip and state to a rare path to
automate a judgement an operator makes reliably in one command.

**promtool fixture** (`ops/observability/prometheus/rules/tests/iam.test.yml`) — the first draft
of this design specified "a control series that must not fire", which is **unachievable**
against a bare `sum()`: with no label selector to drop, any added series folds into the same
sum and necessarily fires. What actually discriminates here:

- a **flat-at-zero evaluation window** on an existing series, which is the only thing that
  catches a `>= 0`-for-`> 0` mutant;
- an **absent-series** case pinning the memory-backend contract (the counter is never emitted
  there, and the rule must stay silent) — the same contract `iam.test.yml:283-303` already pins
  for SMA-473;
- separate `repaired` and `repair_failed` cases, which the `sum by (counter, outcome)` grouping
  makes possible and a bare `sum()` does not.

## 5. Documentation

`docs/ops/RUNBOOK-observability.md` needs **five** edits plus the new alert entry:

1. The **metric catalog** table gains the new counter.
2. The **`maxmemory-policy` mandate** paragraph states that evicting a generation key "silently
   rewinds that counter". After this change it does not — it is detected, repaired and counted.
   The mandate itself stays (it is still the right configuration, and §3.4's residue means
   detection is not a substitute for it), but the "silently" claim must go and the new signal
   must be named as the way a violation surfaces.
3. The **`IamAuthzRedisCacheBypassed` cause list** (`:869-877`) explicitly promises the
   generation read is `readonly fast` and not `denyoom`, and that an evicted counter is "a
   successful read of the wrong value, not an error". Both become wrong — see §3.7.
4. The **remediation paragraph** (`:912-919`) says "Nothing about the decision path needs repair
   afterwards" and "an `allkeys-*` policy will have been rewinding the counters silently the
   whole time". Both are superseded by the repair mechanism and the new alert.
5. The **`entity_gen` bound** paragraph ("This bound covers policy and role-grant revocation
   only") should record that the 90 s tenancy-path bound is now the *residual* exposure after
   repair, and cross-reference D1's structural reason for why content-addressing did not
   transfer.

## 6. Out of scope

- **Retrying a swallowed bump.** The *stall* half of the defect — a `bump_entity_gen` that fails
  and is swallowed (`pg_organizations.rs:45-47`, `pg_teams.rs:38-40`, `pg_projects.rs:38-40`) —
  is a different failure mode with a different fix. Not what SMA-474 describes.
- **Postgres-backed generation counters.** SMA-475 eliminates the window rather than bounding
  it, and wants an ADR first. It is also the only structural answer to §3.4's residue. This
  design is the cheap local hardening in the meantime and does not prejudge it.
- **Any change to the two cache key shapes**, to `authz.*` config, or to the memory backend.

## 7. Tests

### 7.1 Unit — the pure guard and the memory backend

`guard(observed, high_water)` needs no connection, so it is tested exhaustively in-process:
steady state (`observed > high_water`, `observed == high_water`), a fresh handle at `0/0`, a
rewind to `0`, a partial rewind to a non-zero lower value, the `saturating_add` boundary, and
the i64 ceiling. Plus: the memory backend's existing tests continue to pass unchanged, and the
guard is never reached on that path.

### 7.2 Docker-gated integration — `authz_generations_redis.rs`

This file already has the container harness and the CI-hard-fail / local-skip gating.

- **Rewind is repaired:** bump to N, `DEL` the key, then read — must return a value
  `>= JUMP`, never `0`.
- **The repair is persisted, and a *different process* observes it:** construct a second handle
  via a second `Generations::redis_connect(&url)` — **not** `gens.clone()`, which shares the
  same `Arc<AtomicU64>` marks and would prove nothing. `authz_acceptance.rs:454-470` already
  uses two independent instances for exactly this reason.
- **A rewind followed by a bump cannot re-enter a used generation:** bump to N, `DEL`, then
  `bump` — the result must exceed N, proving the guard is on the bump path too (§3.2). This is
  the test that fails if the guard is only added to `read`.
- **`policy_gen` and `entity_gen` repair independently** — repairing one must not move the
  other, matching the existing independence test.
- **A failed repair falls back locally and does not error** (D4): `CONFIG SET maxmemory 1` on
  the container makes `INCRBY` fail with OOM (`denyoom`) while `GET` keeps succeeding
  (`readonly`) — the exact asymmetry RUNBOOK:869-877 documents. Assert `read` returns `Ok` with
  a value `>= JUMP`, that `outcome="repair_failed"` is counted, and that the Redis-side value is
  unchanged. An earlier draft of this design recorded "no clean way to fail only the `INCRBY`"
  as a known gap; that was wrong, and this test replaces it.

Every new test must be **mutation-tested**: each is required to fail against the pre-fix code
before it is accepted, matching the bar SMA-470 set.

## 8. Rollout, rollback, residual risk

**Rollout** is a plain deploy — no migration, no config change, no key-shape change. Mixed
versions interoperate: an old replica has no guard, so it neither detects rewinds nor repairs,
and a repaired (large) counter value is an ordinary number it reads and `INCR`s from normally.
During the window a new replica may repair while old replicas continue writing at the rewound
value; the result is the same disjoint-key-space fragmentation D4 already accepts as safe.

**Rollback** is a plain revert, with one caveat the first draft got wrong. A counter left at a
repaired value is harmless *up to the ceiling*: at `i64::MAX` the pre-fix `INCR` also fails, so
a saturated counter cannot be fixed by reverting. §3.2's ceiling check exists to make that
unreachable, and §5's RUNBOOK entry documents the manual remediation.

**Residual risk** is §3.4's long-idle-replica case and §3.7's new `denyoom` exposure on the read
path.

## 9. Acceptance criteria

1. A rewind **below the observing process's high-water mark** is detected by `Generations` on
   the next `read` **or** `bump`, for both `policy_gen` and `entity_gen`.
2. A detected rewind is repaired forward in Redis with a single atomic `INCRBY` of
   `high_water + JUMP`, single-flighted per counter per process, so other replicas converge on
   the repaired value.
3. A repair that fails — including under `maxmemory` OOM — returns `Ok` with a locally-jumped
   value, never `Err`, and is counted as `repair_failed`. `Err` from `read` continues to mean
   only "the `GET` failed", with its existing fail-open behaviour unchanged.
4. `iam_authz_generation_rewinds_total{counter, outcome, reason}` is registered in `names::ALL`
   and `describe_iam_metrics`, panelled, and alerted on by `IamAuthzGenerationRewound` using
   `sum by (counter, outcome)` with an explicit `for:`. Its promtool fixture includes a
   flat-at-zero window and an absent-series case, not a second summed series.
5. The memory backend, both cache key shapes, and the `authz.*` config surface are unchanged.
6. The RUNBOOK's five superseded paragraphs (§5) are corrected, and D1 (why content-addressing
   does not transfer) and D2 (why `volatile-*` alone is not sufficient) are recorded as the
   decisions SMA-474 asked for.
