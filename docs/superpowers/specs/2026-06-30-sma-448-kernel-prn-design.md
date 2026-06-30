# SMA-448 — `paigasus-kernel`: PRN primitive + UUIDv7 minting

**Status:** Draft (pending adversarial challenge + GATE 1 approval)
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

## 2. Resolved design decisions (brainstorm)

1. **UUIDv7 minting is fully injected.** The kernel mint takes `(unix_ms, random_bytes)`
   and performs **no I/O** — no clock, no `getrandom`. Each binding/wrapper supplies time
   and entropy from its host. This keeps the kernel pure (ADR-0005), makes minting
   deterministic (so the UUIDv7 *bit-layout* is itself parity-vector-testable), and —
   critically — **avoids pulling `getrandom`/a clock into the `wasm32-unknown-unknown`
   build**, the binding where neither is available without JS shims.
2. **Org slot carries the raw UUID** (no `org_` display prefix) in the canonical PRN.
   "Identity/equality is the UUID" (ADR-0014 §4); the canonical form drives
   equality / cache-keys / signatures, so a decorative prefix must never be part of it.
3. **Cedar mapping is a pure string mapping** — `(service, resource-type) → "Pgs::Iam::Project"`
   + `resource-id → entity id`, returned as plain data. No `cedar-policy` dependency
   (ADR-0013 is not yet implemented).
4. **Org-self PRN grammar corrected** (Sven): the org-itself resource is
   `prn:pgs:iam:::organization/<org>` — **empty tenant slot**, the org's UUID in the
   **resource-id** position — *not* the ADR's `…:org_…:organization`. This makes every PRN
   uniform (`<resource-type>/<resource-id>` always present, no "type-without-id" special
   case) and reads correctly: an empty tenant slot means "no owning org," true for both the
   tenant root (`organization`) and global principals (`user`). **ADR-0014 must be amended**
   to reflect this (it is still *Proposed*); see §10.
5. **Kernel validates grammar/syntax only**, not tenancy semantics. It does *not* enforce
   "`organization`/`user` must have an empty tenant slot while `team`/`project` must have a
   non-empty one." That org-presence-per-resource-type rule is IAM policy; encoding a
   resource-type taxonomy in the kernel would couple it to every service. Kernel = grammar;
   services = semantics. `resource-type` is therefore an **open set** (any `[a-z][a-z0-9-]*`).
6. **`resource-id` is validated as any syntactically valid UUID** on parse (accept any
   version for forward-compat); **minting always emits UUIDv7**. Non-UUID resource-ids are
   rejected with a typed error.

## 3. Grammar & canonical form

```
prn:pgs:<service>:<region>:<org>:<resource-type>/<resource-id>
```

Six colon-delimited fields; field 6 is the resource path with exactly one `/`.

| field | v1 rule |
|---|---|
| `prn` | literal scheme keyword (lowercase) |
| `pgs` | constant partition (lowercase) |
| `service` | `^[a-z][a-z0-9-]*$` (e.g. `iam`, `gateway`); non-empty |
| `region` | **empty in v1** (reserved). Validation: empty, OR `^[a-z0-9-]+$` (forward-compat) |
| `org` | tenant slot. Empty, OR a syntactically valid UUID. Empty for tenant-root (`organization`) and global (`user`) resources |
| `resource-type` | `^[a-z][a-z0-9-]*$`; non-empty; open set |
| `resource-id` | a syntactically valid UUID; **always present**; minted ids are UUIDv7 |

**Canonical form** = parse → re-emit:

- lowercase ASCII throughout;
- exactly six `:`-separated fields, the sixth containing exactly one `/`;
- UUID fields (org when present, resource-id) normalized to lowercase hyphenated
  `8-4-4-4-12`;
- no surrounding whitespace.

Canonicalization is **idempotent** (`canonical(canonical(x)) == canonical(x)`) and is the
basis for equality, cache-keys, and signatures. Parsing accepts mixed-case UUIDs and
lowercases them; it rejects structural violations.

