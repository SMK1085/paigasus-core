# SMA-448 — `paigasus-kernel`: PRN primitive + UUIDv7 minting

**Status:** Draft — revised after adversarial challenge (pending GATE 1 approval)
**Date:** 2026-06-30
**Linear:** SMA-448 (blocks SMA-442 / IAM M1)
**ADRs:** ADR-0014 (Tenancy & PRN), ADR-0005 (kernel + bindings), ADR-0013 (Cedar — referenced, not yet implemented)

## 1. Context & goal

`paigasus-kernel` is still a placeholder (`sum()`). ADR-0014 makes PRN **kernel-first**:
every Paigasus resource needs one stable, legible identifier — a `Prn` value type
(parse / validate / build / canonicalize) plus UUIDv7 id minting — living once in the
Rust kernel (pure logic, ADR-0005) and bound to Python / Node / WASM with cross-language
parity vectors. IAM M1 (SMA-442) mints PRNs via this kernel and is blocked until it lands.

This feature adds the PRN surface **alongside** the existing `sum` placeholder (which the
parity harness, bindings, and py/ts replay tests still depend on). Retiring `sum` is an
out-of-scope follow-up; keeping it bounds this PR's blast radius.

## 2. Resolved design decisions

From the brainstorm (B) and the adversarial challenge (C):

1. **(B) UUIDv7 minting is fully injected.** The kernel mint takes `(unix_ms, rand: [u8;10])`
   and performs **no I/O** — no clock, no `getrandom`. Each binding/wrapper supplies time
   and entropy from its host. Keeps the kernel pure (ADR-0005), makes minting deterministic
   (the UUIDv7 *bit-layout* is itself parity-vector-testable), and avoids pulling
   `getrandom`/a clock into the `wasm32-unknown-unknown` build.
2. **(B) Org slot carries the raw UUID** (no `org_` prefix) in the canonical PRN.
   Identity/equality is the UUID (ADR-0014 §4); the canonical form drives
   equality / cache-keys / signatures, so a decorative prefix must never be part of it.
3. **(B) Cedar mapping is a pure string mapping** — no `cedar-policy` dependency.
4. **(B) Org-self PRN grammar corrected:** `prn:pgs:iam:::organization/<org>` — **empty
   tenant slot**, org UUID in the **resource-id** position — not the ADR's
   `…:org_…:organization`. Every PRN is now uniform (`<resource-type>/<resource-id>` always
   present); an empty tenant slot means "no owning org," true for the tenant root
   (`organization`) and global principals (`user`). **ADR-0014 must be amended** (§10).
