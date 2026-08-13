# SMA-496 — redact `redis_url` in config dumps, the last credential-bearing fields left in the clear

**Status:** design
**Date:** 2026-08-13
**Issue:** [SMA-496](https://linear.app/smaschek/issue/SMA-496/iam-redact-redis-url-in-config-dumps-the-last-credential-bearing-field)
**Project:** Paigasus IAM — Hardening
**Follows:** [SMA-489](https://linear.app/smaschek/issue/SMA-489/iam-wake-the-outbox-relay-on-commit-so-delivery-is-not-gated-by-the) (PR 113, `ed25c6d`), which introduced `RedactedUrl` and applied it to `database_url` + `outbox.listen_database_url`
**Touches:** [SMA-485](https://linear.app/smaschek/issue/SMA-485/iam-api-keysintrospect-cacheredis-url-is-required-by-validate-and-then)'s connection-reuse comparison (must keep working)

All line references verified against `origin/main` at `42eb734`.

## 1. Problem

`IamConfig` derives `Debug` and `Serialize`. Four of its fields are connection URLs that
routinely embed credentials. Two of them are protected by the `RedactedUrl` newtype SMA-489
introduced (`src/config.rs:50`); the other two are not, or are protected by a mechanism that
does not travel with the type.

| Field | Location | Today |
|---|---|---|
| `database_url` | `config.rs:20` | `RedactedUrl` ✅ |
| `outbox.listen_database_url` | `config.rs:419` | `Option<RedactedUrl>` ✅ |
| `authn.jwks_cache.redis_url` | `config.rs:121` | `Option<String>` — **in the clear** |
| `authz.cache.redis_url` | `config.rs:167` | `Option<String>` — **in the clear** |
| `api_keys.introspect_cache.redis_url` | `config.rs:275` | `Option<String>` — **in the clear** |
| `outbox.publisher.url` | `config.rs:494` | `Option<String>` + ~40 lines of hand-rolled `Debug`/`Serialize` |

A Redis connection string carries credentials exactly as a Postgres DSN does
(`redis://user:password@host:6379/0`), so the three `redis_url` fields are the same class of
secret as the two already covered.

### 1.1 The stated motivation is not quite true, and the real one is better

SMA-489's `RedactedUrl` doc says `IamConfig` "is dumped in logs and `readyz` (`main.rs`)". So
does `RawPepper`'s doc (`config.rs:243`), and so does the PR-113 regression test's doc
(`config.rs:2137`). SMA-496's issue text repeats it: these fields "reach a log line or a
`/readyz` config dump unredacted".

**No such dump exists.** Verified against `42eb734`:

- `readyz` (`adapters/http/mod.rs:842-851`) returns `{"status":"ready"}` or
  `{"status":"unready"}`. It never touches the config.
- The only config-bearing log line is `main.rs:394`:
  `tracing::info!(%config.http_addr, %config.grpc_addr, "paigasus-iam started")` — two socket
  addresses, nothing else.
- `IamConfig` is `Serialize`d in exactly three places, all of them tests in this module
  (`config.rs:2128`, `:2179`, `:2928`).

This does not weaken the case for the change; it relocates it. The redaction is
**defense-in-depth against a dump that does not exist yet**: `Debug` and `Serialize` are both
public, derived, and reachable, so the leak arrives the day someone adds a boot-time config
log, a debug endpoint, or a stray `{config:?}` in an error path — and it arrives silently,
because nothing about writing that line looks dangerous. Choosing the type now makes that
future line safe by construction instead of a finding in review.

The inaccurate claim is corrected wherever it appears rather than propagated into three new
field docs (§5).

### 1.2 `publisher.url` is redacted by the pattern `RedactedUrl` exists to replace

`PublisherConfig` (`config.rs:490`) derives `Clone, Deserialize, PartialEq, Eq` and hand-rolls
`Debug` (`config.rs:571-587`) and `Serialize` (`config.rs:589-609`). Both enumerate all eleven
fields solely so that one of them — `url` — can be replaced with `"<redacted>"`. The
`Serialize` impl additionally hand-maintains its own field count:

```rust
let mut state = serializer.serialize_struct("PublisherConfig", 11)?;
```

`RedactedUrl`'s own doc (`config.rs:40-44`) names this as the thing it was built to avoid:

> A newtype rather than per-container manual impls **because redaction then travels with the
> type**: a future credential-bearing URL field is protected by choosing this type, not by
> remembering to extend two hand-written impls that spell out every sibling field.

The footgun is live. A twelfth `PublisherConfig` field must be added to two impls and a hand-
written count, or it silently vanishes from every dump — and no test would catch it, because
the existing publisher redaction tests only assert `contains("redacted")` (§4.3). Converting
`url` to `RedactedUrl` lets both impls be deleted and replaced with the ordinary derive every
sibling config type already carries.

Including it here is the same reasoning SMA-489 used to widen from `listen_database_url` to
`database_url`: redacting one connection URL while its identically-sensitive neighbour keeps a
weaker, hand-maintained guard would be incoherent.

### 1.3 There is no coverage of the three `redis_url` fields today

`config.rs`'s existing redaction tests cover `api_keys.pepper` (`:2102`), the two database DSNs
(`:2142`), the newtype in isolation (`:2194`), and `publisher.url` (`:2907`, `:2923`). Nothing
asserts anything about a `redis_url` in `Debug` or serialized output, because there is nothing
to assert.

## 2. Decisions

### D1 — All four URLs, one newtype, no new machinery

`JwksCacheConfig.redis_url`, `AuthzCacheConfig.redis_url`,
`ApiKeyCacheConfig.redis_url` and `PublisherConfig.url` all become `Option<RedactedUrl>`.
`RedactedUrl` itself is unchanged: no new methods, no `Display`, no `AsRef<str>`, no
`Option`-flattening helper. `as_str()` stays the single greppable door out, which is the
property the type's doc calls load-bearing (`config.rs:46-48`).

Every field is `Option` today and stays `Option`. None becomes required; this change has no
effect on `validate()`'s rules.

### D2 — `RedactedUrl` stays in `paigasus-iam::config`

The issue asks whether the type should move somewhere more central "if other services grow the
same need — `paigasus-gateway` has its own config with URLs". It should not, and the reason is
worth recording so it is not re-litigated:

1. **The gateway has no credential-bearing URL.** Its only URL is
   `upstream.openai.base_url` (`paigasus-gateway/src/config.rs:93`), defaulting to
   `https://api.openai.com` — an endpoint, not a DSN.
2. **The gateway already solved its actual secret differently.** `OpenAiConfig.api_key` is a
   `secrecy::SecretString` (`paigasus-gateway/src/config.rs:101`).
3. **IAM cannot adopt `secrecy`.** `SecretString` implements neither `PartialEq` nor `Eq`, and
   `IamConfig` derives both — a constraint already recorded at `paigasus-iam/src/config.rs:618`
   ("no `SecretString`-carrying field blocks it, unlike `GatewayConfig`"). This is why the
   bespoke newtype exists at all.
4. **There is no shared config crate to move it to.** `rs/crates/libs/` holds
   `paigasus-iam-core`, `paigasus-kernel`, `paigasus-kernel-parity`, `paigasus-logging`,
   `paigasus-observability`, `paigasus-proto` — none is a natural home. Minting one would pull
   in `deny.toml`, `cargo-machete`, CODEOWNERS and (if it touched the kernel) the
   `:affected-smoke` expected-set guard, for zero present benefit.

Move it when a second consumer with a credentialed URL actually exists.

### D3 — The SMA-485 reuse comparison is untouched

`AppState::new` pairs the authz Redis handle with the URL it was opened with
(`http/mod.rs:317`), and the API-key cache decides whether to reuse that connection by calling
`shares_one_connection` (`http/mod.rs:767`), which compares the two configured URLs after
trimming.

Neither the pairing type nor the comparison changes:

```rust
let (gens, redis_conn): (Generations, Option<(RedisHandle, &str)>) = …   // unchanged
pub(crate) fn shares_one_connection(authz_url: &str, api_key_url: &str) -> bool {
    authz_url.trim() == api_key_url.trim()                                // unchanged
}
```

Only the expression that produces the `&str` changes, from `.as_deref()` to
`.as_ref().map(RedactedUrl::as_str)`. The issue's caution — *"check the trimming logic still
applies to the right thing"* — resolves cleanly:

- `as_str()` returns the **real** stored string, not the placeholder. Redaction lives entirely
  in the `Debug`/`Serialize` directions, and the comparison goes through neither.
- The `&str` is still borrowed from the same `&IamConfig` that outlives `redis_conn`, so
  lifetimes are unchanged.
- `.trim()` therefore operates on byte-for-byte the same input as today, preserving D1 of
  SMA-485 (trailing-newline tolerance for env-var overrides).

`api_key_cache_connection.rs` — SMA-485's own Docker-gated regression suite, covering the
shared / split / unreachable / missing-URL cases — is the direct proof and keeps passing (§4.4).

### D4 — `.as_deref()` becomes `.as_ref().map(RedactedUrl::as_str)`, spelled out at each site

`.as_deref()` stops compiling: it requires `Deref`, which `RedactedUrl` deliberately does not
implement. No helper is added to shorten the replacement. Five sites is not enough repetition
to justify an extension trait, and the explicit `RedactedUrl::as_str` at each one is precisely
what makes `grep as_str` a complete audit of where real credentials are read. This is already
the house idiom — `main.rs:284` reads `listen_database_url` exactly this way.

### D5 — No config dump is added

Making the redaction load-bearing today by adding a boot-time `tracing::info!` of the whole
serialized config was considered and rejected for this issue. It is a new operator-visible log
line whose safety depends on auditing **every** field of `IamConfig` for sensitivity — not just
the four URLs, but `authz.bootstrap_admins[].subject` (IdP `sub` claims),
`authn.issuers[]`, and every field added later. That is a separate change with its own
threat model. This issue closes the type-level gap; it does not open a new output channel.

### D6 — The defaults layer gets a test, not three mirror structs

The `*Defaults` structs feed figment's default layer via `Serialized::defaults`
(`config.rs:909`). `OutboxDefaults` documents the trap at `config.rs:719-724`: a `RedactedUrl`
placed in a defaults struct serializes the literal `"<redacted>"` **into** that layer, and
figment then deserializes that string back out as the value. `OutboxDefaults` avoids it by
mirroring `listen_database_url` as a plain `String`.

But the `*Defaults` structs mirror only their *top-level* struct and reuse the real nested
types — `AuthzDefaults.cache: AuthzCacheConfig` (`config.rs:668`), `ApiKeyDefaults
.introspect_cache: ApiKeyCacheConfig` (`:686`), `AuthnDefaults.jwks_cache: JwksCacheConfig`
(`:656`), `OutboxDefaults.publisher: PublisherConfig` (`:726`). After this change, four
`RedactedUrl`s therefore sit inside the defaults layer, safe **only** because every default
value is `None`.

That is a real latent hazard, and it already exists today for `publisher.url` with no test
behind it. Someone writing `redis_url: Some("redis://localhost:6379".into())` into
`AuthzDefaults::default()` — an entirely reasonable-looking edit — would ship a build where
every deployment that does not override it boots with the literal string `<redacted>` as its
Redis URL.

Guarded by one test (§4.2) that serializes `Defaults::default()` and asserts the JSON contains
no `<redacted>` anywhere. Rejected alternative: mirror structs for the three cache configs.
That is structurally airtight but costs three new structs plus conversion sites, still would
not cover `publisher.url` without a fourth, and does not extend to fields added later — whereas
the test covers all four now and everything added afterwards, for four lines.

### D7 — The two existing publisher redaction tests are strengthened in place

`the_publisher_url_is_redacted_in_debug` (`config.rs:2907`) and
`…_in_serialize` (`:2923`) assert only `contains("redacted")`. That passes even if `url` were
dropped from the output entirely — the exact failure mode PR 113's in-place assertions were
written to catch (`config.rs:2182-2185`: *"a field silently dropped from the dump would also
satisfy the two assertions above"*). Both tests are being edited anyway for the type change, so
both get the in-place treatment.

## 3. The fix

### 3.1 Type changes (`src/config.rs`)

```rust
pub struct JwksCacheConfig {
    pub backend: JwksCacheBackend,
    pub redis_url: Option<RedactedUrl>,
}

pub struct AuthzCacheConfig {
    pub backend: AuthzCacheBackend,
    pub redis_url: Option<RedactedUrl>,
}

pub struct ApiKeyCacheConfig {
    pub backend: ApiKeyCacheBackend,
    pub redis_url: Option<RedactedUrl>,
    pub ttl_secs: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]   // was: Clone, Deserialize, PartialEq, Eq
pub struct PublisherConfig {
    pub backend: PublisherBackend,
    pub url: Option<RedactedUrl>,
    …
}
```

`impl std::fmt::Debug for PublisherConfig` (`config.rs:571-587`) and
`impl Serialize for PublisherConfig` (`config.rs:589-609`) are **deleted**.

The `*Defaults` structs are not modified: all four defaults are `None`, and D6's test guards
that they stay that way.

### 3.2 Read sites — five

| Site | Before | After |
|---|---|---|
| `http/mod.rs:322` | `authz_cfg.cache.redis_url.as_deref()` | `.as_ref().map(RedactedUrl::as_str)` |
| `http/mod.rs:579` | `cfg.api_keys.introspect_cache.redis_url.as_deref()` | same shape |
| `http/mod.rs:673` | `authn_cfg.jwks_cache.redis_url.as_deref()` | same shape |
| `config.rs:1152` | `p.url.as_deref()` | same shape |
| `adapters/events/nats_publisher.rs:231` | `cfg.url.as_deref()` | same shape |

`nats_publisher.rs` gains a `use crate::config::RedactedUrl;`. `http/mod.rs` extends its
existing `crate::config::{…}` import (`http/mod.rs:71`).

Unchanged: `p.url.is_none()` (`config.rs:1145`), every `validate()` error string, every
`redis_url: None` construction, and all three `AuthnError::Backend("… without redis_url …")`
wiring-defect guards.

### 3.3 Behavioural delta

Serialized JSON is **identical** for all four fields (`"url":"<redacted>"` either way — the old
hand-rolled impl emitted `Option<&str>`, which serializes the same as `Option<RedactedUrl>`).

Replacing the hand-rolled `PublisherConfig` impls with derives is safe to the byte: the struct
has exactly 11 fields (`config.rs:491-543`), carries **no** `#[serde(...)]` attribute on any of
them, and both hand-rolled impls already enumerate all 11 in declaration order — which is the
order the derives use. So the derived `Serialize` produces the same field set, the same names
and the same order as `serialize_struct("PublisherConfig", 11)` did, and the derived `Debug` the
same `debug_struct` layout.

`Debug` output for `publisher.url` changes cosmetically:

```
before:  url: Some("<redacted>")
after:   url: Some(RedactedUrl("<redacted>"))
```

Both existing publisher `Debug` assertions (`!contains("hunter2")`, `contains("redacted")`)
hold across that change. No runtime behaviour changes anywhere: no connection is opened
differently, no validation rule moves, no operator-visible output changes except the `Debug`
rendering above, which nothing in production emits.

## 4. Tests

### 4.1 `redis_urls_never_appear_in_debug_or_serialized_config` — new, `config.rs` `mod tests`

Modelled on `connection_urls_never_appear_in_debug_or_serialized_config` (`config.rs:2142`). A
figment `Jail` config setting all three caches to the `redis` backend, each with a **distinct**
password and host so a leak names its own source:

```toml
[authn.jwks_cache]
backend = "redis"
redis_url = "redis://jwks_user:jwks_pw_secret@jwks.example.com:6379/0"

[authz.cache]
backend = "redis"
redis_url = "redis://authz_user:authz_pw_secret@authz.example.com:6379/1"

[api_keys.introspect_cache]
backend = "redis"
redis_url = "redis://apikey_user:apikey_pw_secret@apikey.example.com:6379/2"
```

Asserts, in order:

1. **Non-vacuity.** Each field's `as_str()` yields the real configured URL back — so the
   negative assertions below cannot pass because figment simply failed to populate anything.
2. **`Debug` carries no secret.** No `*_pw_secret` and no `*.example.com` host in
   `format!("{cfg:?}")`.
3. **Serialized form carries no secret.** Same six substrings absent from
   `serde_json::to_string(&cfg)`.
4. **The placeholder lands in place.** `"redis_url":"<redacted>"` occurs exactly three times in
   the JSON — a count, not a `contains`, so a field dropped from the dump fails the test.

### 4.2 `defaults_never_serialize_a_redaction_placeholder` — new, `config.rs` `mod tests`

D6's guard.

```rust
let layer = serde_json::to_string(&Defaults::default()).expect("Defaults serializes");
assert!(
    !layer.contains("<redacted>"),
    "a RedactedUrl with a non-None default leaked the placeholder INTO figment's default \
     layer, which would then be deserialized back out as the real value: {layer}"
);
```

Covers all four URLs at once, plus any added later.

### 4.3 Strengthened — the two existing publisher tests

`the_publisher_url_is_redacted_in_debug` / `…_in_serialize` (`config.rs:2907`, `:2923`) keep
their `!contains("hunter2")` assertions and gain in-place ones, replacing bare
`contains("redacted")`:

```rust
assert!(serialized.contains(r#""url":"<redacted>""#), "{serialized}");
```

Construction changes from `Some("nats://user:hunter2@host:4222".to_string())` to
`Some("nats://user:hunter2@host:4222".into())`.

### 4.4 Existing tests — churn inventory

Unchanged (no edit needed):

- Every `redis_url: None` construction — `config.rs:756`, `:772`, `:793`;
  `tests/keycloak_e2e.rs:212`; `tests/support/mod.rs:331`.
- Every `assert_eq!(cfg.….redis_url, None)` — `config.rs:1348`, `:1576`, `:1793` (`RedactedUrl`
  derives `PartialEq`).
- Every `validate()` error-string assertion — `config.rs:1499`, `:1599`, `:1928`, `:2685`.
- `assert_eq!(cfg.outbox.publisher.url, None)` — `config.rs:2641`.

Mechanical edits:

| File | Sites | Change |
|---|---|---|
| `config.rs` | 3 (`:1531`, `:1763`, `:2068`) | `.as_deref()` → `.as_ref().map(RedactedUrl::as_str)` |
| `tests/authz_acceptance.rs` | 3 (`:466`, `:553`, `:637`) | `Some(redis_url)` → `Some(redis_url.into())` |
| `tests/api_key_cache_connection.rs` | 2 (`:58`, `:60`) | `Some(url.to_string())` → `Some(url.into())` |

`api_key_cache_connection.rs` is SMA-485's regression suite (shared URL / split URL /
unreachable URL / missing URL). It passing unchanged in substance is the evidence for D3.

### 4.5 Not tested

No test asserts that a `redis_url` reaches a log line, because no code path emits one (§1.1,
D5). The redaction is verified at the type and serialization level only — which is the whole of
what this change delivers.

## 5. Documentation

Corrected — the "dumped in logs and `readyz`" claim, replaced with the defense-in-depth
rationale of §1.1:

- `RedactedUrl`'s doc (`config.rs:33-36`)
- `RawPepper`'s doc (`config.rs:243-244`)
- `connection_urls_never_appear_in_debug_or_serialized_config`'s doc (`config.rs:2136-2138`)

Updated for the new type:

- `PublisherConfig`'s doc (`config.rs:483-488`) — the "hand-rolled rather than derived" paragraph
  is replaced by a note that `url` carries `RedactedUrl` and the struct now derives normally.
- `PublisherConfig.url`'s field doc (`config.rs:492-493`) — "see the manual impls below" no
  longer resolves.
- `RedactedUrl`'s doc gains the three `redis_url` fields in its "worn by" list (`config.rs:30-31`).

Added — a one-line doc on each of the three `redis_url` fields, pointing at `RedactedUrl` and
naming `as_str()` as the way to the real value, mirroring `database_url`'s (`config.rs:17-19`).

Not edited: `docs/ops/RUNBOOK-observability.md` mentions `redis_url` only as a configuration key
name, which does not change. No operator-facing behaviour changes, so no runbook or migration
note is required.

## 6. Out of scope

- **Adding a config dump.** D5.
- **Moving `RedactedUrl` to a shared crate.** D2.
- **Migrating `RawPepper` to a shared secret type**, or adopting `secrecy` in IAM. Blocked by
  the `PartialEq`/`Eq` constraint (D2.3) and orthogonal to URLs.
- **Normalising or parsing `redis_url`.** SMA-485 D1 deliberately compares raw strings after
  trimming rather than through `redis::ConnectionInfo`; this issue does not revisit that.
- **Trimming `redis_url` in `validate()`.** The trim stays where SMA-485 put it, inside
  `shares_one_connection`.
- **`paigasus-gateway`.** No credential-bearing URL to redact (D2.1).

## 7. Rollout and residual risk

Config-file compatible in both directions: `RedactedUrl`'s `Deserialize` delegates straight to
`String` (`config.rs:66-73`), so every existing `iam.toml` and `IAM_*` env override parses
identically. No migration, no operator action, no version coupling. Revertible by reverting the
commit.

Residual risks, both low:

1. **A `RedactedUrl` reaches a default layer later.** Guarded by §4.2.
2. **A future reader reaches for the real URL and finds only the placeholder.** `as_str()` is
   the only accessor and is documented at its definition; the compiler rejects `.as_deref()`,
   `{}` formatting, and `AsRef<str>` coercion, so the failure mode is a compile error rather
   than a silent wrong value.

## 8. Acceptance criteria

1. `authn.jwks_cache.redis_url`, `authz.cache.redis_url` and
   `api_keys.introspect_cache.redis_url` are `Option<RedactedUrl>`.
2. `outbox.publisher.url` is `Option<RedactedUrl>`; `PublisherConfig`'s hand-rolled `Debug` and
   `Serialize` impls are deleted and the struct derives both.
3. Neither `Debug` nor the serialized form of an `IamConfig` contains a configured Redis or NATS
   credential; each redacted field emits `"<redacted>"` **in place**, asserted by count.
4. `IamConfig::validate`'s rules and error strings are unchanged; SMA-485's
   `shares_one_connection` reuse decision behaves identically, proven by
   `tests/api_key_cache_connection.rs` passing unmodified in substance.
5. Figment's default layer contains no redaction placeholder, asserted by a test.
6. The "dumped in logs and `readyz`" claim is corrected everywhere it appears (§5).

## 9. Verification

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"

# Fast loop, in rs/
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo nextest run -p paigasus-iam config:: --no-tests=pass

# Docker-gated: the SMA-485 regression suite (D3's evidence)
cargo nextest run -p paigasus-iam --test api_key_cache_connection --test authz_acceptance

# Full graph, as CI runs it
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :nats-permissions :release-parity :release-parity-py \
  :release-parity-ts --base origin/main --include-relations
```

No new dependency is added, so no `rs/deny.toml` or `cargo-machete` waiver is expected.
