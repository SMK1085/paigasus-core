# SMA-441 — `paigasus-iam` M0 walking skeleton

**Status:** Draft — pending adversarial challenge & GATE 1 approval
**Date:** 2026-07-04
**Linear:** SMA-441 (Epic M0 of 6, IAM v1; blocks SMA-442 / IAM M1 Tenancy)
**ADRs:** ADR-0004 (contracts/buf codegen), ADR-0005 (kernel + bindings), ADR-0014 (Tenancy & PRN)
**References:** Notion — [IAM Architecture & Data Model](https://app.notion.com/p/385830e8fbaa81a6b516cf6a3646743d)

## 1. Context & goal

Stand up the **first vertical slice** of a brand-new `paigasus-iam` service: a running,
hexagonal *walking skeleton* that later milestones (M1–M5) grow into the full IAM system.
This is deliberately **not** the IAM system — it is the scaffold that proves the shape works
end-to-end: a pure domain libs crate, a hexagonal service crate, an `iam.proto` wired through
buf codegen, axum + tonic health surfaces sharing one tower stack, a SeaORM/Postgres
`Principal`/`User` round-trip, structured JSON logs, config, and green Moon CI.

The value is **pattern-setting**: `paigasus-iam` is the first real Paigasus *service* (the
existing `paigasus-gateway` is a `fn main() {}` stub). Every choice here — logging crate,
health mechanism, port topology, DB test strategy, hexagonal module layout — becomes the
template M1+ and future services follow.

## 2. Scope

### In scope (M0)

1. `paigasus-logging` — new shared libs crate: `init(service)` installs a JSON
   `tracing-subscriber` layer + env-filter. First consumer is `paigasus-iam`; reused by
   every future service (and, later, the gateway).
2. `paigasus-iam-core` — new pure-domain libs crate: `Principal`, `User`, value objects
   (`PrincipalId`, `Email`), `PrincipalKind`/`PrincipalStatus` enums, and the port traits
   (`PrincipalRepository`, `IdGenerator`). No I/O, no SeaORM, no axum/tonic. Depends on
   `paigasus-kernel` only for the `Prn` value type.
3. `paigasus-iam` — new hexagonal **service** crate under `rs/crates/services/`:
   `application/` use cases, `adapters/` (http, grpc, persistence, id), `config`, `main`.
4. `contracts/proto/paigasus/iam/v1/iam.proto` — placeholder messages + comment-reserved
   future RPC/message names; wired through the existing `contracts:generate` buf pipeline
   (rs/py/ts), with the generated output committed and drift-gated.
5. axum HTTP (`/healthz`, `/readyz`) + tonic gRPC (`grpc.health.v1`) on **two ports**, built
   from **one shared `tower::ServiceBuilder`** layer stack.
6. SeaORM + Postgres wiring: migration `m0001` + entities for `principal` + `user`
   (1:1, shared PK); `PgPrincipalRepository` adapter; a real create→read round-trip.
7. figment config (defaults ← optional TOML ← `IAM_` env), `database_url` required.
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

From the brainstorm (**B**); challenge findings will be appended as **C** after Stage 2.

1. **(B) DB round-trip is verified with testcontainers, Docker-gated.** The AC-#2 round-trip
   runs against an **ephemeral Postgres in Docker** (`testcontainers` + `testcontainers-modules`).
   GitHub `ubuntu-latest` ships Docker, so it runs for real in CI; on a Docker-less laptop the
   test **detects Docker's absence at runtime and skips cleanly** (returns Ok), keeping
   `moon run paigasus-iam-rs:test` green everywhere. Chosen over a CI Postgres service
   (workflow surgery on the single tuned `moon ci` job) and over a fake-only CI (would not
   verify the literal "through Postgres" AC in CI).
2. **(B) Structured logging lives in a shared `paigasus-logging` crate**, not inline — the
   Notion doc names "paigasus-logging conventions" and M1+ services need the same setup.
3. **(B) gRPC health is the well-known `grpc.health.v1`** served via `tonic-health` (what
   k8s gRPC probes / `grpc_health_probe` expect), **not** a bespoke `HealthService` RPC.
   `iam.proto` still carries placeholder messages to exercise the iam codegen path.
4. **(B) Two ports, one shared tower layer stack.** axum HTTP (`:8080`) and tonic gRPC
   (`:9090`) are each built from a single `tower::ServiceBuilder` factory (`TraceLayer`,
   `TimeoutLayer`), giving clean k8s liveness/readiness probes and the least-magic wiring.
   (The gRPC `TraceLayer` uses `new_for_grpc()`; the "shared stack" is one layer-factory
   applied to both servers.)
5. **(B) config via figment**, layered defaults ← optional `iam.toml` ← `Env::prefixed("IAM_")`.
6. **(B) `/readyz` is included** (DB `SELECT 1` → 200/503) beyond the AC — a skeleton without
   a readiness probe is incomplete, and it exercises the DB in the HTTP path.
7. **(B) Domain purity via injected IDs.** `paigasus-iam-core` never touches a clock or
   entropy; the `IdGenerator` port mints IDs, so unit tests inject deterministic IDs. The
   real `KernelIdGenerator` (service adapter) supplies `SystemTime` + `rand` bytes to the
   kernel's `mint_uuid7`.

## 4. Architecture

Hexagonal, mirroring the Notion sketch (trimmed to M0). Ports (traits) live in the pure core;
adapters (impls) live in the service; the wire model lives in `contracts/`.

```
contracts/proto/paigasus/iam/v1/iam.proto        # placeholder msgs + reserved-name comments

rs/crates/libs/paigasus-logging/
  src/lib.rs            # pub fn init(service: &str) -> JSON tracing-subscriber + EnvFilter

rs/crates/libs/paigasus-iam-core/                # pure domain (native; deps: paigasus-kernel)
  src/lib.rs
  src/principal.rs      # Principal { id, kind, status, created_at, updated_at }
                        # PrincipalKind { User }, PrincipalStatus { Active }
  src/user.rs           # User { principal_id, email, display_name, locale?, timezone?, ts }
  src/value.rs          # PrincipalId(Prn + Uuid), Email (VO, light validation)
  src/ports.rs          # PrincipalRepository, IdGenerator (traits) + RepositoryError

rs/crates/services/paigasus-iam/                 # hexagonal service binary
  src/main.rs           # composition root (see §7)
  src/config.rs         # figment IamConfig
  src/application/
    mod.rs
    create_user.rs      # CreateUser use case (mint id -> Principal+User -> repo.create_user)
  src/adapters/
    http/mod.rs         # axum Router: GET /healthz, GET /readyz
    grpc/mod.rs         # tonic server + tonic-health grpc.health.v1
    id/mod.rs           # KernelIdGenerator (SystemTime + rand -> kernel mint_uuid7 + Prn::build)
    persistence/
      mod.rs
      entities/{principal.rs,user.rs}   # SeaORM entities
      migration/{mod.rs,m0001_...}      # sea-orm-migration Migrator
      pg_repository.rs                  # PgPrincipalRepository impl PrincipalRepository
  tests/roundtrip.rs    # testcontainers Postgres integration (Docker-gated)
  tests/health.rs       # axum /healthz oneshot -> 200 (fast, no DB)
```

### 4.1 Ports (in `paigasus-iam-core`)

```rust
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError { /* NotFound?, Conflict(unique), Backend(String) */ }

pub trait PrincipalRepository: Send + Sync {
    async fn create_user(&self, principal: &Principal, user: &User) -> Result<(), RepositoryError>;
    async fn find_user(&self, id: &PrincipalId)
        -> Result<Option<(Principal, User)>, RepositoryError>;
}

pub trait IdGenerator: Send + Sync {
    fn new_principal_id(&self) -> PrincipalId;   // (Uuid v7, Prn) minted together
}
```

Native `async fn` in traits (stable since 1.75; per the Notion Rust guidelines and the
workspace `async-trait` note). If a `dyn PrincipalRepository` is required at a call site,
that site uses `async-trait`; otherwise generics keep it allocation-free — the plan pins which.

### 4.2 PRN shape

An M0 principal is `service=iam`, `region=""` (v1 mints empty region), `org=None` (no tenancy
yet), `resource_type="principal"`, `resource_id=<uuidv7>` — canonical:
`prn:pgs:iam:::principal/<uuid>` (empty region **and** empty org ⇒ three consecutive colons).
Built via `paigasus_kernel::Prn::build("iam", "", None, "principal", uuid)`.

## 5. Data model — migration `m0001`

| Table | Columns |
|---|---|
| `principal` | `id UUID PK` (UUIDv7); `prn TEXT NOT NULL UNIQUE`; `kind TEXT NOT NULL`; `status TEXT NOT NULL`; `created_at TIMESTAMPTZ NOT NULL`; `updated_at TIMESTAMPTZ NOT NULL` |
| `user` | `principal_id UUID PK REFERENCES principal(id) ON DELETE CASCADE`; `email TEXT NOT NULL UNIQUE`; `display_name TEXT NOT NULL`; `locale TEXT NULL`; `timezone TEXT NULL`; `created_at TIMESTAMPTZ NOT NULL`; `updated_at TIMESTAMPTZ NOT NULL` |

`User` is a subtype of `Principal` (shared PK, 1:1). `kind`/`status` are **text-backed**
(SeaORM `DeriveActiveEnum`, string repr) — no native PG enums, so the migration stays trivial
and adding M1 variants needs no `ALTER TYPE`. Timestamps are UTC (`chrono::DateTime<Utc>`).
Migrations run programmatically via `Migrator::up` (see §7) and in the integration test.

## 6. Health + gRPC surfaces

- **axum** on `http_addr` (default `0.0.0.0:8080`):
  - `GET /healthz` → always `200 {"status":"ok"}` (liveness)
  - `GET /readyz` → DB ping (`SELECT 1` via SeaORM) → `200 {"status":"ready"}` or
    `503 {"status":"unready"}` (readiness)
- **tonic** on `grpc_addr` (default `0.0.0.0:9090`): `grpc.health.v1.Health` via `tonic-health`,
  status set `SERVING` at startup.

Both are built from a shared `fn service_layers() -> ServiceBuilder<…>` factory
(`TraceLayer` + `TimeoutLayer`).

## 7. Composition root (`main.rs`)

```
paigasus_logging::init("paigasus-iam")
  → IamConfig::load()                     # figment: defaults ← iam.toml? ← IAM_* env
  → SeaORM connect(config.database_url)   # fail-fast if unreachable
  → Migrator::up(&db, None)
  → build shared tower layers
  → spawn axum::serve(http_addr, http_router(db.clone()).layer(layers))
  → spawn tonic Server::builder().layer(layers).add_service(health).serve(grpc_addr)
  → await both; graceful shutdown on SIGINT/SIGTERM
```

`IamConfig { http_addr: SocketAddr, grpc_addr: SocketAddr, database_url: String, log_level:
String }`. `database_url` has **no default** (fail-fast with a clear error if unset).

## 8. Testing strategy

| Test | Location | DB | CI |
|---|---|---|---|
| Domain invariants (`Email` validation, `Principal`/`User` construction, PRN formation) | `paigasus-iam-core` unit | no | always |
| `CreateUser` use case against `InMemoryPrincipalRepository` + `FixedIdGenerator` fakes | `paigasus-iam` unit | no | always |
| **Principal/User round-trip** (AC #2): migrate → `create_user` → `find_user` → assert eq | `paigasus-iam` `tests/roundtrip.rs`, testcontainers | **yes** | **yes** on `ubuntu-latest`; **skips** locally when Docker absent |
| HTTP `/healthz` via `tower::ServiceExt::oneshot` → 200 | `paigasus-iam` `tests/health.rs` | no | always |

The round-trip test calls a `docker_available()` guard (checks the Docker daemon/socket or
`DOCKER_HOST`) and returns early (skip, with an `eprintln!`) when Docker is unreachable — so
the *same* `test` task is green in CI (Docker present) and on a Docker-less laptop. `nextest`
treats an early-returning test as a pass; the skip is logged, not silent-green-forever, because
CI always exercises the real path.

## 9. Moon / CI wiring

- **New Moon projects:** `paigasus-logging-rs`, `paigasus-iam-core-rs`, `paigasus-iam-rs`, each
  with `build`/`test` `deps: ['^:build']`.
  - `paigasus-iam-core-rs` → `dependsOn: [paigasus-kernel-rs]`
  - `paigasus-iam-rs` → `dependsOn: [paigasus-iam-core-rs, paigasus-logging-rs,
    paigasus-proto-rs, paigasus-kernel-rs]`
- **Proto:** `iam.proto` flows through the existing `contracts:generate`. Add the
  `paigasus::iam::v1` module wiring (`include!`) to `paigasus-proto/src/lib.rs`. Regenerate and
  commit rs/py/ts output; the **codegen drift gate** must be green.
- **CI task list is unchanged** — the fixed `:build :test :lint :fmt :deny :machete :typecheck
  :breaking …` graph picks up the new crates via wildcards. No `ci.yml` edits are required
  (Docker is already available on the runner).

## 10. Workspace dependencies to add

Added to `rs/Cargo.toml` `[workspace.dependencies]` (Sven curates these; each is a MINIMAL
feature baseline that unions across the workspace):

- `sea-orm` — `sqlx-postgres`, `runtime-tokio-rustls`, `macros`, `with-uuid`, `with-chrono`
  (rustls, not native-tls, to match the `reqwest` posture)
- `sea-orm-migration` — for `Migrator`
- `figment` — `toml`, `env`
- `tonic-health`
- `rand` — service-only entropy for the kernel mint (the service is native; **not** in the
  wasm tree, so this does not affect `wasm-getrandom-free`)
- `tower-http` — add `trace`, `timeout` features (crate already pinned)
- `tokio` — the service adds `rt-multi-thread`, `macros`, `net`, `signal`, `time`
- **dev-deps:** `testcontainers`, `testcontainers-modules` (`postgres`)

## 11. Risks & mitigations

1. **`cargo-deny` (biggest CI-red risk).** `sea-orm`/`sqlx`/`testcontainers` pull large trees;
   licenses or duplicate versions may trip `deny.toml`. Mitigation: run `moon run repo:deny`
   during implementation and add license-allow / `[[bans.skip]]` entries as needed, documented.
2. **`cargo-machete`.** Every added workspace dep must be consumed by at least one crate or the
   unused-dep gate fails. Mitigation: only add a dep when its consumer lands in the same step.
3. **Codegen drift gate.** Forgetting `moon run contracts:generate` after editing `iam.proto`
   → red. The py (betterproto2) and ts (protobuf-es) generators must also emit cleanly for the
   new `iam/v1` package. Mitigation: regenerate + commit as part of the proto step.
4. **testcontainers image pull in CI.** First run pulls `postgres:<tag>` (network available on
   the runner). Pin a specific tag (e.g. `postgres:16-alpine`) for reproducibility.
5. **`async fn` in traits + `dyn` object safety.** If a call site needs `dyn
   PrincipalRepository`, native async-in-trait isn't object-safe → use `async-trait` there. The
   plan pins generic-vs-dyn per call site.

## 12. Acceptance-criteria mapping

| AC | Satisfied by |
|---|---|
| Service boots; HTTP + gRPC health checks pass | §6/§7 composition root; `tests/health.rs` (HTTP) + `grpc.health.v1 SERVING` (gRPC) |
| One `Principal`/`User` row round-trips through Postgres | §8 `tests/roundtrip.rs` (testcontainers, real Postgres) |
| `moon ci :build` and `moon ci :test` are green | §9 Moon wiring; §11 risk mitigations keep `deny`/`machete`/drift green |

## 13. Open questions

None blocking. Config library (figment) and the extra `/readyz` probe were confirmed at design
approval. The generic-vs-`dyn` choice for `PrincipalRepository` and the exact `docker_available()`
mechanism are implementation details the plan will pin.