### Canonical examples

```
prn:pgs:iam:::organization/0190a1e5-0000-7000-8000-000000000000      # org (tenant root) — empty tenant slot
prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:team/0190a1b2-…    # team, scoped to org
prn:pgs:iam::0190a100-…:project/0190a1c3-…                           # project, scoped to org
prn:pgs:iam::0190a100-…:service-account/0190a1d4-…
prn:pgs:iam:::user/0190a1e5-…                                        # user = global identity, empty tenant slot
prn:pgs:gateway::0190a100-…:api-key/0190a1f6-…                       # another service, same scheme
```

## 4. Module layout (`rs/crates/libs/paigasus-kernel/src/`)

- **`prn.rs`** — the core value type and parser:
  - `pub struct Prn` owning the five logical fields (`service`, `region`, `org: Option<Uuid>`,
    `resource_type`, `resource_id: Uuid`). Region kept as `String` (empty in v1) for
    forward-compat.
  - `pub enum PrnError` (derive `thiserror::Error`, plus a stable `kind()` token for the
    binding/parity surface): variants such as `Empty`, `BadScheme`, `BadPartition`,
    `WrongFieldCount`, `BadService`, `BadRegion`, `BadOrg`, `MissingResourcePath`,
    `BadResourceType`, `MissingResourceId`, `BadResourceId`.
  - `Prn::parse(&str) -> Result<Prn, PrnError>`, `Prn::canonical(&self) -> String`,
    `Prn::build(service, region, org: Option<Uuid>, resource_type, resource_id) -> Result<Prn, PrnError>`,
    accessors (`service()`, `region()`, `org()`, `resource_type()`, `resource_id()`).
  - `PartialEq`/`Eq`/`Hash` defined over the canonical field tuple (equivalent to canonical
    string equality).
- **`uuid7.rs`** — `pub fn mint_uuid7(unix_ms: u64, rand: [u8; 10]) -> Uuid`: builds the
  RFC 9562 v7 layout (48-bit big-endian ms timestamp, version nibble `0b0111`, variant bits
  `0b10`, remaining 74 bits from `rand`). Constructed via the `uuid` crate's `Builder`
  (or hand-assembled bytes) so the **wasm build links no `getrandom`/clock**. Pure and
  deterministic.
- **`cedar.rs`** — `pub struct CedarUid { entity_type: String, entity_id: String }` and
  `pub fn to_cedar_uid(prn: &Prn) -> CedarUid`. `entity_type = "Pgs::<ServicePascal>::<TypePascal>"`
  where PascalCase converts kebab segments (`iam`→`Iam`, `service-account`→`ServiceAccount`,
  `api-key`→`ApiKey`); `entity_id = resource_id` (lowercase hyphenated UUID). The `Pgs`
  prefix is the partition (`pgs`) PascalCased — constant for v1.
- **`lib.rs`** — re-exports `prn`, `uuid7`, `cedar` items; **keeps `sum`**.

### `uuid` crate features

The workspace pins `uuid = { version = "1", features = ["v4", "serde"] }`. This feature adds
`"v7"` if (and only if) the chosen constructor needs it. **Invariant:** the kernel's
`wasm32-unknown-unknown` build must not depend on `getrandom`. If enabling `"v7"` would pull
`getrandom` transitively, assemble the 16 UUID bytes directly instead and keep the feature
set minimal. This is verified by a wasm build during implementation.

## 5. Binding surface — stateless, value-returning FFI functions

Each shim (`paigasus-py-bindings`, `paigasus-node-bindings`, `paigasus-wasm`) exposes plain
functions over strings — **no `Prn` object crosses the FFI boundary**:

