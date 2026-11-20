# SMA-821: a durable generation record for the decision log outbox

- Linear: SMA-821. Follow-up of SMA-755 (PR 509, PR 511).
- Date: 2026-11-20.
- Status: draft, for approval.

## 1. Intent

The gateway decision log writes each event to a Redis stream (`<prefix>:decisions`). The relay
reads the stream and publishes to the JetStream stream `GATEWAY_DECISIONS`. When Redis loses the
stream, the relay counts the incident in `gateway_decision_log_redis_resets_total`. It adds an
estimate of the lost events to `gateway_decision_log_lost_estimate_total`. The alert
`GatewayDecisionLogRedisReset` reads the reset counter.

Today the relay finds a loss only from `NOGROUP`. The SMA-755 spec (§5.8, "Known limits") lists
three cases that it does not count:

- (a) A fresh relay makes the group at its start. It cannot tell a first start from a start after a
  loss.
- (b) A relay that holds a batch settles it with `XACK`. `XACK` returns 0, not `NOGROUP`, on a
  missing group. If a new relay makes the group before the next read, no relay counts the reset.
- (c) Redis makes the group, but the reply to `XGROUP CREATE` is lost. The retry gets `BUSYGROUP`
  and counts nothing.

Also, the estimate is the last `XLEN` sample, and that sample is in the memory of one relay only. A
fresh relay has no sample.

**The goal:** a relay that starts after a Redis reset detects the reset and counts the loss. Sven
chose this follow-up on 2026-11-20 (CodeRabbit, PR 509, `relay.rs:408`).

## 2. Goals, non-goals, anti-goals

Goals:

- G1. Count each Redis reset exactly once over all relays, also in cases (a), (b) and (c).
- G2. Give the reset an estimate from the last `XLEN` sample before the loss, also when the relay
  that counts it started after the loss.
- G3. Detect a restore of Redis from a snapshot that was taken BEFORE a reset (the snapshot holds an
  older generation).
- G4 (Q2, Sven). Detect a loss of entries INSIDE one generation: a Redis restart from a snapshot of
  the current generation, or a failover to a replica that did not have the last writes (§5.3b). So
  `gateway_decision_log_redis_resets_total` now means "the Redis decision stream lost entries", not
  only "Redis lost the stream".

Non-goals:

- An exact count of the lost events. The estimate keeps its SMA-755 semantics: it is the last
  sample, not a bound. The export stays the exact source (the sequence gaps).
- An exact count for a loss inside one generation. Its estimate is the last stored sample, not a
  bound (§5.3b).
- A change to the decision path (the writer), to the event format, or to `GATEWAY_DECISIONS`.

Anti-goals (a failure even if the count works):

- A KV failure (NATS down, a missing permission, a missing bucket) stops or slows the relay or the
  writer. The generation check is best-effort and runs beside the relay. It never blocks a publish,
  an ack or a write.
- A false reset on a normal start, on a restart of one instance, on a rolling deploy, or on the
  first deploy of this change.
- Read access to the decision stream for the publisher user.

## 3. Constraints

- The record must survive `FLUSHALL` and a Redis restart without persistence. So the record of the
  last generation cannot live in Redis only.
- The NATS user `gateway-decision-publisher` has a small, audited permission set
  (`ops/nats/subjects.env`, `GW_DEC_PUBLISHER_PUB`). `repo:nats-permissions` gates it. Any new grant
  goes into that set, into `ops/nats/permissions.md`, and through the gate.
- More than one gateway instance runs. They share one Redis stream and one NATS account.
- The workspace pins `async-nats` 0.53 with `default-features = false`. Its KV API needs the `kv`
  feature, which is not on today (`rs/Cargo.toml:298`).
- Rust edition 2024, the repo's lint level (`-D warnings`), SPDX headers.

## 4. Approaches