5. **(B) Kernel validates grammar/syntax only**, not tenancy semantics (no
   org-presence-per-resource-type rule — that's IAM policy). `resource-type` is an **open
   set**. `resource-id` is validated as any syntactically valid UUID on parse; **minting
   always emits UUIDv7**.
6. **(C-BLOCKER) The kernel's `uuid` dependency carries NO rng feature.** The workspace pin
   is currently `uuid = { version = "1", features = ["v4", "serde"] }`; `v4` (and `rng`)
   transitively pull `getrandom`, which does not build on `wasm32-unknown-unknown` without a
   JS cfg. The kernel is `uuid`'s first real consumer, so it would inherit `v4` and **break
   the wasm build**. Fix: **slim the workspace `uuid` baseline to `uuid = { version = "1" }`**
   (default `std` only; nothing consumes `uuid` today, so `v4`/`serde` are safe to drop), and
   mint via **hand-assembled bytes + `Uuid::from_bytes`** (needs no feature). This is
   mandatory, not conditional.
7. **(C-BLOCKER) FFI `unix_ms` is `f64`, not `u64`.** napi/wasm map `u64`→JS `BigInt`, which
   would throw against the prescribed `Date.now()` wrappers and the JSON-number corpus (the
   same hazard that forced `sum` to `i32`). All valid 48-bit ms values are < 2^53, so `f64`
   is exact. The **kernel** signature stays `u64`; each shim casts `f64 → u64`.
8. **(C-MAJOR) Cedar is exposed as two string accessors**, not an object — see §5.
9. **(C-MAJOR) `service` and `resource-type` use a hyphen-strict regex**
   `^[a-z][a-z0-9]*(-[a-z][a-z0-9]*)*$` (no leading/trailing/double hyphen, AND every
   post-hyphen segment starts with a letter — else `a1`/`a-1` collide under the Cedar
   PascalCase mapping) so the PascalCase Cedar mapping is injective (`api--key` and
   `api-key` must not collide, nor must `a1` and `a-1`).
10. **(C-MAJOR) Case handling is split:** scheme/partition/service/region/resource-type are
    matched **case-sensitively (lowercase only)** — uppercase is rejected, not normalized.
    Only UUID fields (org, resource-id) are case-normalized (accepted mixed-case, emitted
    lowercase).
11. **(C-MAJOR) FFI `mint_uuid7` is fallible** — it validates `rand_hex`; the kernel mint
    stays infallible.
12. **(C-QUESTION) `Prn` does not derive serde.** The FFI surface is strings and the corpus
    structs use `String`, so neither `Prn` nor the kernel's `uuid` needs `serde`.

## 3. Grammar, parsing & canonical form

```
prn:pgs:<service>:<region>:<org>:<resource-type>/<resource-id>
```

Six **colon-delimited** fields; the sixth is the resource path with exactly one `/`. The
resource path contains no `:`, so splitting the whole string on `:` yields exactly six fields
for a well-formed PRN.

| field | v1 rule (matched case-sensitively unless noted) |
|---|---|
| `prn` | literal lowercase `prn` |
| `pgs` | literal lowercase `pgs` |
| `service` | `^[a-z][a-z0-9]*(-[a-z][a-z0-9]*)*$`; non-empty; every post-hyphen segment starts with a letter — else `a1`/`a-1` collide under the Cedar PascalCase mapping |
| `region` | **empty in v1** (what minting emits), OR forward-compat `^[a-z0-9]+(-[a-z0-9]+)*$` (accepted on parse, never minted in v1) |
| `org` | **empty**, OR a syntactically valid UUID (case-normalized to lowercase). Empty for `organization`/`user` |
| `resource-type` | `^[a-z][a-z0-9]*(-[a-z][a-z0-9]*)*$`; non-empty; open set; every post-hyphen segment starts with a letter — else `a1`/`a-1` collide under the Cedar PascalCase mapping |
| `resource-id` | a syntactically valid UUID (case-normalized); **always present**; minted ids are UUIDv7 |

**Max length:** `parse` rejects inputs longer than **512 bytes** (`TooLong`) before any other
work — bounds regex scans and downstream storage/index cost (ARNs cap for the same reason).

### Parse algorithm (deterministic; defines which error fires)

1. empty input → `Empty`.
2. `len > 512` → `TooLong`.
3. split on `:`; if the count ≠ 6 → `WrongFieldCount` (covers `prn:pgs:iam::::extra` → 7 parts).
4. field 0 ≠ `prn` → `BadScheme`.
5. field 1 ≠ `pgs` → `BadPartition`.
6. field 2 fails the service regex → `BadService`.
7. field 3 non-empty and fails the region regex → `BadRegion`.
8. field 4 non-empty and not a valid UUID → `BadOrg`.
9. field 5 (resource path): not exactly one `/` → `BadResourcePath`; left segment empty or
   fails the resource-type regex → `BadResourceType`; right segment empty or not a valid UUID
   → `BadResourceId`.

`build(service, region, org: Option<Uuid>, resource_type, resource_id)` applies the same field
validation and returns the same typed errors.

**Canonical form** = parse → re-emit: lowercase ASCII throughout; exactly six `:`-fields, the
sixth with exactly one `/`; UUID fields (org when present, resource-id) lowercase hyphenated
`8-4-4-4-12`; no surrounding whitespace. Canonicalization is **idempotent**
(`canonical(canonical(x)) == canonical(x)`) and is the basis for equality, cache-keys, and
signatures. `Prn` derives `Eq`/`Hash` over the canonical field tuple (≡ canonical-string
equality).

### Canonical examples

```
prn:pgs:iam:::organization/0190a1e5-0000-7000-8000-000000000000      # org (tenant root) — empty tenant slot
prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:team/0190a1b2-…    # team, scoped to org
prn:pgs:iam::0190a100-…:project/0190a1c3-…
prn:pgs:iam::0190a100-…:service-account/0190a1d4-…
prn:pgs:iam:::user/0190a1e5-…                                        # user = global identity, empty tenant slot
prn:pgs:gateway::0190a100-…:api-key/0190a1f6-…                       # another service, same scheme
```

## 4. Module layout (`rs/crates/libs/paigasus-kernel/src/`)

- **`prn.rs`** — `pub struct Prn { service: String, region: String, org: Option<Uuid>,
  resource_type: String, resource_id: Uuid }`; `pub enum PrnError` (derive
  `thiserror::Error`, plus `kind(&self) -> &'static str` returning the stable token from §7);
  `Prn::parse`, `Prn::canonical`, `Prn::build`, accessors (`service`, `region`, `org`,
  `resource_type`, `resource_id`); `PartialEq`/`Eq`/`Hash` over the canonical tuple. No serde
  derive.
- **`uuid7.rs`** — `pub fn mint_uuid7(unix_ms: u64, rand: [u8; 10]) -> Uuid`, pure &
  deterministic, exact RFC 9562 v7 layout (below). Built with `Uuid::from_bytes` — **no
  `uuid` feature beyond default**.
- **`cedar.rs`** — `pub struct CedarUid { pub entity_type: String, pub entity_id: String }`
  (Rust-only) and `pub fn to_cedar_uid(prn: &Prn) -> CedarUid`. `entity_type =
  "Pgs::" + pascal(service) + "::" + pascal(resource_type)` where `pascal` upper-cases the
  first char of each `-`-segment and concatenates (the §2.9 regex guarantees no empty
  segment, so the mapping is injective); `entity_id = resource_id` (lowercase hyphenated).
  `Pgs` is the partition (`pgs`) PascalCased — constant for v1.
- **`lib.rs`** — re-exports `prn`, `uuid7`, `cedar`; **keeps `sum`**.

### UUIDv7 byte layout (RFC 9562) — exact, deterministic

Given `unix_ms: u64` and `rand: [u8; 10]`, produce `bytes: [u8; 16]`:

- `ms48 = unix_ms & 0x0000_FFFF_FFFF_FFFF` (mask to low 48 bits; values ≥ 2^48 wrap — the
  proptest asserts the embedded ts equals `ms48`).
- `bytes[0..6] = ms48.to_be_bytes()[2..8]` (48-bit big-endian timestamp).
- `bytes[6..16] = rand[0..10]`, then overwrite the version/variant bits:
  - `bytes[6] = 0x70 | (rand[0] & 0x0F)` → version nibble `0b0111`; **discards the high nibble
    of `rand[0]`** (4 bits).
  - `bytes[8] = 0x80 | (rand[2] & 0x3F)` → variant bits `0b10`; **discards the high 2 bits of
    `rand[2]`** (2 bits).
- 74 random bits survive (4 + 8 + 6 + 56). `Uuid::from_bytes(bytes)`.

### `uuid` crate features (workspace + kernel)

Change the workspace pin to **`uuid = { version = "1" }`** (default `std` only). The kernel
declares `uuid.workspace = true`. Parsing (`Uuid::parse_str`/`try_parse`), `from_bytes`, and
`Display` (lowercase hyphenated) are all in default features — **no `rng`/`v4`/`v7`/`getrandom`
is pulled**. Invariant verified by §8's wasm dependency-graph gate.

## 5. Binding surface — stateless, value-returning FFI functions

Each shim exposes plain functions; **no `Prn` or struct object crosses the FFI boundary**:

| function | returns / errors |
|---|---|
| `prn_canonicalize(s: String) -> String` | canonical string; raises/throws on invalid (message embeds the §7 token) |
| `prn_error_kind(s: String) -> String` | `""` if valid, else the stable §7 token (the value the parity corpus compares) |
| `prn_build(service, region, org, resource_type, resource_id) -> String` | canonical string; raises/throws on invalid |
| `prn_service` / `prn_region` / `prn_org` / `prn_resource_type` / `prn_resource_id` `(s) -> String` | accessor strings; `org`/`region` map `None`/empty → `""` |
| `mint_uuid7(unix_ms: f64, rand_hex: String) -> String` | minted UUID string; **fallible** — raises/throws `bad-rand-hex` if `rand_hex` is not exactly 20 lowercase hex chars, or `bad-unix-ms` if `unix_ms` is not finite/non-negative/integral/`< u64::MAX` |
| `prn_cedar_entity_type(s) -> String` | `"Pgs::Iam::Project"`-style namespace+type |
| `prn_cedar_entity_id(s) -> String` | the resource-id UUID |

**Marshalling rules** (stated so all three shims agree):
- `unix_ms`: FFI `f64`, cast `f64 → u64` in the shim (48-bit ms is exact in `f64`). The shim
  validates `unix_ms` is finite, non-negative, integral, and `< u64::MAX as f64` **before** the
  cast, else raises/throws `bad-unix-ms` (a bare `as u64` would silently coerce NaN→0,
  +Inf→`u64::MAX`, negative→0, fractional→truncated, and any finite value ≥ `u64::MAX` saturated).
- `rand_hex`: exactly 20 lowercase hex chars → `[u8;10]`; else the shim raises/throws
  `bad-rand-hex` (a mint-only token, distinct from `PrnError`).
- `org`/`region`: `"" ⇔ None`/empty in **both** directions (`prn_build` maps `""` org → `None`,
  not `BadOrg`; `prn_org` maps `None` → `""`).

**Rationale & rejected alternative.** A rich `Prn` *class* across FFI was rejected: 3× the
surface, harder parity testing, divergence risk, YAGNI (IAM is Rust and uses the real `Prn`).
A structured `prn_to_cedar` object was also rejected (C-MAJOR): wasm-bindgen can't return an
arbitrary struct without `serde-wasm-bindgen` (reopening the getrandom-style feature audit) or
typed `any`, and `any` makes the napi↔wasm type-parity guard pass vacuously. Two string
accessors keep the surface honest and guardable.

### Ambient minting convenience (language wrappers, not FFI)

The FFI `mint_uuid7` is pure (injected `unix_ms` + `rand_hex`). The ergonomic ambient `mint()`
lives in the language wrappers and must hex-encode exactly 10 bytes:

- Python (`paigasus_kernel/__init__.py`): `time.time_ns() // 1_000_000` (→ `float`) and
  `os.urandom(10).hex()`.
- Node (`ts .../index.ts`) & browser (`ts .../wasm.ts`): `Date.now()` and
  `[...crypto.getRandomValues(new Uint8Array(10))].map(b => b.toString(16).padStart(2,'0')).join('')`.

Rust/IAM inject their own clock + RNG via DI.

## 6. Parity vectors (extend `paigasus-kernel-parity`)

The kernel is the **single oracle**; the parity crate generates byte-stable JSON corpora every
binding replays. New corpora alongside `vectors/sum.json`, all **deterministically enumerated
(no PRNG)** so regeneration is byte-stable:

- **`vectors/uuid7.json`** — `{ unix_ms: <number>, rand_hex: <20-hex>, expected_uuid }`.
  Boundary timestamps (0, `2^48-1`) and representative `rand` patterns (all-zero, all-`0xFF`,
  alternating, distinct bytes) — deterministically enumerated.
- **`vectors/prn_canonical.json`** — `{ input, error_kind, canonical }`. Valid cases:
  `error_kind: ""`, `canonical: <string>`. Invalid cases: `error_kind: <token>`,
  **`canonical: null`** (always `null`, never absent — `null` and absent are different bytes;
  no `skip_serializing_if`). Covers every `PrnError` variant ≥ 1 case + the malformed shapes
  in §3 (uppercase scheme/service reject, mixed-case UUID accept, `type/a/b`, `/id`, `type/`,
  7-field, `a--b`/`a-` rejects, too-long, empty org/region forms).
- **`vectors/prn_cedar.json`** — `{ prn, entity_type, entity_id }`, including multi-dash
  (`api-key`, `service-account`, `oauth2-client`) and the `organization`/`user` types.

Harness changes in `paigasus-kernel-parity/src/lib.rs`:
- New case structs (`Uuid7Case`, `PrnCanonicalCase`, `PrnCedarCase`) + `build_*_corpus()`
  generators computing expectations **from the kernel**.
- The byte-stable one-object-per-line `serialize` and the `corpus_path`/`load_corpus` helpers
  are **generified** (currently monomorphic to `Case`/`sum.json`): a generic
  `serialize<T: Serialize>` and a `corpus_path(name)` / `load_corpus<T>(name)` taking the file
  stem, so each corpus reuses the same byte-stable format.
- `src/bin/gen-parity-vectors.rs` writes **all** corpora; `repo:parity-corpus-drift`
  regenerates and `git diff --exit-code`s.
- Replays add, **per corpus**, both guards the sum corpus has: a non-empty integrity guard and
  a `committed == fresh-generation` guard — in Rust `tests/replay.rs`, Python
  `tests/test_parity.py`, and TS `tests/*.test.ts` / `*.wasm.test.ts` (parametrized, with the
  "present and non-empty" guard so an empty set fails RED, not vacuously green).

## 7. Error model — complete token vocabulary

`PrnError` (Rust, `thiserror`) is the source of truth; `kind()` returns the stable token that
the `prn_error_kind` FFI fn and the `prn_canonical.json` corpus compare across languages.

| `PrnError` variant | token |
|---|---|
| `Empty` | `empty` |
| `TooLong` | `too-long` |
| `BadScheme` | `bad-scheme` |
| `BadPartition` | `bad-partition` |
| `WrongFieldCount` | `wrong-field-count` |
| `BadService` | `bad-service` |
| `BadRegion` | `bad-region` |
| `BadOrg` | `bad-org` |
| `BadResourcePath` | `bad-resource-path` |
| `BadResourceType` | `bad-resource-type` |
| `BadResourceId` | `bad-resource-id` |

Mint-only (not a `PrnError`; raised by the FFI shim, not the kernel): **`bad-rand-hex`**,
**`bad-unix-ms`**.

Idiomatic wrappers raise/throw with the token in the message: PyO3 → `ValueError`; napi/wasm →
`Error`. The token vocabulary is the cross-language contract; adding a variant means adding a
token here + a corpus case.

## 8. Testing strategy

- **Kernel proptests** (`tests/`, proptest):
  - UUIDv7: version == 7; RFC-4122 variant; embedded 48-bit ts == `unix_ms & 0xFFFF_FFFF_FFFF`;
    the 74 surviving rand bits round-trip from the input; **k-sortability** —
    `ms_a < ms_b (both < 2^48) ⇒ bytes(mint(ms_a, r1)) < bytes(mint(ms_b, r2))` lexicographically.
  - PRN: `parse(canonical(p)) == p`; `canonical` idempotence; `build`↔`parse` agreement;
    representative invalid strings reject with the expected `kind()`.
- **Parity replays** (Rust + Py + TS, §6): the cross-language safety net.
- **Per-binding runtime smoke** (extend existing per-binding smoke): each binding mints a fixed
  `(unix_ms, rand_hex)` → asserts the deterministic UUID, and canonicalizes a known PRN.
- **wasm `getrandom`-free gate (mandatory):** a check that asserts `getrandom` is absent from
  `paigasus-wasm`'s dependency graph (e.g. a Moon/CI step running
  `cargo tree -p paigasus-wasm --target wasm32-unknown-unknown -i getrandom` and requiring it
  to find nothing, plus a `cargo build -p paigasus-wasm --target wasm32-unknown-unknown`). This
  pins decision §2.6 as a regression guard, not a one-time manual check.