| function | returns |
|---|---|
| `prn_canonicalize(s)` | canonical string; raises/throws on invalid (idiomatic wrapper) |
| `prn_error_kind(s)` | `""` if valid, else a **stable error-kind token** (the value the parity corpus compares) |
| `prn_build(service, region, org, resource_type, resource_id)` | canonical string; raises/throws on invalid |
| `prn_service(s)` / `prn_region(s)` / `prn_org(s)` / `prn_resource_type(s)` / `prn_resource_id(s)` | accessor strings |
| `mint_uuid7(unix_ms, rand_hex)` | the minted UUID string |
| `prn_to_cedar(s)` | `{ entity_type, entity_id }` (dict / object) |

**Rationale & alternative considered.** A rich `Prn` *class* across FFI (PyO3 `#[pyclass]` /
napi class / wasm struct) was rejected: 3× the FFI surface, harder parity testing,
per-binding divergence risk, and YAGNI (no consumer needs a cross-FFI object — IAM is Rust
and uses the real `Prn` type directly). Stateless functions match the established
`sum`/`sum_as_string` pattern and keep parity vectors as simple value comparisons.

**`prn_error_kind` exists for parity hygiene:** comparing a raised exception's *shape* across
Python/Node/WASM is fragile; comparing a returned token string is byte-exact. The idiomatic
throwing `parse()` is built on top in the language wrappers.

### Ambient minting convenience (language wrappers, not FFI)

The FFI `mint_uuid7` is pure (injected `unix_ms` + `rand_hex`). The ergonomic
"`mint()` from the ambient clock + CSPRNG" lives in the **language wrappers**:

- Python (`paigasus_kernel/__init__.py`): `time.time_ns() // 1_000_000` + `os.urandom(10)`.
- Node (`ts .../index.ts`): `Date.now()` + `crypto.getRandomValues`.
- Browser (`ts .../wasm.ts`): `Date.now()` + `crypto.getRandomValues`.