1. **A NATS KV bucket holds the last generation and the last sample. Picked by Sven.** Redis holds
   a generation id next to the stream. A mismatch between the two is a reset. A compare-and-set
   (CAS) on the KV entry selects exactly one relay to count it. It survives a Redis loss, it closes
   (a), (b) and (c), and it detects a restore from an older snapshot. The cost is a new bucket and a
   larger publisher permission set.
2. **A Redis key only.** `FLUSHALL` and a restart without persistence delete it too. It closes only
   the case of an evicted stream key. It does not meet the goal. Rejected.
3. **A generation header on each message in `GATEWAY_DECISIONS`.** No new bucket. But to read the
   last generation at start, the publisher needs read access to the decision stream of every org.
   `subjects.env` keeps that access away from the publisher on purpose. Rejected.

## 5. Design

### 5.1 The Redis generation key

- Key: `<prefix>:decisions:gen`. It holds one random generation id (a UUID v4 string).
- The relay makes the group with ONE Lua script, which replaces `create_group`. The script:
  - runs `XGROUP CREATE <key> relay 0 MKSTREAM`.
  - On success, it writes a NEW generation id to the generation key with `SET` (overwrite). It
    returns "created".
  - On `BUSYGROUP`, it writes a generation id with `SET … NX` (only if the key is missing). It
    returns "exists".
- The new id comes from the relay as a script argument. Lua in Redis has no good random source.
- Why overwrite on create: if only the stream key is lost (an eviction, a `DEL`), the generation key
  stays. The new group must still get a new generation, or the loss is not seen.
- Why `NX` on `BUSYGROUP`: this is the upgrade from the SMA-755 version, where the group exists and
  the key does not. It is also a lost generation key alone. Both get an id, and no group change.
- The workspace `redis` crate has no cluster feature, so the gateway connects to one node and the
  two keys need no hash tag. The builder confirms this in `rs/Cargo.toml` and in
  `paigasus-redis`.
- The script uses `redis.pcall` for `XGROUP CREATE` and matches the error by its `BUSYGROUP`
  prefix. Any other error goes back to the relay as an error.
- `handle_nogroup` and the start arm of `iteration` call this script. They no longer count a reset.
  The fields `Inner::recovering` and the `count_reset` call in `handle_nogroup` go away.

### 5.2 The NATS KV record

- Bucket: `GATEWAY_DECISIONS_GEN` (its stream is `KV_GATEWAY_DECISIONS_GEN`). History 1, file
  storage, replicas 1 (the same as `stream_config`), no TTL, `allow_direct = true`, and
  `max_bytes` = 1 MiB (my call, D19). `GATEWAY_DECISIONS` takes 90 % of the account storage
  (`max_bytes_for_account`), so 10 % stays free. The builder confirms that 1 MiB fits in that rest
  for the smallest account that `provision-decisions.sh` allows, or lowers the value.
- One entry per Redis stream key. The entry key is derived from the Redis stream key, so that two
  deployments with different prefixes in one NATS account do not share an entry. The derivation is
  deterministic and collision-free over the valid key characters of NATS KV (builder's call, see
  §7).
- **A deployment rule (Q1, Sven: not supported, document it).** Two deployments with their own Redis
  must not share one NATS account AND the same Redis prefix. "Their own Redis" also means another DB
  index on the same Redis server. The default prefix is `paigasus:gateway` (`config.rs:752`), so
  two regions, or staging and production, collide with the default configuration. They would share
  one KV entry and count false resets with no end. The runbook and `ops/nats/permissions.md` state
  the rule.
- **A detector for the rule (my call, D18).** A relay remembers the Redis generation of its last
  successful KV step. If the Redis generation did not change since then, but the KV entry now holds
  a different generation, another deployment writes the entry. The relay then logs an error,
  increases `generation_check_errors_total{kind="foreign_writer"}`, writes nothing and counts no
  reset. A restore and a `FLUSHALL` change the Redis generation, so they do not trigger the
  detector.