- **napi↔wasm type-parity guard** (`binding-parity.types.ts`): **extended to enumerate every
  new string-returning FFI fn** — `prn_canonicalize`, `prn_error_kind`, `prn_build`, the five
  accessors, `mint_uuid7`, `prn_cedar_entity_type`, `prn_cedar_entity_id` (plus the existing
  `sum`) — so a drift in any one signature fails `tsc`.
- **Surface cleanliness:** basedpyright-clean Python (`.pyi` updated with the full surface),
  ESLint/`tsc`-clean TS, clippy `-D warnings`, `cargo-machete` clean.

## 9. Acceptance-criteria mapping

| AC (SMA-448) | Satisfied by |
|---|---|
| PRN round-trips parse → canonical → string identically across Rust/Py/TS | §3 + §6 `prn_canonical.json` replay |
| Invalid PRNs rejected with typed errors | §7 + `prn_error_kind` + rejection vectors |
| UUIDv7 ids mint and are k-sortable | §4 layout + §8 k-sortability proptest + `uuid7.json` |
| IAM can depend on the published kernel for PRN construction | `Prn`/`build`/`mint_uuid7` public Rust API |
| Org-only encoding; accessors for service/region/org/type/id | §3 + §4 accessors |
| PRN ↔ Cedar UID mapping helper | §4 `cedar.rs` + §5 string accessors + §6 `prn_cedar.json` |
| Cross-binding parity vectors | §6 |
| Bind via PyO3/napi/wasm; smoke each; clean Py/TS surface | §5 + §8 |