Rust/IAM inject their own clock + RNG via DI (Sven's convention). This keeps the
cross-FFI surface deterministic while giving each language an idiomatic ergonomic entry point.

## 6. Parity vectors (extend `paigasus-kernel-parity`)

The kernel is the **single oracle**; the parity crate generates byte-stable JSON corpora
that every binding replays. New corpora alongside `vectors/sum.json`:

- **`vectors/uuid7.json`** — `{ unix_ms, rand_hex, expected_uuid }`. Deterministic mint layout
  (injected inputs make minting itself vector-testable). Boundary timestamps (0, max 48-bit)
  and representative random patterns, deterministically enumerated (no PRNG).
- **`vectors/prn_canonical.json`** — `{ input, error_kind, canonical }` where `error_kind` is
  `""` for valid inputs (and `canonical` is the canonical string) or the stable kind token for
  invalid inputs (and `canonical` is `null`/absent). Covers **valid round-trips and every
  rejection path** (each `PrnError` variant has ≥1 case).
- **`vectors/prn_cedar.json`** — `{ prn, entity_type, entity_id }`.

Implementation in the parity crate:

- New `Case`-style structs (`Uuid7Case`, `PrnCanonicalCase`, `PrnCedarCase`) with
  `build_*_corpus()` generators that compute expectations **from the kernel**.
- `serialize` stays one-compact-object-per-line + trailing newline (byte-stable diffs).
- `src/bin/gen-parity-vectors.rs` writes all corpora; the `repo:parity-corpus-drift` Moon task
  regenerates and `git diff --exit-code`s (a kernel edit without regenerating fails CI red).
- Replays: Rust `tests/replay.rs` (+ integrity guard per corpus: non-empty), Python
  `tests/test_parity.py`, TS `tests/*.test.ts` / `*.wasm.test.ts` — each parametrized over its
  corpus, with the existing "corpus present and non-empty" integrity guard pattern so an empty
  set fails RED rather than passing vacuously.

## 7. Error model

`PrnError` (Rust, `thiserror`) is the source of truth. Each variant maps to a **stable kind
token** (e.g. `"bad-scheme"`, `"wrong-field-count"`, `"bad-resource-id"`) surfaced by
`PrnError::kind()` and by the `prn_error_kind` FFI function — this token is what the parity
corpus compares across languages. Idiomatic wrappers raise/throw:

- PyO3: `prn_canonicalize`/`prn_build` raise `ValueError` (message includes the kind token).
- napi / wasm: throw an `Error` (message includes the kind token).

The token vocabulary is part of the parity contract and is enumerated in the corpus.

## 8. Testing strategy

- **Kernel proptests** (`tests/`, proptest, dev-dep already present):
  - UUIDv7: `mint_uuid7` output has version == 7 and RFC-4122 variant bits; the embedded
    48-bit timestamp equals the input ms; **k-sortability** — `ms_a < ms_b ⇒
    bytes(mint(ms_a, r1)) < bytes(mint(ms_b, r2))` lexicographically.
  - PRN: `parse(canonical(p)) == p` round-trip; `canonical` idempotence; `build`↔`parse`
    agreement; representative invalid strings reject with the expected `kind()`.
- **Parity replays** (Rust + Py + TS, §6): the cross-language safety net.
- **Per-binding runtime smoke** (extend the existing per-binding smoke tasks): each binding
  mints a fixed `(unix_ms, rand_hex)` and asserts the deterministic UUID, and canonicalizes a
  known PRN — proving the FFI path end-to-end on each runtime.
- **Surface cleanliness:** basedpyright-clean Python (`.pyi` updated), ESLint/`tsc`-clean
  TypeScript (napi↔wasm `sum`+new-surface type-parity guard extended), clippy `-D warnings`,
  `cargo-machete` clean.
- **wasm `getrandom`-free build** check (§4 invariant).

## 9. Acceptance-criteria mapping

| AC (SMA-448) | Satisfied by |
|---|---|
| PRN round-trips parse → canonical → string identically across Rust/Py/TS | §3 canonical form + §6 `prn_canonical.json` replay |
| Invalid PRNs rejected with typed errors | §7 `PrnError` + `prn_error_kind` + rejection vectors |
| UUIDv7 ids mint and are k-sortable | §4 `mint_uuid7` + §8 k-sortability proptest + `uuid7.json` |
| IAM can depend on the published kernel for PRN construction | `Prn`/`build`/`mint_uuid7` public Rust API (kernel crate) |
| Org-only encoding; accessors for service/region/org/type/id | §3 grammar + §4 `Prn` accessors |
| PRN ↔ Cedar UID mapping helper | §4 `cedar.rs` + §6 `prn_cedar.json` |
| Cross-binding parity vectors | §6 |
| Bind via PyO3/napi/wasm; smoke each; clean Py/TS surface | §5 + §8 |

## 10. ADR-0014 amendment (required)

ADR-0014 is *Proposed*. Two items must be corrected before/with this work:

1. **Org-self PRN form** → `prn:pgs:iam:::organization/<org>` (empty tenant slot, org UUID as
   resource-id). The ADR's `prn:pgs:iam::org_…:organization` and the `org_…` examples are
   superseded.
2. **No `org_` prefix in the canonical org slot** — the tenant slot is the raw UUID; any type
   prefix is display-only and never part of the PRN.

Action: amend the Notion ADR (offered at GATE 1). The ADR's "supersede, don't edit in place on
revisit" rule applies to *revisits of a decided ADR*; this is a pre-acceptance correction of a
*Proposed* ADR, so an in-place wording fix is appropriate.

## 11. Out of scope (explicit)

- Retiring the `sum` placeholder + its corpus/bindings/tests (follow-up).
- A rich cross-FFI `Prn` object/class (YAGNI; revisit when a consumer needs it).
- Tenancy semantics / org-presence-per-type validation (IAM's job).
- `region` semantics, multi-region, partition splits (forward-compat fields only).
- Cedar policy evaluation / `cedar-policy` integration (ADR-0013, separate work).
- Slugs / display names (separate mutable field, never in the PRN).
- npm/PyPI publication of the new surface (release activation, separate issues).