- A failed KV step makes the relay forget its last own generation (found in the plan review). A
  relay with failing KV steps can miss a whole generation. If Redis then goes back to that older
  generation, a stale memory would show a foreign writer and hide the reset. The relay that wrote
  the newer generation always knows it, so it counts the reset.
- The value, as JSON: `{"generation": "<id>", "epoch": <u64>, "sample": <u64 or null>,
  "sampled_at": <unix ms>, "claim": <null or {"nonce": "<id>", "lost": <u64 or null>}>}`.
- A value that does not parse, and a KV delete or purge marker, are the same as a missing entry.
- async-nats 0.53 has no get-or-create for KV. The relay calls `get_key_value` first, and calls
  `create_key_value` only when the bucket is not found. A bucket that `nats kv add` made has a
  different config, so a create on it fails with 10058. The relay reads with a direct get only.
- `provision-decisions.sh` also makes the bucket with `PUSH=1`, beside `GATEWAY_DECISIONS`. The
  account allows 2 streams (`--js-streams 2`, `provision-decisions.sh:50`). `GATEWAY_DECISIONS` and
  `KV_GATEWAY_DECISIONS_GEN` use both. A third stream needs a higher limit. The spec and
  `permissions.md` state this.
- **Two violation counters (D10, corrected in round 2).** The relay's NATS event callback counts each
  "Permissions Violation for Publish" (`jetstream.rs:286`). `classify` uses that count to turn a
  publish `TimedOut` into a permanent rejection, and the relay then deletes the event. A refused KV
  call on the same connection must never do that. async-nats reports a refused KV call as
  `TimedOut` too, and the only sign of the refusal is the same `-ERR` event. So the callback splits
  the count by the quoted subject:
  - a subject that starts with `gateway.decision.` increases the publish counter (as today);
  - any other subject increases a second counter, for the KV step. The KV step classifies its own
    `TimedOut` with `violation_since` on that counter. A new violation gives `kind="permission"`,
    else `kind="timeout"`.
  - The error log of the callback is rate-limited for the second counter, so that a missing grant
    does not log on each tick.

### 5.3 Detection in the sampler

Detection moves beside the sampler loop (every `sample_every`, 15 s).

**Step 1, the gauge (each tick).** The read-only sample script runs as today (`XLEN` and the group
flag). It now also reads the generation key and the epoch key (§5.3b) with `GET`. It sets the
backlog gauge at once. It writes nothing, so a Redis at its memory limit still gives the gauge. The
relay keeps the result as its local record: one `(generation, epoch, sample, sampled_at)` tuple per
generation, for the current and the previous generation only (D13, corrected in round 2).