## 10. ADR-0014 amendment (applied)

ADR-0014 was *Proposed*. Two corrections were applied in place in the Notion ADR on
2026-06-30 (pre-acceptance fix of a Proposed ADR; the "supersede, don't edit" rule governs
revisits of *decided* ADRs):

1. **Org-self PRN form** → `prn:pgs:iam:::organization/<org>` (empty tenant slot, org UUID as
   resource-id). Supersedes `prn:pgs:iam::org_…:organization` and the `org_…` examples.
2. **No `org_` prefix in the canonical org slot** — the tenant slot is the raw UUID; any type
   prefix is display-only.

## 11. Out of scope (explicit)

- Retiring the `sum` placeholder + its corpus/bindings/tests (follow-up).
- A rich cross-FFI `Prn` object/class (YAGNI).
- Tenancy semantics / org-presence-per-type validation (IAM's job).
- `region` semantics, multi-region, partition splits (forward-compat fields only).
- Cedar policy evaluation / `cedar-policy` integration (ADR-0013, separate work).
- Slugs / display names (separate mutable field, never in the PRN).
- npm/PyPI publication of the new surface (release activation, separate issues).

## 12. Changelog — adversarial challenge (Stage 2)

Challenger verdict: **NEEDS-WORK**. All 15 findings were justified and folded in; none rejected.

- **BLOCKER (uuid `v4`→getrandom):** §2.6, §4 — slim workspace `uuid` to no-features; mint via
  `from_bytes`; added the wasm dep-graph gate (§8).
- **BLOCKER (`unix_ms` u64→BigInt):** §2.7, §4, §5 — FFI `unix_ms` is `f64`; kernel stays `u64`.
- **MAJOR (cedar object):** §2.8, §4, §5 — two string accessors; `CedarUid` Rust-only.
- **MAJOR (hyphen-loose regex → non-injective Cedar):** §2.9, §3 — hyphen-strict regex.
- **MAJOR (case contradiction):** §2.10, §3 — lowercase-only fields rejected if uppercase;
  only UUIDs case-normalized.
- **MAJOR (mint rand_hex no error contract):** §2.11, §5, §7 — FFI mint fallible, `bad-rand-hex`.
- **MINOR (ts masking / rand-bit layout):** §4 — exact byte layout + `& 0xFFFF_FFFF_FFFF`.
- **MINOR (corpus null-vs-absent / generify / per-corpus guards):** §6 — `canonical: null`
  pinned; generified `serialize`/`load_corpus`; per-corpus guards in all three languages.
- **MINOR (token vocabulary by example):** §7 — full variant→token table.
- **MINOR (malformed-path disambiguation):** §3 — explicit parse algorithm + corpus cases.
- **MINOR (no max length):** §3 — 512-byte cap, `too-long` token.
- **MINOR (empty/None FFI marshalling):** §5 — `"" ⇔ None`/empty both directions.
- **QUESTION (cedar entity_id / namespace):** §4 — bare resource-id UUID; `Pgs::<Service>::<Type>`,
  constant `Pgs` v1 (org via Cedar `parents`), per ADR-0013/0014.
- **QUESTION (Prn serde):** §2.12 — no serde derive; corpus uses `String`; supports the
  no-feature `uuid`.
- **QUESTION (type-parity guard scope):** §8 — guard enumerates the full FFI surface.
