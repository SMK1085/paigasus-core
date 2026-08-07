# SMA-485 — `api_keys.introspect_cache.redis_url` is required by `validate()` and then ignored

**Status:** design (revised after adversarial review)
**Date:** 2026-08-07
**Issue:** [SMA-485](https://linear.app/smaschek/issue/SMA-485/iam-api-keysintrospect-cacheredis-url-is-required-by-validate-and-then)
**Project:** Paigasus IAM — Hardening
**Follows:** [SMA-476](https://linear.app/smaschek/issue/SMA-476/iam-circuit-break-a-known-down-redis-a-blackholed-backend-still-costs) (PR 109), whose design doc §1.3 recorded this behaviour as "inherited rather than changed"

All line references verified against `origin/main` at `dcd69ef`.

## 1. Problem

`AppState::new` picks the API-key introspection cache's Redis connection like this
(`rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:565-576`):

```rust
let conn = match &redis_conn {
    Some(conn) => conn.clone(),
    None => {
        let redis_url = cfg.api_keys.introspect_cache.redis_url.as_deref()
            .ok_or_else(|| AuthnError::Backend("… without redis_url …".into()))?;
        connect_redis(redis_url, RedisRole::ApiKeys).await?
    }
};
```

The `Some` arm never reads `api_keys.introspect_cache.redis_url`. Whenever
`authz.cache.backend = "redis"` — the ordinary posture — the API-key cache is wired to the
*authz* Redis regardless of what the operator configured for it.

The config layer sharpens this rather than softening it. `IamConfig::validate`
(`src/config.rs:761-762`) **requires** the URL:

```
api_keys.introspect_cache.backend = "redis" requires api_keys.introspect_cache.redis_url
```

An operator who wants a Redis-backed API-key cache is therefore forced to supply a URL, and
that URL is discarded in exactly the configuration where they were most likely to point it
somewhere deliberate. The config does not merely permit a lie; it demands one.

### 1.1 Why it matters more since SMA-476

The reuse was a deliberate optimisation (SMA-444 Task 21 / SMA-445 Task 19): one connection
instead of four for the single-Redis deployment. That reasoning is sound *when both URLs name
the same endpoint*. It was never conditioned on that being true.

1. **Data path.** Every `get`/`put`/`evict` for API-key validation goes to the authz Redis.
   Self-consistent, so no stale-read bug — but an operator who separated the two backends for
   capacity or blast-radius reasons has not actually separated them.
2. **The configuration is silently inert.** Nothing warns, nothing logs. The only symptom is
   keys appearing in a Redis the operator did not expect.
3. **SMA-476's isolation guarantee does not hold here.** SMA-476 D1 scopes one breaker per
   *connection* specifically so a down backend cannot short-circuit traffic to a healthy one.
   Today, an operator who configures two distinct URLs still gets one connection, hence one
   breaker, so an authz-Redis outage also short-circuits API-key introspection against a Redis
   that is perfectly healthy. SMA-476 D10 documented the metric half of this; the availability
   half was not called out.

### 1.2 The shape of the fix

`redis_conn::connect` attaches a fresh `Breaker::new(role)` to each `RedisHandle`
(`src/adapters/redis_conn.rs:125-129`), and that breaker sets
`iam_redis_breaker_state{role}` at construction (`redis_conn.rs:334`). So "which connection"
and "which breaker" and "which metric series" are one and the same question: choosing the
connection per *endpoint* rather than per *authz-backend* is what restores D1's guarantee, and
it is observable from the outside.

### 1.3 There is no coverage here at all today

`grep -rn introspect_cache rs/crates/services/paigasus-iam/tests/` returns nothing. Every
Redis-backed `AppState::new` boot in the suite (e.g. `tests/authz_acceptance.rs:465,552,636`)
leaves `api_keys.introspect_cache` on `ApiKeyCacheBackend::Memory`, because
`support::test_config` inherits `ApiKeyDefaults::default()` (`src/config.rs:520-524`). The
entire `ApiKeyCacheBackend::Redis` arm of the composition root — both the buggy shared path and
the memory-authz path SMA-476 D10's caveat is *about* — is unexecuted by any test. §4 therefore
builds this coverage from zero rather than extending something.

## 2. Decisions

### D1 — Endpoint identity is exact string equality, after trimming

Two configured URLs share one connection when, and only when, `authz_url.trim() ==
api_key_url.trim()`.

**Why trim.** `IamConfig::validate` trims and rejects padding for `authn.issuers`
(`src/config.rs:673-679`) but does nothing of the sort for any `redis_url`, and a trailing
newline is what an env-var override or a heredoc'd secret produces. That newline does not fail
the dial: URL parsing strips leading and trailing C0 controls and spaces, so
`redis://cache:6379\n` connects fine — it would only differ *textually*, silently splitting a
deployment the operator believes is unified and minting an `iam_redis_breaker_state{role=
"api_keys"}` series that reads as deliberate separation. Trimming costs one call and no parse.

**Why nothing more than trim.** This is deliberately not endpoint equality.
`redis://cache:6379` and `redis://cache:6379/0` name the same backend and will get two
connections; so will two URLs differing only in credentials. Accepted: erring toward a second
connection is wasteful, never wrong. The key namespaces are disjoint (`iam:apikey:` at
`src/adapters/api_keys/cache.rs:49` vs `iam:authz:dec:` / `iam:authz:slice:` /
`iam:authz:policy_gen` vs `iam:jwks:`), so two handles onto one physical Redis cannot corrupt
each other, and the RUNBOOK already documents that two roles may front the same physical Redis
with independent breakers.

Normalising through redis-rs (`Client::open(u)?.get_connection_info()`, then comparing
`ConnectionAddr` + db + username) was considered and declined on two grounds:

- **It does not solve the motivating case.** The example that makes normalisation sound
  necessary — `redis://localhost:6379` vs `redis://127.0.0.1:6379/0` — differs by *host*, and
  no parser resolves a hostname to an address at config-read time. Normalisation would buy only
  the default-port and default-db spellings.
- **It puts credential comparison in the composition root.** `ConnectionInfo` carries
  `password`. Comparing two of them means deciding whether two URLs that differ only in
  credentials are "the same endpoint" — a question with no good answer that nothing is asking.
  Under trim-only comparison the answer falls out: they are not, so a partially-rotated
  credential yields two connections and boots, rather than failing.

**Named residual.** After trimming, an accidental split still requires a *semantic* alias
(`localhost` vs `127.0.0.1`, an explicit `/0`, a differing password). The symptom is a
`role="api_keys"` breaker series that the operator did not intend; §5 makes the RUNBOOK say so
next to the existing "two roles may front the same physical Redis" caveat, which is the same
observation from the other side.

### D2 — A missing `redis_url` is a wiring defect, not an inheritance signal

The URL is read up front and its absence is an error, using the same wording the authz and JWKS
arms already use ("… without redis_url (`IamConfig::validate` must run first)").

Today such a config boots when authz Redis exists, because the `Some` arm masks the missing
URL. After this change it fails. That is a behaviour change only for configs `validate()`
already rejects — `main.rs:22` calls `validate()` before `main.rs:60` constructs the state — and
per §1.3 no test configures a Redis introspect cache at all, so no existing suite is affected.

The alternative — treat `None` as "inherit the authz endpoint" — was declined. It would make
the URL genuinely optional, which is a coherent design, but it requires relaxing `validate()`
too (otherwise the lenient arm is unreachable through any accepted config) and introduces a
config-inheritance semantic the issue does not ask for. Fixing the contradiction by honouring
the URL is the smaller change and the one the acceptance criteria describe.

### D3 — The comparison is a named `pub(crate)` function, the wiring stays inline

`shares_one_connection(authz_url: &str, api_key_url: &str) -> bool` lives beside `connect_redis`
in `http/mod.rs`, with unit tests in that file's existing `#[cfg(test)] mod tests`
(`http/mod.rs:841`). The `match` that uses it stays inline in the `ApiKeyCacheBackend::Redis`
arm, carrying the D1/D2 rationale in the comment block that convention puts there — the existing
comment on that arm becomes wrong with this change and is rewritten regardless.

An earlier draft kept the comparison inline as a bare `==` on the grounds that every wiring
decision in `AppState::new` is explained next to the code it governs. That is still true of the
*wiring*; it is not a reason to leave the *policy* untestable. D1 enumerates concrete spellings
it accepts (`/0`, host aliases, differing credentials) and one it rejects (surrounding
whitespace), and every one of those is unreachable from a test unless the predicate is callable
on its own: `AppState::new` needs Postgres and a live Redis, and `RedisApiKeyCache::
from_connection` is `pub(crate)` (`src/adapters/api_keys/cache.rs:224`, SMA-476 D13), so there
is no cheaper observation point. The name is `shares_one_connection`, not `same_endpoint`,
because D1's whole point is that it decides connection sharing and does *not* claim endpoint
identity.

### D4 — No boot log line

Considered, because §1's second consequence is that the configuration is *silent*. Declined: the
silence complaint is that a configured value has no effect, and honouring the value is what
fixes it. A log line describing which of two postures was chosen adds boot noise without adding
a capability — the same fact is already exposed as `iam_redis_breaker_state{role="api_keys"}`,
which is queryable, alertable, and per-replica, where a log line is none of those.

### D5 — The authz handle carries the URL it was opened with

The binding changes from `Option<RedisHandle>` to `Option<(RedisHandle, &str)>`:

```rust
let (gens, redis_conn): (Generations, Option<(RedisHandle, &str)>) = match authz_cfg.cache.backend { … };
```

The naive fix compares `api_key_url` against a *second, independent* read of
`authz_cfg.cache.redis_url`, and nothing enforces that this read agrees with the one at
`http/mod.rs:317-322` that actually opened the handle. It also admits an impossible state —
`(Some(handle), None)` — which would fall through to "dial a second connection" silently, where
every sibling arm in this file treats an impossible config state as a loud wiring defect
(`:321`, `:573`, `:655`). Pairing the URL with the handle makes the comparison structurally be
against that handle's own origin and deletes the impossible state instead of handling it.

Cost: three destructuring sites (`http/mod.rs:396`, `:401`, and the API-key arm) become
`Some((conn, _))`.

### D6 — At most one connection per `RedisRole`

`iam_redis_breaker_state` is described everywhere as per-*connection* (`names.rs:62`, RUNBOOK
`:96`, `:1374`), but it is keyed by `role` — two breakers with the same role would have the
later construction silently overwrite the earlier one's gauge (`redis_conn.rs:334`). Today the
composition root opens at most one connection per role and this change preserves that: the
API-key arm dials at most once, and only with `RedisRole::ApiKeys`. Stated here because D1's
observability rests on it, and because it is the invariant a future fourth cache would break.

## 3. The fix

`src/adapters/http/mod.rs`, the `ApiKeyCacheBackend::Redis` arm (currently lines 557-577):

```rust
ApiKeyCacheBackend::Redis => {
    let api_key_url = cfg
        .api_keys
        .introspect_cache
        .redis_url
        .as_deref()
        .ok_or_else(|| AuthnError::Backend("api_keys.introspect_cache.backend = \"redis\" without redis_url (IamConfig::validate must run first)".into()))?;
    let conn = match &redis_conn {
        Some((conn, authz_url)) if shares_one_connection(authz_url, api_key_url) => conn.clone(),
        _ => connect_redis(api_key_url, RedisRole::ApiKeys).await?,
    };
    Arc::new(RedisApiKeyCache::from_connection(conn, cfg.api_keys.introspect_cache.ttl_secs))
}
```

with, beside `connect_redis`:

```rust
/// Whether the API-key introspect cache may reuse the authz connection: textual equality after
/// trimming, NOT endpoint identity (SMA-485 D1) — `…:6379` and `…:6379/0` are one backend spelled
/// two ways and deliberately get two connections.
pub(crate) fn shares_one_connection(authz_url: &str, api_key_url: &str) -> bool {
    authz_url.trim() == api_key_url.trim()
}
```

The resulting behaviour, exhaustively:

| `authz.cache.backend` | `authz.cache.redis_url` | `api_keys…redis_url` | Connection | Breaker role |
|---|---|---|---|---|
| `memory` | — | `redis://a` | own | `api_keys` |
| `redis` | `redis://a` | `redis://a` | shared with authz | `authz` only |
| `redis` | `redis://a` | `redis://b` | own | `api_keys` |
| `redis` | `redis://a` | *(none)* | — | boot error (D2) |
| `memory` | — | *(none)* | — | boot error (unchanged) |

Row 2 is SMA-444 Task 21's optimisation, preserved. Row 3 is the bug. Row 1 is unchanged in
behaviour but, per §1.3, untested until now.

The authz connection is still opened once and still shared by `Generations`, `SliceCache` and
`RedisDecisionCache`. Only the API-key cache's selection changes, plus D5's binding shape.

## 4. Tests

### 4.1 `shares_one_connection` — unit, Docker-free (`http/mod.rs` `mod tests`)

One table-driven test pinning every spelling D1 names, so the accepted costs and the one
rejected case are executable rather than prose:

| `authz_url` | `api_key_url` | shares | Which D1 clause |
|---|---|---|---|
| `redis://a:6379` | `redis://a:6379` | yes | the optimisation |
| `redis://a:6379` | `redis://a:6379\n` | yes | trim |
| ` redis://a:6379 ` | `redis://a:6379` | yes | trim |
| `redis://a:6379` | `redis://a:6379/0` | no | accepted cost |
| `redis://localhost:6379` | `redis://127.0.0.1:6379` | no | accepted cost |
| `redis://:pw1@a:6379` | `redis://:pw2@a:6379` | no | credentials differ |
| `redis://a:6379` | `redis://b:6379` | no | the genuine split |

### 4.2 Composition root — new `tests/api_key_cache_connection.rs`, Docker-gated

Same CI-hard-fail / local-skip gating every other Redis suite uses
(`tests/authz_cache_redis.rs`, `tests/redis_jwks_cache.rs`): a missing Docker daemon panics when
`CI` is set and returns early otherwise.

**One test function, four ordered phases, one Postgres container and one Redis container.** Two
reasons, neither of which is the `OnceLock`: container reuse (four `AppState::new` boots against
one pair of containers, not four pairs), and correctness under a plain `cargo test`, where the
whole file shares one process and one metrics registry. Under `cargo nextest run` — what
`.moon/tasks/rust.yml:23` actually runs — each test is its own process, so the ordering is
belt-and-braces rather than load-bearing. It is kept because the file must be correct under both
runners.

**`paigasus_observability::init("test-iam-api-key-cache-conn")` is the first statement of the
test**, before any `AppState::new`. `metrics::gauge!` against the not-yet-installed global
recorder is silently dropped (`rs/crates/libs/paigasus-observability/src/lib.rs:28-40`), so the
`tests/metrics.rs:29-31` ordering — build the app, *then* `init` — would make every negative
assertion below pass vacuously. This is the single most important sentence in §4.

| Phase | Config | Assertion | AC |
|---|---|---|---|
| a | authz `redis://127.0.0.1:{port}`, api_keys the same string | `Ok`; exposition **contains** `iam_redis_breaker_state{role="authz"}` and does **not** contain `…{role="api_keys"}` | 2 |
| b | authz `…:{port}`, api_keys `…:{port}/1` | `Ok`; exposition **contains** `…{role="api_keys"}` | 1 |
| c | authz `…:{port}`, api_keys `redis://127.0.0.1:1` | `AppState::new` is `Err` | 1, 3 |
| d | authz `…:{port}`, api_keys `backend = "redis"` with `redis_url = None` | `AppState::new` is `Err` | D2 |

Notes on each:

- **(a) carries a positive control.** Asserting only the *absence* of `role="api_keys"` is an
  assertion that cannot fail — a dead recorder, a renamed metric or a misspelled label all read
  as "absent". The paired `role="authz"` presence assertion is what makes the absence mean
  something. This mirrors the `redis-connect-single-site` gate's own non-empty `expected` check
  (`moon.yml:207-210`) and the promtool gate's control-series rule.
- **(a) and (b) use one container.** `…/1` selects logical database 1 on the same server: a
  different URL string, so D1 splits it, and a reachable endpoint, so the dial succeeds. Stock
  Redis ships `databases 16`, which the testcontainers module does not override; a `SELECT 1`
  failure would surface as a connect error and red phase (b)'s `Ok` assertion rather than pass
  silently. `…/0` was considered instead and rejected: it is the same endpoint spelled twice,
  i.e. D1's *accepted-cost* case, where AC1 is about a genuinely different backend — and §4.1
  already pins the `/0` spelling for free.
- **(c) is the regression proof.** `redis_conn::connect` is eager, so `main` today boots happily
  against a bogus `api_keys` URL (it never dials it) while the fixed code refuses to start. This
  is the only phase that *cannot* pass before the fix. `redis://127.0.0.1:1` follows the crate's
  own precedent (`redis_conn.rs:568,596,891,913`): unbindable by an unprivileged process, so
  deterministically refused, and not racy against testcontainers' port mapping the way
  bind-ephemeral-then-drop would be.
- **(c) is safe after (b)** because `connect` propagates the dial failure with `?` **before**
  `Breaker::new(role)` runs (`redis_conn.rs:125-129`), so a failed dial registers no gauge and
  cannot retroactively invalidate (a).
- **`AppState` is not `Debug`** (`http/mod.rs:149-150` derives `Clone` only), so `.unwrap_err()` /
  `.expect_err()` will not compile — the same trap SMA-476 documented at `redis_conn.rs:258-260`.
  Phases (c) and (d) use `assert!(result.is_err(), …)`.
- **(c) and (d) discriminate.** "Returns `Err`" alone would also pass for a Postgres hiccup or a
  pepper failure. Each asserts on the rendered message: (d) must mention `IamConfig::validate`
  (D2's wiring-defect text), (c) must **not** — (c) is a dial failure, and pinning it more
  tightly than that would couple the test to an OS-specific `Connection refused` string. Phases
  (a) and (b) having already booted successfully against the same containers is what rules out
  the environmental explanations.
- **`AppState::new` runs four times against one Postgres.** Boot reconciliation
  (`bootstrap::reconcile_starter`) is converge-to-code and idempotent since SMA-477, so repeated
  boots against the same database are exactly what production does on restart.

`support::test_config(&idp)` is memory-backed on every cache; the test mutates `authz.cache` and
`api_keys.introspect_cache` per phase.

### 4.3 Why the test observes a metric rather than the data path

A test that proved *traffic* reaches the API-key Redis — issue a key, authenticate, then check
which logical database the `iam:apikey:*` keys landed in — would have to open its own Redis
client, and `repo:redis-connect-single-site` bans `.get_connection` and
`.get_multiplexed_async_connection` in `tests/` as well as `src/` (`moon.yml:202-203`,
`:213-215`). The gate is the reason for the metric-as-proxy design, not an afterthought.

The residual is real and accepted: these phases prove *a second connection with role
`api_keys` was opened from the configured URL*, not *that cache traffic flows through it*. A
refactor that dialled the URL and then discarded the handle would still pass. What closes most
of that gap is §3's shape — the dialled handle is the sole argument to
`RedisApiKeyCache::from_connection` on the next line — and §4.1 pinning the predicate that
selects it.

### 4.4 Row 1 stays untested at the composition root

Phase (b) covers "authz on Redis, api_keys elsewhere". The memory-authz shape (§3 row 1) is
untouched by this change and, per §1.3, has never had coverage; adding it would be a fifth phase
and a second config permutation for behaviour this issue does not modify. Recorded here as a
known gap rather than left implicit.

## 5. Documentation

The rule "the API-key cache reuses the authz connection unless authz is memory-backed" is
written down in nine places. It becomes "… unless the two `redis_url`s differ (textually, after
trimming)".

### 5.1 Operator-facing, and wrong the moment this ships

- **`rs/crates/services/paigasus-iam/src/main.rs:386`** — the `describe_gauge!` help text for
  `iam_redis_breaker_state`, which is served as `# HELP` in **every `/metrics` scrape**:
  `role="api_keys" only exists when authz.cache.backend="memory" and
  api_keys.introspect_cache.backend="redis"; …`. Leaving this is worse than the original bug: an
  on-call reading it would conclude an `api_keys` series is impossible in their authz-on-Redis
  deployment, which is precisely the deployment where it will now appear.
- **`rs/crates/libs/paigasus-observability/src/names.rs:69-73`** — the canonical doc comment on
  the metric name, carrying the same claim as its first attribution caveat.

`names.rs` is inside `repo:observability-drift`'s inputs (`moon.yml:119`), so these are not
"prose-only edits with no gate interaction": the drift suite must be run. It asserts that
dashboards and rules reference registered metric *families*, and no family is added or renamed
here, so it is expected to stay green — but that is a result to verify, not to assume.

### 5.2 RUNBOOK (`docs/ops/RUNBOOK-observability.md`)

Two sites are substantively wrong:

- **`:1447-1450`** — the SMA-476 D10 attribution caveat AC5 names. The trigger condition gets
  restated, plus D1's consequence: because the comparison is textual, two spellings of one
  endpoint now produce an `api_keys` series fronting the same physical Redis — which is exactly
  what the *next* bullet in that same list already warns about, so the two are cross-referenced.
- **`:1556-1561`** — "It reuses the `redis_conn` handle **only when `authz.cache.backend =
  "redis"`**; with the authz cache on `memory` it dials its own …". This states the old rule
  outright. It is also the paragraph an operator reads when reasoning about blast radius on the
  hottest path, so it gets the fullest rewrite: reuse is now conditioned on the URLs matching,
  and the split configuration is reachable with authz on Redis.

Three sites carry the phrase in passing and are made consistent: `:96` (metric table), `:1083`
(`IamRedisBreakerOpen` meaning), `:1375` (breaker overview).

One site is **new** — a remediation entry near `:1462`, where the existing Redis boot-failure
narrative (including the crash-loop-without-backoff warning at `:1470-1478`) already lives:

> **Symptom.** IAM refuses to boot with an error naming `api_keys.introspect_cache`, on a
> deployment that started fine yesterday.
> **Cause.** Since SMA-485 that URL is actually dialled. It was previously ignored whenever
> `authz.cache.backend = "redis"`, so a wrong or unreachable value was harmless.
> **Remediation.** Fix the endpoint, or — to restore the previous behaviour exactly — set
> `api_keys.introspect_cache.redis_url` byte-identical to `authz.cache.redis_url`.

Nobody infers that last line from the error string, and §7's "revert the deploy" is the slower
answer.

### 5.3 In-crate docs that state the old rule

- **`src/adapters/api_keys/cache.rs:214-218`** — "`AppState::new` shares ONE redis connection
  across the redis-backed `Generations` + `RedisDecisionCache` + `SliceCache` + this cache".
- **`src/adapters/http/mod.rs:713-718`** — "… when it can't reuse the already-open `redis_conn`
  LOCAL BINDING in `AppState::new`", and **`:722-723`** — "see SMA-476 D10 for why a shared
  connection reports as `authz` even when it also serves the API-key cache".
- **`src/adapters/http/mod.rs:557-564`** — the comment on the arm being changed (rewritten as
  part of §3, not as a separate edit).

### 5.4 Not edited

Historical spec and plan documents, including
`docs/superpowers/plans/2026-08-06-sma-476-redis-circuit-breaker.md:124`, which quotes the
`describe_gauge!` string §5.1 changes. Those are records of what was decided then; this document's
header names SMA-476 as the work it follows, and that is where a reader picks up the thread.

## 6. Out of scope

- **`authn.jwks_cache` keeps dialling its own connection unconditionally.** It never shared, so
  it has no equivalent bug, and making it share-if-identical would *reduce* isolation on the one
  fail-closed path. Its asymmetry with `api_keys` is now deliberate and documented rather than
  accidental.
- **`IamConfig::validate` is untouched** — including the absence of a trim/whitespace rule on
  any `redis_url`. D1 handles whitespace at the comparison, which is where it matters here;
  normalising the stored config is a broader change touching four URL fields.
- **Normalised endpoint comparison.** See D1. If a future issue wants it, the change is local to
  `shares_one_connection` and its table test.
- **Composition-root coverage for §3 row 1.** See §4.4.

## 7. Rollout and residual risk

No migration, no config change required, no new configuration surface. A deployment whose two
URLs are identical strings sees no change at all.

A deployment whose URLs differ gains, at boot, a second Redis connection — which is the point —
and with it:

- **A new failure mode.** A previously ignored URL is now dialled eagerly, so a wrong or
  unreachable `api_keys.introspect_cache.redis_url` that used to be harmless now fails boot.
  This is the intended reading of D2 and the one operational surprise in the change; it belongs
  in the PR description and, per §5.2, in the RUNBOOK.
- **A late failure, at that.** The dial sits at `http/mod.rs:558-578`, *after*
  `bootstrap::reconcile_starter` (`:384`) and `PolicySnapshot::new` (`:386`), so each crash-loop
  attempt pays a full Postgres reconcile and Cedar compile before failing — on top of the
  no-backoff crash-loop hazard the RUNBOOK already warns about at `:1470-1478`.
- **Double the connections per replica.** Every replica in a textually-split deployment holds
  two `ConnectionManager`s where it held one, charged against the backend's `maxclients`.
  Negligible at normal replica counts; named because this is the section for naming it.
- **A new `iam_redis_breaker_state{role="api_keys"}` series**, and the alerts keyed off it
  (`IamRedisBreakerOpen`, `IamRedisBreakerFlapping`) now have a second series to fire on.

One thing to confirm during implementation rather than assume: the new boot error must not
embed the URL. `connect_redis` wraps the raw `redis::RedisError` (`http/mod.rs:725`) and
`main.rs` surfaces it through `anyhow`; SMA-476 D4 went to some trouble to keep connection
details out of the breaker's error, and a config-triggered boot failure deserves the same
posture.

Rollback is a revert; nothing persists.

## 8. Acceptance criteria

| AC | Where it is met |
|---|---|
| 1. Distinct URLs → own connection, `RedisRole::ApiKeys` | §3; tests §4.2 phases (b) and (c) |
| 2. Identical URLs → shared connection preserved | §3 row 2; test §4.2 phase (a), with its positive control |
| 3. A test covering the distinct-URL configuration | §4.2 phases (b) and (c), plus the §4.1 table |
| 4. URL-comparison semantics are a recorded decision | D1, the `shares_one_connection` doc comment (D3), and §4.1's table making them executable |
| 5. SMA-476 D10's RUNBOOK caveat updated | §5.2 — and §5.1, which is the same caveat shipped as `# HELP` text |
| 6. `repo:redis-connect-single-site` still passes | §3 dials only through `connect_redis`; §4.3 records that the gate is what shapes the test's observation channel |

## 9. Verification

```
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

`:observability-drift` is expected to stay green (§5.1 changes doc text, not metric families)
but must be run rather than reasoned about, because `names.rs` is one of its inputs.
