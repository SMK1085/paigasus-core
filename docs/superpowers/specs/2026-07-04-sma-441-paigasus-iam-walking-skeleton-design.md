# SMA-441 — `paigasus-iam` M0 walking skeleton

**Status:** Draft — revised after adversarial challenge (pending GATE 1 approval)
**Date:** 2026-07-04
**Linear:** SMA-441 (Epic M0 of 6, IAM v1; blocks SMA-442 / IAM M1 Tenancy)
**ADRs:** ADR-0004 (contracts/buf codegen), ADR-0005 (kernel + bindings), ADR-0014 (Tenancy & PRN)
**References:** Notion — [IAM Architecture & Data Model](https://app.notion.com/p/385830e8fbaa81a6b516cf6a3646743d)

## 1. Context & goal

Stand up the **first vertical slice** of a brand-new `paigasus-iam` service: a running,
hexagonal *walking skeleton* that later milestones (M1–M5) grow into the full IAM system.
This is deliberately **not** the IAM system — it is the scaffold that proves the shape works
end-to-end: a pure domain libs crate, a hexagonal service crate, an `iam.proto` wired through
buf codegen, axum + tonic health surfaces, a SeaORM/Postgres `Principal`/`User` round-trip,
structured JSON logs, config, and green Moon CI.

The value is **pattern-setting**: `paigasus-iam` is the first real Paigasus *service* (the
existing `paigasus-gateway` is a `fn main() {}` stub). Every choice here — logging crate,
health mechanism, port topology, DB test strategy, hexagonal module layout — becomes the
template M1+ and future services follow. Findings from the Stage-2 adversarial challenge (C1–C15
in §3) are folded in for exactly this reason: a wrong pattern here propagates.

## 2. Scope

### In scope (M0)

1. `paigasus-logging` — new shared libs crate: `init(service)` installs a JSON
   `tracing-subscriber` layer + env-filter. First consumer is `paigasus-iam`; reused by
   every future service (and, later, the gateway).
2. `paigasus-iam-core` — new pure-domain libs crate: `Principal`, `User`, value objects
   (`PrincipalId`, `Email`), `PrincipalKind`/`PrincipalStatus` enums, and the port traits
   (`PrincipalRepository`, `IdGenerator`, `Clock`). No I/O, no SeaORM, no axum/tonic. Depends on
   `paigasus-kernel` (for `Prn`), `uuid`, `chrono`, `thiserror`, `async-trait`.
3. `paigasus-iam` — new hexagonal **service** crate under `rs/crates/services/`:
   `application/` use cases, `adapters/` (http, grpc, persistence, id, clock), `config`, `main`.
4. `contracts/proto/paigasus/iam/v1/iam.proto` — placeholder message(s) + comment-reserved
   future RPC/message names; wired through the existing `contracts:generate` buf pipeline
   (rs/py/ts), generated output committed and drift-gated. **AC-mandated** (issue scope item);
   the message exercises the iam codegen path even though nothing consumes it in M0.
5. axum HTTP (`/healthz`, `/readyz`) + tonic gRPC (`grpc.health.v1`) on **two ports**, each
   with its protocol-correct middleware built from **one shared middleware *config*** (§6).
6. SeaORM + Postgres wiring: migration `m0001` + entities for `principal` + `user`
   (1:1, shared PK); `PgPrincipalRepository` adapter; a real create→read round-trip.
7. figment config (defaults ← optional TOML ← `IAM_` env), `database_url` required; plus a
   committed `iam.toml.example` documenting every key so a dev can run `main` locally.
8. Moon projects (`-rs` suffix) with correct `dependsOn`/`^:build` wiring; CI green across
   `build / lint / fmt / test / deny / machete / typecheck / breaking`.

### Explicitly out of scope (deferred to M1–M5)

Cedar/authz engine · Redis/caches · OIDC/`Authenticator` (JWKS) · `ApiKey` · `Policy`/`Schema` ·
tenancy (`Organization`/`Team`/`Project`/`Membership`) · `Role`/`RoleGrant` · `ServiceAccount` ·
`ExternalIdentity` · outbox/domain events · OTel/Prometheus · the `IsAuthorized`/`Introspect`
RPCs. `iam.proto` **comment-reserves** these names but defines none of them. `Principal.kind`
and `.status` each carry exactly **one** M0 variant (`User`, `Active`); the enums exist so the
columns and domain types are shaped for M1 without a migration rewrite.

## 3. Resolved design decisions

From the brainstorm (**B**) and the Stage-2 adversarial challenge (**C**).

**Brainstorm:**
1. **(B) DB round-trip is verified with testcontainers, Docker-gated** — real Postgres in Docker
   (`testcontainers` + `testcontainers-modules`). Chosen over a CI Postgres `services:` block
   (workflow surgery on the single tuned `moon ci` job) and over a fake-only CI (would not verify
   the literal "through Postgres" AC).
2. **(B) Structured logging in a shared `paigasus-logging` crate**, not inline.
3. **(B) gRPC health is the well-known `grpc.health.v1`** via `tonic-health`, not a bespoke RPC.
4. **(B) Two ports** (axum `:8080`, tonic `:9090`).
5. **(B) config via figment** (defaults ← optional `iam.toml` ← `Env::prefixed("IAM_")`).
6. **(B) `/readyz` included** (DB `SELECT 1` → 200/503) beyond the AC.
7. **(B) Domain purity via injected dependencies** — the core never touches a clock or entropy;
   `IdGenerator` and `Clock` ports feed it, so unit tests are deterministic.

**Challenge fixes (all folded in; verdict was APPROVE WITH CHANGES, no blockers):**
8. **(C-MAJOR) gRPC health gets a real test** — `tests/grpc_health.rs` boots tonic on an
   ephemeral port and asserts `grpc.health.v1.Health/Check` → `SERVING` (no DB). AC #1 was
   otherwise unverified (§8/§12).
9. **(C-MAJOR) The Docker skip is a hard gate in CI.** GitHub sets `CI=true`. The round-trip
   test skips only when `CI` is **unset** (local, Docker-less); when `CI` **is** set and Docker
   is unreachable it **panics/fails**. This removes the "silent-green-forever" hole — made worse
   by `.moon/tasks.yml` `outputStyle: buffer-only-failure` + nextest capture hiding a skip
   `eprintln!` on a pass (§8).
10. **(C-MAJOR) A `Clock` port + microsecond timestamps.** Postgres `TIMESTAMPTZ` is µs-resolution
    while `chrono::DateTime<Utc>` is ns; a raw `SystemTime::now()` truncates on store and breaks
    the round-trip `assert_eq`. Fix: a `Clock` port yields **µs-truncated** UTC times; the use
    case stamps `created_at`/`updated_at` from it (deterministic in tests via a fixed clock), so
    persisted and read-back values are bit-equal (§4.1/§5). Resolves the "where do timestamps come
    from" gap in the pure-core story.
11. **(C-MAJOR) `cargo-deny` OpenSSL exception is pre-committed, not "as needed".** The rustls
    crypto backend (`ring`) carries the **OpenSSL** license, which `rs/deny.toml`'s allow-list
    omits. Pin the backend to `ring` and add `[[licenses.exceptions]] name="ring", allow=["OpenSSL"]`
    up front (if resolution pulls `aws-lc-sys` instead, add its exception too). `multiple-versions
    = "warn"` means duplicate versions won't fail (§10/§11).
12. **(C-MAJOR) `paigasus-iam` does NOT depend on `paigasus-proto` in M0.** Health is
    `grpc.health.v1` (tonic-health's own proto); nothing in the service consumes an `iam/v1`
    generated type, so a Cargo dep would trip `cargo-machete`. The iam codegen path is exercised
    by the **`paigasus-proto` crate** compiling the `iam/v1` module. The Cargo dep (and any Moon
    `dependsOn` on `paigasus-proto`) returns in M1 when a real RPC lands (§9).
13. **(C-MAJOR) `iam/v1` wiring mirrors `common/v1`, not `gateway/v1`.** A serviceless proto emits
    **no** `.tonic.rs` (neoeinstein-tonic skips packages with no service — see
    `paigasus-proto/src/lib.rs:18-26`). The `include!` for `paigasus::iam::v1` includes the prost
    `.rs` **only** (§9). Copying the `gateway/v1` block (which includes `.tonic.rs`) would break
    `build`.
14. **(C-MAJOR) No "one shared `ServiceBuilder`" — shared *config*, protocol-correct layers.**
    axum needs `TraceLayer::new_for_http()`; tonic needs `new_for_grpc()` (different layer types
    that cannot unify in one `ServiceBuilder<L>`), and `tower_http::timeout::TimeoutLayer` returns
    an HTTP 408 that is wrong for gRPC. Each server builds its own stack from a shared config
    struct (timeout `Duration`, trace settings); the gRPC timeout uses **tonic's** built-in request
    timeout (§6).
15. **(C-MAJOR) `PrincipalRepository` uses `#[async_trait]`.** Native `async fn` in traits yields
    non-`Send` futures, but the repo is awaited inside axum/tonic handlers on the multi-thread
    runtime (needs `Send`) and is injected as a trait object. `#[async_trait]` gives both. Add
    `async-trait` (already workspace-pinned) to the core + service crates. `IdGenerator`/`Clock`
    stay plain sync traits.
16. **(C-MINOR, folded)** `RepositoryError::Backend` preserves the source
    (`Box<dyn Error + Send + Sync>`), not a `String`; the adapter maps `DbErr` (incl. unique →
    `Conflict`). SeaORM derives live on the **entity** enums, never on the pure-core enums (mapped
    in `pg_repository`). `Email` rule pinned (§4.1). `PrincipalId` wraps just `Prn` with a `.uuid()`
    accessor (the UUID is `prn.resource_id()` — no redundancy). The `KernelIdGenerator` adapter
    `.expect()`s the statically-infallible `Prn::build`. Graceful shutdown fans one signal out to
    both servers via `tokio::sync::watch` (§7). All new `.rs` files carry the SPDX header;
    `iam.proto` passes `buf format` + STANDARD lint (`contracts:fmt`/`contracts:lint` gates). A
    `config` unit test covers figment layering + the missing-`database_url` error (§8).

## 4. Architecture

Hexagonal, mirroring the Notion sketch (trimmed to M0). Ports (traits) live in the pure core;
adapters (impls) live in the service; the wire model lives in `contracts/`.

```
contracts/proto/paigasus/iam/v1/iam.proto        # placeholder msg + reserved-name comments

rs/crates/libs/paigasus-logging/
  src/lib.rs            # pub fn init(service: &str) -> JSON tracing-subscriber + EnvFilter

rs/crates/libs/paigasus-iam-core/                # pure domain (native; deps: kernel, uuid, chrono,
  src/lib.rs                                     #                    thiserror, async-trait)
  src/principal.rs      # Principal { id, kind, status, created_at, updated_at }
                        # PrincipalKind { User }, PrincipalStatus { Active }
  src/user.rs           # User { principal_id, email, display_name, locale?, timezone?, ts }
  src/value.rs          # PrincipalId(Prn) + .uuid(); Email (VO)
  src/ports.rs          # PrincipalRepository (#[async_trait]), IdGenerator, Clock + RepositoryError

rs/crates/services/paigasus-iam/                 # hexagonal service binary
  src/main.rs           # composition root (see §7)
  src/config.rs         # figment IamConfig  (+ unit test)
  src/application/
    create_user.rs      # CreateUser use case: id_gen + clock -> Principal+User -> repo.create_user
  src/adapters/
    http/mod.rs         # axum Router: GET /healthz, GET /readyz(db ping)
    grpc/mod.rs         # tonic server + tonic-health grpc.health.v1
    id/mod.rs           # KernelIdGenerator (SystemTime + rand -> mint_uuid7 -> Prn::build)
    clock/mod.rs        # SystemClock (Utc::now truncated to microseconds)
    persistence/
      entities/{principal.rs,user.rs}   # SeaORM entities (own persistence enums)
      migration/{mod.rs,m0001_...}      # sea-orm-migration Migrator
      pg_repository.rs                  # PgPrincipalRepository impl PrincipalRepository
  tests/roundtrip.rs    # testcontainers Postgres integration (Docker-gated, hard-fail in CI)
  tests/grpc_health.rs  # tonic Health/Check -> SERVING on ephemeral port (no DB)
  tests/health.rs       # axum /healthz oneshot -> 200 (no DB)
  iam.toml.example      # documents http_addr/grpc_addr/database_url/log_level + IAM_* env
```

### 4.1 Ports & value objects (in `paigasus-iam-core`)

```rust
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("conflict: {0}")] Conflict(String),          // unique violation (email/prn)
    #[error(transparent)] Backend(#[from] Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait::async_trait]
pub trait PrincipalRepository: Send + Sync {
    async fn create_user(&self, principal: &Principal, user: &User) -> Result<(), RepositoryError>;
    async fn find_user(&self, id: &PrincipalId)
        -> Result<Option<(Principal, User)>, RepositoryError>;
}

pub trait IdGenerator: Send + Sync { fn new_principal_id(&self) -> PrincipalId; } // Prn + uuidv7
pub trait Clock: Send + Sync { fn now(&self) -> chrono::DateTime<chrono::Utc>; }  // µs-truncated
```

- **`PrincipalId(Prn)`** with `fn uuid(&self) -> Uuid { self.0.resource_id() }` — the UUID is the
  PK/FK; the PRN is the canonical name. No stored redundancy.
- **`Email`** value object — rule: **non-empty, exactly one `@`, non-empty local part and non-empty
  domain part**; nothing more for M0. `Email::parse(&str) -> Result<Email, DomainError>`.
- `KernelIdGenerator` `.expect()`s `Prn::build("iam","",None,"principal",uuid)` (statically
  infallible for these fixed, valid inputs).

### 4.2 PRN shape (verified against the kernel)

`service=iam`, `region=""` (v1 mints empty region), `org=None`, `resource_type="principal"`,
`resource_id=<uuidv7>` → canonical `prn:pgs:iam:::principal/<uuid>` (empty region **and** empty
org ⇒ three consecutive colons). Confirmed: `Prn::build("iam","",None,"principal",uuid)`
(`resource_name.rs:155`) with empty region (validation skipped, `:131`) and `is_valid_label`
accepting `"iam"`/`"principal"` produces exactly this.

## 5. Data model — migration `m0001`

| Table | Columns |
|---|---|
| `principal` | `id UUID PK` (UUIDv7); `prn TEXT NOT NULL UNIQUE`; `kind TEXT NOT NULL`; `status TEXT NOT NULL`; `created_at TIMESTAMPTZ NOT NULL`; `updated_at TIMESTAMPTZ NOT NULL` |
| `user` | `principal_id UUID PK REFERENCES principal(id) ON DELETE CASCADE`; `email TEXT NOT NULL UNIQUE`; `display_name TEXT NOT NULL`; `locale TEXT NULL`; `timezone TEXT NULL`; `created_at TIMESTAMPTZ NOT NULL`; `updated_at TIMESTAMPTZ NOT NULL` |

`User` is a subtype of `Principal` (shared PK, 1:1). `kind`/`status` persist as **text** — the
SeaORM entity owns its persistence representation (a `String`/entity-side enum), mapped to/from
the pure-core `PrincipalKind`/`PrincipalStatus` in `pg_repository` (SeaORM derives never touch the
core enums — hexagonal purity). Timestamps are `chrono::DateTime<Utc>` **truncated to microseconds**
by the `Clock` (§3-C10) so round-trip equality holds against `TIMESTAMPTZ`. Migrations run via
`Migrator::up` at boot (§7) and in the integration test (test DB is single-connection, so the
skeleton's boot-time migration has no concurrency concern; a dedicated migration entrypoint is an
M1 follow-up).

## 6. Health + gRPC surfaces (two ports, shared config)

A single `MiddlewareConfig { request_timeout: Duration, … }` is built from `IamConfig` and passed
to **both** server builders; each applies its **protocol-correct** layers:

- **axum** on `http_addr` (default `0.0.0.0:8080`): `TraceLayer::new_for_http()` +
  `tower_http::timeout::TimeoutLayer`.
  - `GET /healthz` → always `200 {"status":"ok"}` (liveness)
  - `GET /readyz` → DB ping (`SELECT 1`) → `200 {"status":"ready"}` / `503 {"status":"unready"}`
- **tonic** on `grpc_addr` (default `0.0.0.0:9090`): `TraceLayer::new_for_grpc()` + tonic's
  **built-in** request timeout (`Server::builder().timeout(..)`, which yields the correct
  `DEADLINE_EXCEEDED`/`grpc-status`, not an HTTP 408). `grpc.health.v1.Health` via `tonic-health`,
  set `SERVING` at startup.

There is deliberately **no** single `tower::ServiceBuilder` shared object (the two layer stacks are
different concrete types); the shared unit is the config, not the builder.

## 7. Composition root (`main.rs`)

```
paigasus_logging::init("paigasus-iam")
  → IamConfig::load()                     # figment: defaults ← iam.toml? ← IAM_* env; database_url required
  → SeaORM connect(database_url)          # fail-fast (typed error out of main) if unreachable
  → Migrator::up(&db, None)
  → let (tx, rx) = tokio::sync::watch::channel(())          # shutdown fan-out
  → spawn axum::serve(..).with_graceful_shutdown(rx.clone())
  → spawn tonic Server::builder().timeout(..).add_service(health).serve_with_shutdown(rx.clone())
  → await SIGINT/SIGTERM -> tx.send(()) ; try_join! both handles
```

`IamConfig { http_addr: SocketAddr, grpc_addr: SocketAddr, database_url: String, log_level: String }`.
`database_url` has **no default** (fail-fast with a clear, typed error). `main` returns
`anyhow::Result<()>` so connect/bind failures surface as a non-zero exit with context.

## 8. Testing strategy

| Test | Location | DB | CI behaviour |
|---|---|---|---|
| Domain invariants (`Email` rule, `Principal`/`User` construction, PRN formation) | `iam-core` unit | no | always |
| `CreateUser` use case against `InMemoryPrincipalRepository` + `FixedIdGenerator` + `FixedClock` | `iam` unit | no | always |
| **Principal/User round-trip** (AC #2): migrate → `create_user` → `find_user` → assert eq | `iam` `tests/roundtrip.rs`, testcontainers | **yes** | runs (Docker present); **skips only when `CI` unset**; **hard-fails when `CI` set + Docker absent** |
| **gRPC `Health/Check` → SERVING** (AC #1) | `iam` `tests/grpc_health.rs`, ephemeral port | no | always |
| HTTP `/healthz` → 200 via `oneshot` (AC #1) | `iam` `tests/health.rs` | no | always |
| `IamConfig` figment layering + missing-`database_url` error | `iam` `config` unit | no | always |

`/readyz`'s 200 path is covered by the round-trip container run; its 503 path can be checked with an
unreachable DB URL (no container). The Docker gate (§3-C9): `if docker_unavailable() { if
std::env::var_os("CI").is_some() { panic!(...) } else { eprintln!("skip: no Docker"); return } }`.
`nextest` counts an early return as a pass; the CI branch guarantees the real path is exercised on
every PR. Nextest runs with `--no-tests=pass` per the workspace convention.

## 9. Moon / CI wiring

- **New Moon projects:** `paigasus-logging-rs`, `paigasus-iam-core-rs`, `paigasus-iam-rs`, each with
  `build`/`test` `deps: ['^:build']`, `layer:` set (`library` / `library` / `application`), SPDX-
  headed sources.
  - `paigasus-iam-core-rs` → `dependsOn: [paigasus-kernel-rs]`
  - `paigasus-iam-rs` → `dependsOn: [paigasus-iam-core-rs, paigasus-logging-rs, paigasus-kernel-rs]`
    (**no** `paigasus-proto-rs` in M0 — §3-C12)
- **Proto:** `iam.proto` flows through the existing `contracts:generate`. Wire `paigasus::iam::v1`
  into `paigasus-proto/src/lib.rs` **mirroring the `common/v1` block** — prost `.rs` only, **no**
  `.tonic.rs` (§3-C13). Regenerate + commit rs/py/ts; the **codegen drift gate** must be green.
  `iam.proto` must pass `buf format` (`contracts:fmt`) and STANDARD lint (`contracts:lint`).
- **CI task list is unchanged** — the fixed `:build :test :lint :fmt :deny :machete :typecheck
  :breaking …` graph picks up the new crates via wildcards; Docker is already on the runner. No
  `ci.yml` edits.

## 10. Workspace dependencies to add

Added to `rs/Cargo.toml` `[workspace.dependencies]` (Sven curates these; each is a MINIMAL feature
baseline that unions across the workspace):

- `sea-orm` — `sqlx-postgres`, `runtime-tokio-rustls` (**ring** backend), `macros`, `with-uuid`,
  `with-chrono` (rustls, not native-tls, to match the `reqwest` posture)
- `sea-orm-migration` — for `Migrator`
- `figment` — `toml`, `env`
- `tonic-health`
- `rand` — service-only entropy for the kernel mint (native; **not** in the wasm tree, so
  `wasm-getrandom-free` is untouched)
- `async-trait` — already pinned; consumed by the core (repository port) + service adapter
- `chrono` — already pinned; now consumed by `iam-core` (timestamps) + service
- `tower-http` — add `trace`, `timeout` features (crate already pinned)
- `tokio` — the service adds `rt-multi-thread`, `macros`, `net`, `signal`, `time`, `sync`
- **dev-deps:** `testcontainers`, `testcontainers-modules` (`postgres`)
- **`rs/deny.toml`:** add `[[licenses.exceptions]] name = "ring", allow = ["OpenSSL"]` (§3-C11)

## 11. Risks & mitigations

1. **`cargo-deny` licenses.** Concretely pre-committed: the `ring` OpenSSL exception (§3-C11). If
   `aws-lc-sys` is resolved instead of `ring`, add its exception too. `multiple-versions = "warn"`
   means duplicate versions won't red. Run `moon run repo:deny` before pushing.
2. **`cargo-machete`.** Resolved structurally by **not** depending on `paigasus-proto` (§3-C12) and
   by only adding a workspace dep when its consumer lands in the same step.
3. **Codegen drift gate.** Regenerate (`moon run contracts:generate`) + commit rs/py/ts after any
   `iam.proto` edit; py (betterproto2) and ts (protobuf-es) must emit cleanly for `iam/v1`.
4. **testcontainers image pull in CI.** Pin `postgres:16-alpine` for reproducibility; first run
   pulls it (runner has network).
5. **Timestamp / round-trip equality.** Closed by the µs-truncating `Clock` (§3-C10).
6. **gRPC/HTTP middleware type mismatch.** Closed by dropping the shared-`ServiceBuilder` framing
   (§3-C14).

## 12. Acceptance-criteria mapping

| AC | Satisfied by |
|---|---|
| Service boots; HTTP + gRPC health checks pass | §7 composition root; `tests/health.rs` (HTTP `/healthz`) + `tests/grpc_health.rs` (`grpc.health.v1` → SERVING) |
| One `Principal`/`User` row round-trips through Postgres | §8 `tests/roundtrip.rs` (testcontainers, real Postgres; hard-fails in CI if Docker absent) |
| `moon ci :build` and `moon ci :test` are green | §9 Moon wiring; §10/§11 keep `deny`/`machete`/drift/fmt/lint green |

## 13. Open questions

None blocking. figment, `/readyz`, testcontainers-vs-`services:`, and keeping the (AC-mandated)
placeholder `iam.proto` were all confirmed. Remaining choices are implementation detail for the
plan: the exact `docker_available()` probe, and whether `PrincipalRepository` is injected as
`Arc<dyn …>` (default) vs a generic (both work with `#[async_trait]`).