**Step 2, the KV step.** It runs as a single-flight task: if the KV step of an earlier tick still
runs, this tick skips it. It has one `tokio::time::timeout` (about 5 s, builder's call). So a slow
KV never delays the next tick or the gauge. The order inside the step is fixed (D17, from round 2):

1. **KV get** of the entry (with its revision).
2. **One Redis detection script**, in one atomic step: it repairs a missing generation key (below),
   runs `INCR` on the epoch key (§5.3b), and returns the generation, the new epoch, `XLEN` and the
   group flag.
3. **`decide`**, then at most one **CAS** with the revision of step 1.

Why this order: the Redis read in step 2 always comes after the KV get. Any write by another relay
between step 1 and step 3 changes the revision, so this relay's CAS fails. A stale Redis read cannot
reach the KV. No re-read rule is necessary.

**The detection script repairs a missing generation key (D7).** The relay passes a fresh id on each
call. If the group exists and the generation key does not, the script writes that id with
`SET … NX`. So a group that an SMA-755 relay made (a mixed fleet during a rolling deploy) gets a
generation within one KV step. The script uses `redis.pcall` and returns an error to the relay on a
Redis error (for example, an `INCR` refused at the memory limit). That is a KV-step error with
`kind="redis"`.

**`decide(redis_view, kv_entry, pending_claim, last_own) -> Action`** is a pure function, with a
small injected KV port (get, create, update with revision). The rules:

| Redis (step 2) | KV entry (step 1) | Action |
|---|---|---|
| no group | any | No write. Redis lost the stream and no relay made the group yet. |
| generation `R`, epoch `E` | missing | `create {R, E, xlen}`. Count nothing: a first start, or NATS lost its record. |
| `R`, `E` | `{R, e, s}`, `E > e` | `update {R, E, xlen}`. The normal case. A failed CAS is normal and is ignored. |
| `R`, `E` | `{R, e, s}`, `E <= e` | A loss inside one generation (§5.3b). Counting `update {R, E, xlen, claim}`. |
| `R`, `E` | `{K, e, s}`, `K != R`, and `R` is the generation of this relay's last successful KV step | The foreign writer of §5.2. No write, no count. |
| `R`, `E` | `{K, e, s}`, `K != R`, other cases | A reset. Counting `update {R, E, xlen, claim}`. |

- **What a write stores.** `create`, every counting write, and every `K != R` write store the values
  of step 2 as they are. No write takes a maximum or carries a value of the old generation forward.
  So after a `FLUSHALL` the new generation starts with the new epoch (1), and the next tick does not
  count a second time.
- **The claim.** A counting write puts `claim = {nonce, lost}` into the value, with a fresh nonce.
  A normal write (`E > e`) carries the stored `claim` forward unchanged. The relay stores
  `(nonce, lost)` in `State` BEFORE the CAS await, because the timeout can cancel the future and
  the error branch then never runs. On success, the relay counts at once and forgets the nonce. On
  an error or a cancel, it keeps the nonce. On the next successful KV get, it counts `lost` once if
  the entry holds that nonce, and forgets the nonce in both cases.
- **The counted sample (D13).** For a reset (`K != R`): the newer of the KV sample `s` and the
  relay's local tuple of generation `K`. During a NATS outage no relay can write the KV, but the
  local tuple of the old generation stays current until the loss. For a loss inside one generation:
  the KV sample `s` only. The local tuple of the same generation can hold the `XLEN` after the loss.
  If the sample is null, the relay adds 0 and logs that the loss is unknown, as today.
- **A KV error** (NATS down, a timeout, a permission refusal, no bucket, a Redis error in step 2)
  logs a rate-limited warning with the error kind. It increases
  `gateway_decision_log_generation_check_errors_total{kind}`. The next tick tries again. A reset
  during a NATS outage is counted when NATS is back, as long as Redis still holds the new
  generation.
- A sample of 0 under the current generation is a real, empty stream. The group lives inside the
  stream key, so a stream that lost its entries but kept its group is not possible.
- Latency: the count comes at the next KV step after the loss (up to 15 s plus the KV round trip),
  not at the group creation. The tests use `sample_every` = 200 ms (`fast()`).
- The in-memory sample (`State::sample`) of SMA-755 goes away. The local tuples replace it.

### 5.3b A loss inside one generation (G4)

Q2 (Sven): detect a loss inside one generation. The design uses an **epoch key**, not
`entries-added` (D16, changed in round 2).

- Key: `<prefix>:decisions:epoch`, an integer. Only step 2 of the KV step changes it, with `INCR`,
  and only after a successful KV get.
- In normal operation each KV step increases the epoch, so the new epoch `E` is larger than the
  stored epoch `e`. A Redis that went back to an older state (a restart from a snapshot of the
  current generation, a failover to a lagging replica) also went back to an older epoch. If that
  older epoch is not larger than the stored one, `E <= e`, and the relay counts a loss.
- Why not `entries-added` of `XINFO STREAM`: it grows with the write rate, so writes after the loss
  hide the loss. The epoch grows only with KV steps. It also needs no Redis 7.0.
- The estimate for this case is the stored KV sample `s`: the backlog at the last KV write. It is an
  estimate, not a bound, with the same meaning as the reset estimate.
- A restored snapshot also holds entries that the relays already published. The relays publish
  them again. JetStream drops a duplicate inside its dupe window, and the export removes the rest
  (SMA-755 §5.7). This is not new.
- A deleted epoch key alone gives `E = 1`, which is not larger than `e`. So it counts a false loss,
  as a deleted generation key does (§5.5).

### 5.3a Metrics and alerts

- **The alert can miss a reset on a fresh pod.** A fresh relay can count at its first tick, before
  Prometheus scrapes it the first time. Its counter series then starts at 1, and `increase()` gives
  0. The prime at 0 helps only when a scrape sees the 0. So add a gauge
  `gateway_decision_log_last_reset_timestamp_seconds` (unix time of the last reset this instance
  counted; not set until the first one). The alert `GatewayDecisionLogRedisReset` becomes
  `sum(increase(gateway_decision_log_redis_resets_total[15m])) > 0 or
  max(gateway_decision_log_last_reset_timestamp_seconds) > time() - 900`. Add a promtool test for
  a series that starts at 1.
- **The alert text.** The annotation at `gateway.rules.yml:46` says that the relay "made the
  consumer group again". A loss inside one generation makes no group. The new text says that the
  Redis decision stream lost entries. The `or` expression gives a count or a timestamp, so the
  annotation does not print `$value`.
- **A broken KV turns off all reset detection.** D3 removes the `NOGROUP` count, so a reset now
  needs a working KV. So add the counter `gateway_decision_log_generation_check_errors_total{kind}`
  (primed at 0 for each kind; `kind` is a fixed set: `timeout`, `permission`, `no_bucket`, `redis`,
  `foreign_writer`, `other`), and a warning alert `GatewayDecisionLogGenerationCheckFailing` when
  `sum(increase(…[15m])) > 0` for 15 minutes. The new names go into
  `paigasus-observability/src/names.rs`, so `repo:observability-drift` and `repo:promtool` gate
  them.
- `gateway_decision_log_redis_resets_total` now means "the Redis decision stream lost entries": a
  lost stream, or a return to an older state.

### 5.4 Behaviour per case

- **`FLUSHALL` or a restart without persistence, relays running.** The generation key is gone. The
  KV step finds no group and writes nothing. The relay gets `NOGROUP`, and the script makes the
  group and a new generation. The next KV step finds `R != K`. One relay wins the CAS and counts.
  The new entry holds epoch 1, so the next step (`E = 2 > 1`) does not count again.
- **Case (a), a fresh relay after the loss.** It runs the group script at start, which makes a new
  generation. Its KV step finds `R != K` and counts with the stored sample.
- **Case (b), `XACK` = 0.** Some relay makes the group and the generation. Any KV step counts. The
  relay that held the batch does not need to see `NOGROUP`.
- **Case (c), a lost reply.** The group and the new generation exist in Redis. Any KV step counts.
- **A restore from a snapshot taken before a reset.** Redis holds an older generation. `R != K`, so
  it is counted. The stored sample is from the generation before the restore, so the estimate means
  less here. The runbook says so.
- **A restart from a snapshot of the current generation, or a failover to a lagging replica.**
  `R == K`, and the epoch went back: `E <= e`. Counted by §5.3b.
- **`XGROUP DESTROY` only.** The script makes the group with a new generation, so it is counted, as
  today. The estimate overcounts, as today (the events are still in the stream).
- **Normal start, restart, rolling deploy.** The group exists. The script keeps the generation. Each
  KV step increases the epoch. Nothing is counted.
- **The first deploy of this change.** Redis has the group and no generation key or epoch key, and
  the KV has no entry. The detection script writes a generation with `NX` and sets the epoch to 1.
  The KV step creates the entry. Nothing is counted.
- **A mixed fleet during the rolling deploy.** An SMA-755 relay can make the group after a loss and
  write no generation key. A new relay's detection script repairs the key with a new id, finds
  `R != K`, and counts the reset. The old relay counts it too, by its own `NOGROUP` path. So during
  the deploy one incident can count twice. This ends when the last old relay stops.
- **Two deployments that share one KV entry (against the rule of §5.2).** Each relay sees the KV
  move away from its own unchanged generation. The detector of §5.2 counts `foreign_writer` errors,
  and its alert fires. A running relay counts no reset. A fresh relay has no `last_own` yet, so its
  first KV step reads the other deployment's generation as a reset and counts it. So each pod
  restart in either deployment adds one false reset.

### 5.5 Limits that stay

- NATS loses the KV entry (a lost bucket, a new NATS account). The next KV step writes a new entry
  and counts nothing. A Redis loss in the same window is not counted.
- Somebody deletes only the generation key or only the epoch key. The relay counts a false reset or
  a false loss. The runbook already requires `maxmemory-policy noeviction`, so an eviction does not
  cause this. An operator `XSETID` or `RESTORE … REPLACE` on the stream key alone keeps both keys,
  so it is not detected.
- **A loss inside one generation is missed when Redis goes back to a state that is newer than the
  last successful KV write.** Example: the last KV write stored epoch 10, Redis saved a snapshot
  with epoch 10 later, and then Redis restarted from that snapshot. The next step gives `E = 11 >
  10`. In normal operation the window is about one `sample_every` (15 s). During a NATS outage no KV
  step runs, so the window lasts for the whole outage. The export shows the gaps.
- The estimates are the last stored sample, with about one `sample_every` of error.
- During the rolling deploy from SMA-755, one incident can count twice (§5.4).
- A relay that made an ambiguous counting write and stops before its next KV get loses that count.
- A relay that only loses CAS races keeps a stale `last_own`: only a successful write sets it, and a lost
  CAS does not clear it. After a later restore to that generation, this relay can report a false
  `foreign_writer` error. Another relay still counts the restore.
- The deployment rule of §5.2 (Q1) is only detected, not prevented.
- A restore to an older generation is missed when the relay that wrote the newer generation has
  stopped and every other relay saw no KV step fail during that generation. Those relays report a
  `foreign_writer` error, so the alert of §5.3a still fires.
- A Redis loss and a return to a generation that the KV holds again are not possible: each new group
  gets a new random id.

### 5.6 NATS permissions

The publisher needs grants for the KV bucket. The exact subjects depend on what async-nats 0.53
sends. The builder measures them against a live server and adds only those. The expected set is:

- `$JS.API.STREAM.INFO.KV_GATEWAY_DECISIONS_GEN` and `$JS.API.STREAM.CREATE.KV_GATEWAY_DECISIONS_GEN`
  (get or create the bucket).
- A get of one entry: `$JS.API.DIRECT.GET.KV_GATEWAY_DECISIONS_GEN.>` (direct get).
- `$KV.GATEWAY_DECISIONS_GEN.>` to put, create and update.

The direct get is the only read path (`allow_direct = true`). There is no `STREAM.MSG.GET`
alternative.

These grants touch only the new bucket. They give no read, delete or purge right on
`GATEWAY_DECISIONS`. `ops/nats/permissions.md` states the change and the reason.

**Existing users.** `provision-decisions.sh:64` runs `nsc add user … || echo "already exists"`, so a
second run does not change the grants of an existing user. The script gets an `nsc edit user` step
that sets the publisher grants on an existing user. The runbook states the order of the upgrade:
the new grants and the bucket first, then the new gateway binary. A gateway without the grants
still relays (the two violation counters of §5.2), and the new error counter and its alert show the missing
grant.

## 6. Tests

Extend `rs/crates/services/paigasus-gateway/tests/decision_log_redis.rs` (Redis and NATS
containers). Run every Redis command of a test through `redis-cli` in the container, as the
existing tests do (`repo:redis-connect-single-site`).

**"Exactly 1" means: wait for the count, then let at least 3 more KV steps run, then assert 1.**
This catches a second count on the tick after the first (round-2 BLOCKER). It applies to T1, T2,
T5, T8, T9, T13 and T15.

- T1. A fresh relay after `FLUSHALL` counts one reset with the stored sample. A relay runs and
  samples. Stop it. `FLUSHALL`. Start a NEW relay with a new consumer name and no memory. Expect
  exactly 1 reset and the old sample in the estimate.
- T2. `FLUSHALL` with two relays counts exactly one reset (the existing test, through the new path).
- T3. A restart with no loss counts nothing. Stop and start a relay. Expect 0 resets.
- T4. The upgrade: Redis has the group and no generation key or epoch key, and the KV is empty.
  Expect 0 resets, then a generation key, an epoch key and a KV entry.
- T5. A restore from a snapshot taken before a reset: set the generation key back to an older id.
  Expect exactly 1 reset.
- T6. NATS down, then back: the KV step counts errors, and a reset during the outage is counted
  after NATS returns.
- T6b. The anti-goal: a publisher with the SMA-755 grants only (no KV grants). Expect the relay to
  publish every event, `publish_rejected` = 0 also when a publish times out, the publish violation
  counter at 0, and `generation_check_errors_total{kind="permission"}` > 0. The existing test
  `the_relay_sizes_the_stream_to_a_smaller_account_limit` (`v == 0`) must still pass.
- T7. A failed first group create, then a create. The existing test
  `a_group_made_after_a_failed_first_create_counts_one_reset` uses a NATS URL with no server, so it
  cannot reach the KV. Rewrite it with a live NATS.
- T7b. An ambiguous counting write (unit, through `decide` and the port). Two cases: the port's
  `update` applies and then returns a timeout; and the timeout cancels the future after the update
  applied. Expect exactly 1 reset, on the next KV get, in both cases. Also: a pending nonce that the
  next successful get does not hold is dropped and counts nothing.
- T8. Case (b): a relay holds a batch while a second relay makes the group after `FLUSHALL`. Expect
  exactly 1 reset.
- T9. The repair: make the group with plain `XGROUP CREATE` (as an SMA-755 relay does) and no
  generation key, with a KV entry from before. Expect a generation key within one KV step and
  exactly 1 reset.
- T10. A concurrent write (unit, through `decide` and the port): another write changes the revision
  between the get and the CAS. Expect a failed CAS, no count, and a correct result on the next step.
- T11. The gauge with a slow KV: the KV step hangs. Expect the backlog gauge to update on each tick.
- T12. The promtool test of §5.3a: a reset series that starts at 1 fires the alert.
- T13. A loss inside one generation (integration). Copy the stream key, the generation key and the
  epoch key into side keys with `DUMP` (one `EVAL`). Let at least 3 KV steps run, so that the stored
  epoch grows. Then put all three keys back with `RESTORE … REPLACE` (one `EVAL`, so it is atomic).
  This keeps the group and the generation. Expect exactly 1 reset and the stored sample as the
  estimate. (A container restart does not work: the default save points save on SIGTERM, so the
  restart loses nothing, and the mapped port can change.)
- T14. The epoch rules (unit, through `decide`): `E > e` writes and counts nothing; `E <= e` counts;
  a `K != R` write stores the epoch of step 2 as it is (no maximum), so the next step does not count.
- T15. The foreign writer: write a KV entry with another generation while this relay's Redis
  generation does not change. Expect `kind="foreign_writer"` > 0 and 0 resets.

Each new assertion must be shown to fail when the feature is removed (delete the counting branch of
`decide` and see T1, T5, T8 and T13 go red). The permission change must pass
`repo:nats-permissions`.

## 7. Left to the builder

- The KV entry key derivation from the Redis stream key (deterministic, collision-free, valid NATS
  KV key characters). Q1 is answered: no deployment id.
- The exact NATS subjects (§5.6), measured.
- Put the `kv` feature on the gateway's `async-nats` dependency line, not on the workspace line.
- The JSON field names and the serde type of the value.
- How the KV step keeps the KV store handle. It must make the handle again after a failure (D9).
- Rate-limiting of the KV warning log, so that a long NATS outage does not flood the log.

## 8. Documents to update

- The SMA-755 spec §5.8 "Known limits": the reset count is no longer best-effort for cases (a), (b)
  and (c). Add the limits of §5.5. Point to this spec.
- `docs/ops/RUNBOOK-observability.md`: the metric rows for `redis_resets_total` (its new meaning)
  and `lost_estimate_total`; the `GatewayDecisionLogRedisReset` entry (it also fires on a return to
  an older state, and up to one sample period after the loss); a new entry for
  `GatewayDecisionLogGenerationCheckFailing` with each `kind`; the deployment rule of §5.2 and the
  `foreign_writer` sign; the generation key and the epoch key with `noeviction`.
- `ops/nats/permissions.md` and `ops/nats/README.md` (the new bucket, the stream limit, the
  upgrade order).
- `ops/observability/prometheus/rules/gateway.rules.yml` and its promtool tests (§5.3a).
- `paigasus-observability/src/names.rs` and the metric rows of the runbook (the two new metrics).
- `ops/nats/provision-decisions.sh`: the bucket and the `nsc edit user` step.
- The module doc comment of `relay.rs`.

## 9. Decisions

| Id | Decision | Who |
|---|---|---|
| D1 | A NATS KV bucket holds the record (approach 1). | Sven, 2026-11-20 |
| D2 | The publisher permission set grows by the KV grants of §5.6. | Sven, 2026-11-20 |
| D3 | Detection moves into the sampler. `NOGROUP` only makes the group again; it no longer counts. | my call, Sven agreed |
| D4 | The KV entry holds the last sample, so a fresh relay can give the estimate. | my call, Sven agreed |
| D5 | A new generation on every group creation (overwrite); `NX` on `BUSYGROUP`. | my call |
| D6 | REVERSED after the challenge: a counter for KV errors and an alert on it (§5.3a). | my call, after the challenge |
| D7 | The sample script repairs a missing generation key with `NX` when the group exists. | my call, after the builder check |
| D8 | The gauge is set before the KV step; the KV step has a timeout. (Its re-read rule is replaced by D17.) | my call, after the builder check |
| D9 | The sampler makes the KV store handle again after a failure (lazily), so that T6 can pass. | my call, after the builder check |
| D10 | Two violation counters: `gateway.decision.` subjects for the publish path, all other subjects for the KV step (§5.2). | my call, after the challenge (corrected in round 2) |
| D11 | A `claim` nonce in the KV value recovers an ambiguous counting write. | my call, after the challenge |
| D12 | A last-reset timestamp gauge, and the alert also reads it. | my call, after the challenge |
| D13 | For a reset, the estimate is the newer of the KV sample and the local tuple of the old generation. For a loss inside one generation, the KV sample only. | my call, after the challenge (corrected in round 2) |
| D14 | `provision-decisions.sh` edits the grants of an existing publisher user; grants before binary. | my call, after the challenge |
| D15 | Q1: two deployments with their own Redis must not share a NATS account and a Redis prefix. Documented, not checked. | Sven, 2026-11-20 |
| D16 | Q2: detect a loss inside one generation. Changed in round 2 from `entries-added` to an epoch key that each KV step increments (§5.3b). The estimate is the stored sample. | Sven (the check), my call (the mechanism, after the challenge) |
| D17 | The KV step order is KV get, one Redis detection script, then CAS. This replaces the re-read rules of D8. | my call, after the challenge (round 2) |
| D18 | A `foreign_writer` detector for the deployment rule of D15. | my call, after the challenge (round 2) |
| D19 | The bucket has `max_bytes` = 1 MiB. | my call, after the challenge (round 2) |
